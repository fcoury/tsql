#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompletionKind {
    Keyword,
    Table,
    Column,
    #[allow(dead_code)]
    Schema,
    #[allow(dead_code)]
    Function,
}

#[derive(Clone, Debug)]
pub struct CompletionItem {
    pub label: String,
    pub kind: CompletionKind,
    #[allow(dead_code)]
    pub detail: Option<String>, // e.g., table name for columns, return type for functions
}

impl CompletionItem {
    pub fn keyword(s: &str) -> Self {
        Self {
            label: s.to_uppercase(),
            kind: CompletionKind::Keyword,
            detail: None,
        }
    }

    pub fn table(name: String, schema: Option<String>) -> Self {
        Self {
            label: name,
            kind: CompletionKind::Table,
            detail: schema,
        }
    }

    pub fn column(name: String, table: Option<String>) -> Self {
        Self {
            label: name,
            kind: CompletionKind::Column,
            detail: table,
        }
    }
}

pub struct CompletionPopup {
    pub active: bool,
    items: Vec<CompletionItem>,
    filtered: Vec<usize>, // indices into items that match the current filter
    pub selected: usize,  // index into filtered
    pub prefix: String,   // the word prefix being completed
    pub start_col: usize, // column position where the prefix starts
}

impl CompletionPopup {
    pub fn new() -> Self {
        Self {
            active: false,
            items: Vec::new(),
            filtered: Vec::new(),
            selected: 0,
            prefix: String::new(),
            start_col: 0,
        }
    }

    pub fn open(&mut self, items: Vec<CompletionItem>, prefix: String, start_col: usize) {
        self.items = items;
        self.prefix = prefix.to_lowercase();
        self.start_col = start_col;
        self.filter();
        self.selected = 0;
        self.active = !self.filtered.is_empty();
    }

    pub fn close(&mut self) {
        self.active = false;
        self.items.clear();
        self.filtered.clear();
        self.selected = 0;
        self.prefix.clear();
    }

    fn filter(&mut self) {
        self.filtered = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, item)| item.label.to_lowercase().starts_with(&self.prefix))
            .map(|(i, _)| i)
            .collect();
    }

    pub fn update_prefix(&mut self, prefix: String) {
        self.prefix = prefix.to_lowercase();
        self.filter();
        self.selected = 0;
        if self.filtered.is_empty() {
            self.active = false;
        }
    }

    pub fn select_next(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = (self.selected + 1) % self.filtered.len();
        }
    }

    pub fn select_prev(&mut self) {
        if !self.filtered.is_empty() {
            self.selected = self
                .selected
                .checked_sub(1)
                .unwrap_or(self.filtered.len() - 1);
        }
    }

    pub fn selected_item(&self) -> Option<&CompletionItem> {
        self.filtered
            .get(self.selected)
            .and_then(|&i| self.items.get(i))
    }

    pub fn visible_items(&self, max_items: usize) -> Vec<(usize, &CompletionItem)> {
        // Return items around the selected one
        let total = self.filtered.len();
        if total == 0 {
            return Vec::new();
        }

        let start = if total <= max_items || self.selected < max_items / 2 {
            0
        } else if self.selected > total - max_items / 2 {
            total - max_items
        } else {
            self.selected - max_items / 2
        };

        let end = (start + max_items).min(total);

        (start..end)
            .filter_map(|i| {
                self.filtered
                    .get(i)
                    .and_then(|&idx| self.items.get(idx).map(|item| (i, item)))
            })
            .collect()
    }
}

impl Default for CompletionPopup {
    fn default() -> Self {
        Self::new()
    }
}

/// Schema information cached from the database
#[derive(Default)]
pub struct SchemaCache {
    pub tables: Vec<TableInfo>,
    pub loaded: bool,
}

#[derive(Clone)]
pub struct TableInfo {
    pub schema: String,
    pub name: String,
    pub columns: Vec<ColumnInfo>,
}

#[derive(Clone)]
pub struct ColumnInfo {
    pub name: String,
    #[allow(dead_code)]
    pub data_type: String,
}

impl SchemaCache {
    pub fn new() -> Self {
        Self {
            tables: Vec::new(),
            loaded: false,
        }
    }

    pub fn get_completion_items(&self, context: CompletionContext) -> Vec<CompletionItem> {
        let mut items = Vec::new();

        match context {
            CompletionContext::General => {
                // Keywords + tables
                items.extend(sql_keywords().into_iter().map(CompletionItem::keyword));
                for table in &self.tables {
                    items.push(CompletionItem::table(
                        table.name.clone(),
                        Some(table.schema.clone()),
                    ));
                }
            }
            CompletionContext::AfterFrom | CompletionContext::AfterJoin => {
                // Tables only
                for table in &self.tables {
                    items.push(CompletionItem::table(
                        table.name.clone(),
                        Some(table.schema.clone()),
                    ));
                }
            }
            CompletionContext::AfterSelect | CompletionContext::AfterWhere => {
                // Columns from all tables + keywords
                for table in &self.tables {
                    for col in &table.columns {
                        items.push(CompletionItem::column(
                            col.name.clone(),
                            Some(table.name.clone()),
                        ));
                    }
                }
                items.extend(sql_keywords().into_iter().map(CompletionItem::keyword));
            }
        }

        items
    }
}

