//! Typed, database-side transformations for a displayed query result.
//!
//! The grid intentionally stores display strings, so filtering or sorting it in
//! memory would lose PostgreSQL types, SQL NULL identity, and unfetched rows.
//! This module instead composes a new, read-only query around the source SQL.

use super::{pg_snapshot, sql_lexer};

/// A transformation expressed in terms of zero-based result-column ordinals.
#[derive(Clone, Debug, Default, PartialEq)]
pub(crate) struct ResultTransform {
    pub(crate) projection: Vec<usize>,
    pub(crate) filters: Vec<ResultFilter>,
    pub(crate) orders: Vec<ResultOrder>,
    pub(crate) group_count: Option<usize>,
}

impl ResultTransform {
    /// Replaces the projected columns. An empty projection means all columns.
    pub(crate) fn set_projection(&mut self, projection: Vec<usize>) {
        self.projection = projection;
    }

    /// Appends a predicate evaluated before ordering and grouping.
    pub(crate) fn add_filter(&mut self, filter: ResultFilter) {
        self.filters.push(filter);
    }

    /// Removes every filter while preserving projection, ordering, and grouping.
    pub(crate) fn clear_filters(&mut self) {
        self.filters.clear();
    }

    /// Replaces or appends a sort key for a result column.
    pub(crate) fn set_order(&mut self, ordinal: usize, direction: OrderDirection, append: bool) {
        if !append {
            self.orders.clear();
        } else {
            self.orders.retain(|order| order.ordinal != ordinal);
        }
        self.orders.push(ResultOrder { ordinal, direction });
    }

    /// Cycles a single-column sort through ascending, descending, and none.
    pub(crate) fn toggle_order(&mut self, ordinal: usize) {
        let direction = self
            .orders
            .iter()
            .find(|order| order.ordinal == ordinal)
            .map(|order| order.direction);
        self.orders.clear();
        match direction {
            None => self.orders.push(ResultOrder {
                ordinal,
                direction: OrderDirection::Asc,
            }),
            Some(OrderDirection::Asc) => self.orders.push(ResultOrder {
                ordinal,
                direction: OrderDirection::Desc,
            }),
            Some(OrderDirection::Desc) => {}
        }
    }

    /// Clears sort keys without changing other result operations.
    pub(crate) fn clear_orders(&mut self) {
        self.orders.clear();
    }

    /// Groups by one result column and returns its occurrence count.
    pub(crate) fn set_group_count(&mut self, ordinal: Option<usize>) {
        self.group_count = ordinal;
    }

    /// Removes every result operation.
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    /// Returns true when the source query is displayed without transformations.
    pub(crate) fn is_empty(&self) -> bool {
        self.projection.is_empty()
            && self.filters.is_empty()
            && self.orders.is_empty()
            && self.group_count.is_none()
    }

    /// Produces a short, user-facing summary suitable for a results-zone label.
    pub(crate) fn summary(&self, headers: &[String]) -> String {
        let mut parts = Vec::new();
        if !self.filters.is_empty() {
            let suffix = if self.filters.len() == 1 { "" } else { "s" };
            parts.push(format!("{} filter{suffix}", self.filters.len()));
        }
        if !self.orders.is_empty() {
            let rendered = self
                .orders
                .iter()
                .map(|order| {
                    let name = headers.get(order.ordinal).map_or("?", String::as_str);
                    format!("{name} {}", order.direction.label())
                })
                .collect::<Vec<_>>()
                .join(", ");
            parts.push(format!("sort {rendered}"));
        }
        if !self.projection.is_empty() {
            let suffix = if self.projection.len() == 1 { "" } else { "s" };
            parts.push(format!("{} column{suffix}", self.projection.len()));
        }
        if let Some(ordinal) = self.group_count {
            let name = headers.get(ordinal).map_or("?", String::as_str);
            parts.push(format!("count by {name}"));
        }
        parts.join(" · ")
    }
}

/// One predicate over a result column.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResultFilter {
    pub(crate) ordinal: usize,
    pub(crate) op: FilterOp,
    pub(crate) value: FilterValue,
}