#[derive(Clone, Copy, Debug)]
pub enum CompletionContext {
    General,
    AfterSelect,
    AfterFrom,
    AfterJoin,
    AfterWhere,
}

pub fn sql_keywords() -> Vec<&'static str> {
    vec![
        // DML
        "SELECT",
        "FROM",
        "WHERE",
        "AND",
        "OR",
        "NOT",
        "IN",
        "EXISTS",
        "BETWEEN",
        "LIKE",
        "ILIKE",
        "IS",
        "NULL",
        "TRUE",
        "FALSE",
        "ORDER",
        "BY",
        "ASC",
        "DESC",
        "NULLS",
        "FIRST",
        "LAST",
        "LIMIT",
        "OFFSET",
        "FETCH",
        "NEXT",
        "ROWS",
        "ONLY",
        "GROUP",
        "HAVING",
        "DISTINCT",
        "ALL",
        "AS",
        "JOIN",
        "INNER",
        "LEFT",
        "RIGHT",
        "FULL",
        "OUTER",
        "CROSS",
        "ON",
        "USING",
        "UNION",
        "INTERSECT",
        "EXCEPT",
        "INSERT",
        "INTO",
        "VALUES",
        "DEFAULT",
        "RETURNING",
        "UPDATE",
        "SET",
        "DELETE",
        // DDL
        "CREATE",
        "ALTER",
        "DROP",
        "TRUNCATE",
        "TABLE",
        "INDEX",
        "VIEW",
        "SCHEMA",
        "DATABASE",
        "SEQUENCE",
        "PRIMARY",
        "KEY",
        "FOREIGN",
        "REFERENCES",
        "UNIQUE",
        "CHECK",
        "CONSTRAINT",
        "CASCADE",
        "RESTRICT",
        // Types
        "INTEGER",
        "INT",
        "BIGINT",
        "SMALLINT",
        "SERIAL",
        "BIGSERIAL",
        "TEXT",
        "VARCHAR",
        "CHAR",
        "CHARACTER",
        "BOOLEAN",
        "BOOL",
        "TIMESTAMP",
        "TIMESTAMPTZ",
        "DATE",
        "TIME",
        "INTERVAL",
        "NUMERIC",
        "DECIMAL",
        "REAL",
        "DOUBLE",
        "PRECISION",
        "FLOAT",
        "JSON",
        "JSONB",
        "UUID",
        "BYTEA",
        "ARRAY",
        // Functions
        "COUNT",
        "SUM",
        "AVG",
        "MIN",
        "MAX",
        "COALESCE",
        "NULLIF",
        "GREATEST",
        "LEAST",
        "CASE",
        "WHEN",
        "THEN",
        "ELSE",
        "END",
        "CAST",
        "EXTRACT",
        "NOW",
        "CURRENT_TIMESTAMP",
        "CURRENT_DATE",
        "LOWER",
        "UPPER",
        "TRIM",
        "SUBSTRING",
        "LENGTH",
        "CONCAT",
        // Transaction
        "BEGIN",
        "COMMIT",
        "ROLLBACK",
        "SAVEPOINT",
        // Other
        "EXPLAIN",
        "ANALYZE",
        "VERBOSE",
        "WITH",
        "RECURSIVE",
    ]
}

/// Get the word being typed before the cursor position
pub fn get_word_before_cursor(line: &str, col: usize) -> (String, usize) {
    let before: String = line.chars().take(col).collect();

    // Find the start of the current word (alphanumeric + underscore)
    let start = before
        .char_indices()
        .rev()
        .find(|(_, c)| !c.is_alphanumeric() && *c != '_')
        .map(|(i, _)| i + 1)
        .unwrap_or(0);

    let prefix: String = before.chars().skip(start).collect();
    (prefix, start)
}

/// Determine completion context from the text before cursor
pub fn determine_context(text: &str, cursor_col: usize) -> CompletionContext {
    // Get text before cursor on current line
    let before_cursor: String = text.chars().take(cursor_col).collect();
    let upper = before_cursor.to_uppercase();
    let tokens: Vec<&str> = upper.split_whitespace().collect();

    // Look at last few tokens to determine context
    if let Some(&last) = tokens.last() {
        match last {
            "SELECT" | "," => return CompletionContext::AfterSelect,
            "FROM" | "JOIN" => return CompletionContext::AfterFrom,
            "INNER" | "LEFT" | "RIGHT" | "FULL" | "CROSS" | "OUTER" => {
                return CompletionContext::AfterJoin
            }
            "WHERE" | "AND" | "OR" => return CompletionContext::AfterWhere,
            _ => {}
        }
    }

    // Check second-to-last for multi-word contexts
    if tokens.len() >= 2 {
        let second_last = tokens[tokens.len() - 2];
        match second_last {
            "ORDER" | "GROUP" => return CompletionContext::General,
            "LEFT" | "RIGHT" | "FULL" | "INNER" | "CROSS" => {
                if tokens.last() == Some(&"JOIN") {
                    return CompletionContext::AfterFrom;
                }
            }
            _ => {}
        }
    }

    CompletionContext::General
}