/// Supported database-side filter operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FilterOp {
    Eq,
    Ne,
    Lt,
    Lte,
    Gt,
    Gte,
    Contains,
    NotContains,
    StartsWith,
    NotStartsWith,
    EndsWith,
    NotEndsWith,
    IsNull,
    IsNotNull,
}

/// A value used by a typed filter predicate.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum FilterValue {
    Text(String),
    Integer(i64),
    Decimal(String),
    Boolean(bool),
    Null,
}

/// Parses a command-prompt value without conflating the text `NULL` with SQL NULL.
pub(crate) fn parse_filter_value(value: &str) -> FilterValue {
    let value = value.trim();
    if let Some(unquoted) = value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
    {
        return FilterValue::Text(unquoted.replace("''", "'"));
    }
    if let Some(unquoted) = value
        .strip_prefix('"')
        .and_then(|value| value.strip_suffix('"'))
    {
        return FilterValue::Text(unquoted.replace("\"\"", "\""));
    }
    if value.eq_ignore_ascii_case("true") {
        return FilterValue::Boolean(true);
    }
    if value.eq_ignore_ascii_case("false") {
        return FilterValue::Boolean(false);
    }
    if let Ok(value) = value.parse::<i64>() {
        return FilterValue::Integer(value);
    }
    if is_finite_decimal(value) {
        return FilterValue::Decimal(value.to_string());
    }
    FilterValue::Text(value.to_string())
}

/// One sort key over a result column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct ResultOrder {
    pub(crate) ordinal: usize,
    pub(crate) direction: OrderDirection,
}

/// Sort direction for a result column.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum OrderDirection {
    Asc,
    Desc,
}

impl OrderDirection {
    fn sql(self) -> &'static str {
        match self {
            Self::Asc => "ASC",
            Self::Desc => "DESC",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Asc => "asc",
            Self::Desc => "desc",
        }
    }
}

/// Compiles a result transformation around one read-only, row-producing query.
///
/// Deterministic ordinal aliases make duplicate, quoted, and expression-derived
/// result headers safe to address. The compiled statement remains a single
/// `SELECT`, so callers can use a database cursor for paging.
pub(crate) fn compile_result_transform(
    base_sql: &str,
    headers: &[String],
    spec: &ResultTransform,
) -> Result<String, String> {
    if headers.is_empty() {
        return Err("result has no columns to transform".to_string());
    }
    let source = sql_lexer::single_statement(base_sql)?;
    if !pg_snapshot::is_snapshot_candidate(source) {
        return Err("only a single read-only, row-returning query can be transformed".to_string());
    }
    validate_transform(headers.len(), spec)?;

    let aliases = (0..headers.len())
        .map(column_alias)
        .map(|alias| quote_identifier(&alias))
        .collect::<Vec<_>>();
    let source_alias = quote_identifier("__tsql_result");

    let select = if let Some(ordinal) = spec.group_count {
        let column = qualified_column(&source_alias, ordinal);
        format!(
            "{column} AS {}, COUNT(*) AS {}",
            quote_identifier(&headers[ordinal]),
            quote_identifier("count")
        )
    } else {
        let projection = if spec.projection.is_empty() {
            (0..headers.len()).collect::<Vec<_>>()
        } else {
            spec.projection.clone()
        };
        projection
            .iter()
            .map(|ordinal| {
                format!(
                    "{} AS {}",
                    qualified_column(&source_alias, *ordinal),
                    quote_identifier(&headers[*ordinal])
                )
            })
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut sql = format!(
        "SELECT {select}\nFROM (\n{source}\n) AS {source_alias} ({})",
        aliases.join(", ")
    );

    if !spec.filters.is_empty() {
        let predicates = spec
            .filters
            .iter()
            .map(|filter| compile_filter(&source_alias, filter))
            .collect::<Result<Vec<_>, _>>()?;
        sql.push_str("\nWHERE ");
        sql.push_str(&predicates.join("\n  AND "));
    }

    if let Some(ordinal) = spec.group_count {
        sql.push_str("\nGROUP BY ");
        sql.push_str(&qualified_column(&source_alias, ordinal));
    }

    if spec.orders.is_empty() && spec.group_count.is_some() {
        sql.push_str("\nORDER BY COUNT(*) DESC");
    } else if !spec.orders.is_empty() {
        let orders = spec
            .orders
            .iter()
            .map(|order| {
                format!(
                    "{} {}",
                    qualified_column(&source_alias, order.ordinal),
                    order.direction.sql()
                )
            })
            .collect::<Vec<_>>();
        sql.push_str("\nORDER BY ");
        sql.push_str(&orders.join(", "));
    }

    Ok(sql)
}

fn validate_transform(column_count: usize, spec: &ResultTransform) -> Result<(), String> {
    let validate = |ordinal: usize| {
        if ordinal >= column_count {
            Err(format!(
                "result column {} is out of range for {column_count} columns",
                ordinal.saturating_add(1)
            ))
        } else {
            Ok(())
        }
    };
    for ordinal in &spec.projection {
        validate(*ordinal)?;
    }
    for filter in &spec.filters {
        validate(filter.ordinal)?;
    }
    for order in &spec.orders {
        validate(order.ordinal)?;
    }
    if let Some(group_ordinal) = spec.group_count {
        validate(group_ordinal)?;
        if spec
            .orders
            .iter()
            .any(|order| order.ordinal != group_ordinal)
        {
            return Err("grouped result can only be ordered by its grouped column".to_string());
        }
    }
    Ok(())
}

fn compile_filter(source_alias: &str, filter: &ResultFilter) -> Result<String, String> {
    let column = qualified_column(source_alias, filter.ordinal);
    match filter.op {
        FilterOp::IsNull => {
            if filter.value != FilterValue::Null {
                return Err("IS NULL does not accept a filter value".to_string());
            }
            Ok(format!("{column} IS NULL"))
        }
        FilterOp::IsNotNull => {
            if filter.value != FilterValue::Null {
                return Err("IS NOT NULL does not accept a filter value".to_string());
            }
            Ok(format!("{column} IS NOT NULL"))
        }
        FilterOp::Eq if filter.value == FilterValue::Null => Ok(format!("{column} IS NULL")),
        FilterOp::Ne if filter.value == FilterValue::Null => Ok(format!("{column} IS NOT NULL")),
        FilterOp::Contains
        | FilterOp::NotContains
        | FilterOp::StartsWith
        | FilterOp::NotStartsWith
        | FilterOp::EndsWith
        | FilterOp::NotEndsWith => {
            let FilterValue::Text(value) = &filter.value else {
                return Err("text-match filters require a text value".to_string());
            };
            let escaped = escape_like(value);
            let pattern = match filter.op {
                FilterOp::Contains | FilterOp::NotContains => format!("%{escaped}%"),
                FilterOp::StartsWith | FilterOp::NotStartsWith => format!("{escaped}%"),
                FilterOp::EndsWith | FilterOp::NotEndsWith => format!("%{escaped}"),
                _ => unreachable!("text-match operation was checked above"),
            };
            let operator = match filter.op {
                FilterOp::NotContains | FilterOp::NotStartsWith | FilterOp::NotEndsWith => {
                    "NOT ILIKE"
                }
                _ => "ILIKE",
            };
            Ok(format!(
                "{column}::text {operator} {} ESCAPE E'\\\\'",
                quote_literal(&pattern)?
            ))
        }
        FilterOp::Eq
        | FilterOp::Ne
        | FilterOp::Lt
        | FilterOp::Lte
        | FilterOp::Gt
        | FilterOp::Gte => {
            if filter.value == FilterValue::Null {
                return Err("NULL only supports equality or IS NULL filters".to_string());
            }
            let operator = match filter.op {
                FilterOp::Eq => "IS NOT DISTINCT FROM",
                FilterOp::Ne => "IS DISTINCT FROM",
                FilterOp::Lt => "<",
                FilterOp::Lte => "<=",
                FilterOp::Gt => ">",
                FilterOp::Gte => ">=",
                _ => unreachable!("comparison operation was checked above"),
            };
            Ok(format!(
                "{column} {operator} {}",
                compile_value(&filter.value)?
            ))
        }
    }
}

fn compile_value(value: &FilterValue) -> Result<String, String> {
    match value {
        FilterValue::Text(value) => quote_literal(value),
        FilterValue::Integer(value) => Ok(value.to_string()),
        FilterValue::Decimal(value) if is_finite_decimal(value) => Ok(value.clone()),
        FilterValue::Decimal(_) => Err("numeric filter value must be finite".to_string()),
        FilterValue::Boolean(value) => Ok(if *value { "TRUE" } else { "FALSE" }.to_string()),
        FilterValue::Null => Ok("NULL".to_string()),
    }
}

fn is_finite_decimal(value: &str) -> bool {
    let bytes = value.as_bytes();
    let mut index = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
    let integer_start = index;
    while bytes.get(index).is_some_and(u8::is_ascii_digit) {
        index += 1;
    }
    let has_integer = index > integer_start;

    let mut has_fraction = false;
    if bytes.get(index) == Some(&b'.') {
        index += 1;
        let fraction_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        has_fraction = index > fraction_start;
    }
    if !has_integer && !has_fraction {
        return false;
    }

    if matches!(bytes.get(index), Some(b'e' | b'E')) {
        index += 1;
        if matches!(bytes.get(index), Some(b'+' | b'-')) {
            index += 1;
        }
        let exponent_start = index;
        while bytes.get(index).is_some_and(u8::is_ascii_digit) {
            index += 1;
        }
        if index == exponent_start {
            return false;
        }
    }

    index == bytes.len()
}

fn column_alias(ordinal: usize) -> String {
    format!("__tsql_col_{}", ordinal.saturating_add(1))
}

fn qualified_column(source_alias: &str, ordinal: usize) -> String {
    format!(
        "{source_alias}.{}",
        quote_identifier(&column_alias(ordinal))
    )
}

fn quote_identifier(identifier: &str) -> String {
    format!("\"{}\"", identifier.replace('"', "\"\""))
}

fn quote_literal(value: &str) -> Result<String, String> {
    if value.contains('\0') {
        return Err("text filter value cannot contain a NUL byte".to_string());
    }
    Ok(format!(
        "E'{}'",
        value.replace('\\', "\\\\").replace('\'', "''")
    ))
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn headers() -> Vec<String> {
        ["id", "name", "name", "Total \"USD\""]
            .into_iter()
            .map(str::to_string)
            .collect()
    }

    #[test]
    fn wraps_read_only_source_and_uses_deterministic_ordinal_aliases() {
        let sql = compile_result_transform(
            "SELECT id, name, name, amount AS total FROM users; -- trailing",
            &headers(),
            &ResultTransform::default(),
        )
        .unwrap();

        assert!(sql.starts_with("SELECT \"__tsql_result\".\"__tsql_col_1\" AS \"id\""));
        assert!(sql.contains(
            "\"__tsql_col_2\" AS \"name\", \"__tsql_result\".\"__tsql_col_3\" AS \"name\""
        ));
        assert!(sql.contains("AS \"Total \"\"USD\"\"\""));
        assert!(sql.contains("AS \"__tsql_result\" (\"__tsql_col_1\", \"__tsql_col_2\", \"__tsql_col_3\", \"__tsql_col_4\")"));
        assert!(!sql.contains("; -- trailing"));
    }

    #[test]
    fn composes_projection_filters_and_multi_column_ordering() {
        let spec = ResultTransform {
            projection: vec![3, 1],
            filters: vec![
                ResultFilter {
                    ordinal: 0,
                    op: FilterOp::Gte,
                    value: FilterValue::Integer(10),
                },
                ResultFilter {
                    ordinal: 1,
                    op: FilterOp::Eq,
                    value: FilterValue::Text("O'Brien\\ops".to_string()),
                },
            ],
            orders: vec![
                ResultOrder {
                    ordinal: 3,
                    direction: OrderDirection::Desc,
                },
                ResultOrder {
                    ordinal: 0,
                    direction: OrderDirection::Asc,
                },
            ],
            group_count: None,
        };

        let sql =
            compile_result_transform("SELECT id, name, name, total FROM users", &headers(), &spec)
                .unwrap();

        assert!(sql.starts_with("SELECT \"__tsql_result\".\"__tsql_col_4\" AS \"Total \"\"USD\"\"\", \"__tsql_result\".\"__tsql_col_2\" AS \"name\""));
        assert!(sql.contains("WHERE \"__tsql_result\".\"__tsql_col_1\" >= 10\n  AND \"__tsql_result\".\"__tsql_col_2\" IS NOT DISTINCT FROM E'O''Brien\\\\ops'"));
        assert!(sql.ends_with("ORDER BY \"__tsql_result\".\"__tsql_col_4\" DESC, \"__tsql_result\".\"__tsql_col_1\" ASC"));
        assert!(
            sql_lexer::single_statement(&sql).is_ok(),
            "compiled query must remain a single cursor-safe statement: {sql}"
        );
    }

    #[test]
    fn escapes_like_wildcards_and_backslashes() {
        let spec = ResultTransform {
            filters: vec![ResultFilter {
                ordinal: 1,
                op: FilterOp::Contains,
                value: FilterValue::Text("100%_done\\ok".to_string()),
            }],
            ..ResultTransform::default()
        };

        let sql =
            compile_result_transform("SELECT id, name, name, total FROM users", &headers(), &spec)
                .unwrap();

        assert!(
            sql.contains("ILIKE E'%100\\\\%\\\\_done\\\\\\\\ok%' ESCAPE E'\\\\'"),
            "{sql}"
        );
    }

    #[test]
    fn compiles_null_and_typed_comparisons_without_conflating_text_null() {
        let spec = ResultTransform {
            filters: vec![
                ResultFilter {
                    ordinal: 0,
                    op: FilterOp::Eq,
                    value: FilterValue::Null,
                },
                ResultFilter {
                    ordinal: 1,
                    op: FilterOp::Ne,
                    value: FilterValue::Null,
                },
                ResultFilter {
                    ordinal: 2,
                    op: FilterOp::Eq,
                    value: FilterValue::Text("NULL".to_string()),
                },
                ResultFilter {
                    ordinal: 3,
                    op: FilterOp::Lt,
                    value: FilterValue::Decimal("12.5".to_string()),
                },
            ],
            ..ResultTransform::default()
        };

        let sql =
            compile_result_transform("SELECT id, name, name, total FROM users", &headers(), &spec)
                .unwrap();

        assert!(sql.contains("\"__tsql_col_1\" IS NULL"));
        assert!(sql.contains("\"__tsql_col_2\" IS NOT NULL"));
        assert!(sql.contains("\"__tsql_col_3\" IS NOT DISTINCT FROM E'NULL'"));
        assert!(sql.contains("\"__tsql_col_4\" < 12.5"));
    }

    #[test]
    fn compiles_group_count_after_filters_and_defaults_to_count_descending() {
        let spec = ResultTransform {
            filters: vec![ResultFilter {
                ordinal: 0,
                op: FilterOp::Gt,
                value: FilterValue::Integer(0),
            }],
            group_count: Some(1),
            ..ResultTransform::default()
        };

        let sql =
            compile_result_transform("SELECT id, name, name, total FROM users", &headers(), &spec)
                .unwrap();

        assert!(sql.starts_with(
            "SELECT \"__tsql_result\".\"__tsql_col_2\" AS \"name\", COUNT(*) AS \"count\""
        ));
        assert!(sql.contains("WHERE \"__tsql_result\".\"__tsql_col_1\" > 0"));
        assert!(sql.contains("GROUP BY \"__tsql_result\".\"__tsql_col_2\""));
        assert!(sql.ends_with("ORDER BY COUNT(*) DESC"));
    }

    #[test]
    fn rejects_unsafe_or_invalid_source_statements() {
        for source in [
            "",
            "SELECT 1; SELECT 2",
            "UPDATE users SET active = true RETURNING *",
            "WITH removed AS (DELETE FROM users RETURNING *) SELECT * FROM removed",
            "SELECT * FROM users FOR UPDATE",
            "SELECT * FROM users FOR SHARE",
            "SELECT $1::integer",
            "SELECT E'unterminated\\'",
        ] {
            assert!(
                compile_result_transform(source, &headers(), &ResultTransform::default()).is_err(),
                "source should be rejected: {source}"
            );
        }
    }

    #[test]
    fn accepts_valid_row_returning_expression_shapes() {
        for source in [
            "SELECT 1 AS value",
            "WITH source AS (SELECT 1 AS value) SELECT value FROM source",
            "VALUES (1), (2)",
            "TABLE users",
            "SELECT '; FOR UPDATE' AS value /* ignored; */",
            "SELECT foo$1 FROM (VALUES (1)) AS source(foo$1)",
        ] {
            assert!(
                compile_result_transform(
                    source,
                    &["value".to_string()],
                    &ResultTransform::default()
                )
                .is_ok(),
                "source should be accepted: {source}"
            );
        }
    }

    #[test]
    fn rejects_invalid_ordinals_group_order_and_filter_values() {
        let invalid_specs = [
            ResultTransform {
                projection: vec![4],
                ..ResultTransform::default()
            },
            ResultTransform {
                filters: vec![ResultFilter {
                    ordinal: 4,
                    op: FilterOp::Eq,
                    value: FilterValue::Integer(1),
                }],
                ..ResultTransform::default()
            },
            ResultTransform {
                orders: vec![ResultOrder {
                    ordinal: 4,
                    direction: OrderDirection::Asc,
                }],
                ..ResultTransform::default()
            },
            ResultTransform {
                group_count: Some(1),
                orders: vec![ResultOrder {
                    ordinal: 2,
                    direction: OrderDirection::Asc,
                }],
                ..ResultTransform::default()
            },
            ResultTransform {
                filters: vec![ResultFilter {
                    ordinal: 0,
                    op: FilterOp::Contains,
                    value: FilterValue::Integer(1),
                }],
                ..ResultTransform::default()
            },
            ResultTransform {
                filters: vec![ResultFilter {
                    ordinal: 0,
                    op: FilterOp::Lt,
                    value: FilterValue::Null,
                }],
                ..ResultTransform::default()
            },
            ResultTransform {
                filters: vec![ResultFilter {
                    ordinal: 0,
                    op: FilterOp::Eq,
                    value: FilterValue::Decimal("NaN".to_string()),
                }],
                ..ResultTransform::default()
            },
            ResultTransform {
                filters: vec![ResultFilter {
                    ordinal: 0,
                    op: FilterOp::Eq,
                    value: FilterValue::Text("bad\0value".to_string()),
                }],
                ..ResultTransform::default()
            },
        ];

        for spec in invalid_specs {
            assert!(
                compile_result_transform(
                    "SELECT id, name, name, total FROM users",
                    &headers(),
                    &spec
                )
                .is_err(),
                "invalid transform should be rejected: {spec:?}"
            );
        }
        assert!(compile_result_transform("SELECT 1", &[], &ResultTransform::default()).is_err());
    }

    #[test]
    fn mutators_reset_and_summary_keep_operations_composable() {
        let mut spec = ResultTransform::default();
        assert!(spec.is_empty());

        spec.set_projection(vec![0, 3]);
        spec.add_filter(ResultFilter {
            ordinal: 1,
            op: FilterOp::StartsWith,
            value: FilterValue::Text("Ada".to_string()),
        });
        spec.toggle_order(3);
        assert_eq!(spec.orders[0].direction, OrderDirection::Asc);
        spec.toggle_order(3);
        assert_eq!(spec.orders[0].direction, OrderDirection::Desc);
        spec.set_order(0, OrderDirection::Asc, true);
        spec.set_group_count(Some(1));

        assert_eq!(
            spec.summary(&headers()),
            "1 filter · sort Total \"USD\" desc, id asc · 2 columns · count by name"
        );
        spec.clear_filters();
        spec.clear_orders();
        assert!(spec.filters.is_empty());
        assert!(spec.orders.is_empty());
        spec.reset();
        assert!(spec.is_empty());
    }

    #[test]
    fn all_filter_operations_render_expected_predicates() {
        let cases = [
            (
                FilterOp::Eq,
                FilterValue::Integer(1),
                "IS NOT DISTINCT FROM 1",
            ),
            (
                FilterOp::Ne,
                FilterValue::Boolean(false),
                "IS DISTINCT FROM FALSE",
            ),
            (FilterOp::Lt, FilterValue::Integer(2), "< 2"),
            (FilterOp::Lte, FilterValue::Integer(3), "<= 3"),
            (FilterOp::Gt, FilterValue::Integer(4), "> 4"),
            (FilterOp::Gte, FilterValue::Integer(5), ">= 5"),
            (
                FilterOp::StartsWith,
                FilterValue::Text("a".to_string()),
                "ILIKE E'a%'",
            ),
            (
                FilterOp::NotContains,
                FilterValue::Text("x".to_string()),
                "NOT ILIKE E'%x%'",
            ),
            (
                FilterOp::NotStartsWith,
                FilterValue::Text("a".to_string()),
                "NOT ILIKE E'a%'",
            ),
            (
                FilterOp::EndsWith,
                FilterValue::Text("z".to_string()),
                "ILIKE E'%z'",
            ),
            (
                FilterOp::NotEndsWith,
                FilterValue::Text("z".to_string()),
                "NOT ILIKE E'%z'",
            ),
            (FilterOp::IsNull, FilterValue::Null, "IS NULL"),
            (FilterOp::IsNotNull, FilterValue::Null, "IS NOT NULL"),
        ];

        for (op, value, expected) in cases {
            let sql = compile_result_transform(
                "SELECT value FROM users",
                &["value".to_string()],
                &ResultTransform {
                    filters: vec![ResultFilter {
                        ordinal: 0,
                        op,
                        value,
                    }],
                    ..ResultTransform::default()
                },
            )
            .unwrap();
            assert!(sql.contains(expected), "expected {expected} in {sql}");
        }
    }

    #[test]
    fn parses_command_values_without_conflating_literal_null() {
        let cases = [
            ("42", FilterValue::Integer(42)),
            (" -9 ", FilterValue::Integer(-9)),
            ("12.5", FilterValue::Decimal("12.5".to_string())),
            ("2e3", FilterValue::Decimal("2e3".to_string())),
            (".125", FilterValue::Decimal(".125".to_string())),
            ("5.", FilterValue::Decimal("5.".to_string())),
            ("TRUE", FilterValue::Boolean(true)),
            ("false", FilterValue::Boolean(false)),
            ("NULL", FilterValue::Text("NULL".to_string())),
            ("null", FilterValue::Text("null".to_string())),
            ("'NULL'", FilterValue::Text("NULL".to_string())),
            ("'O''Brien'", FilterValue::Text("O'Brien".to_string())),
            (
                "\"say \"\"hi\"\"\"",
                FilterValue::Text("say \"hi\"".to_string()),
            ),
            ("plain words", FilterValue::Text("plain words".to_string())),
            ("NaN", FilterValue::Text("NaN".to_string())),
            ("Infinity", FilterValue::Text("Infinity".to_string())),
            ("1e+", FilterValue::Text("1e+".to_string())),
        ];

        for (input, expected) in cases {
            assert_eq!(parse_filter_value(input), expected, "input: {input}");
        }
    }

    #[test]
    fn preserves_high_precision_decimal_filter_values_exactly() {
        let decimal = "0.123456789012345678901234567890";
        let value = parse_filter_value(decimal);
        assert_eq!(value, FilterValue::Decimal(decimal.to_string()));

        let sql = compile_result_transform(
            "SELECT amount FROM users",
            &["amount".to_string()],
            &ResultTransform {
                filters: vec![ResultFilter {
                    ordinal: 0,
                    op: FilterOp::Eq,
                    value,
                }],
                ..ResultTransform::default()
            },
        )
        .unwrap();
        assert!(
            sql.contains(&format!("IS NOT DISTINCT FROM {decimal}")),
            "high-precision decimal was changed: {sql}"
        );
    }
}
