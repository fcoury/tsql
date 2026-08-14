//! Small PostgreSQL-aware lexer shared by notebook execution and refinement.

use std::ops::Range;

/// Lexical regions relevant to structural SQL inspection.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum SqlSegmentKind {
    Code,
    SingleQuoted,
    DoubleQuoted,
    DollarQuoted,
    LineComment,
    BlockComment,
}

/// One UTF-8-safe byte range in a SQL source string.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SqlSegment {
    pub(crate) kind: SqlSegmentKind,
    pub(crate) range: Range<usize>,
}

/// Splits SQL into structural code, quoted, and comment regions.
pub(crate) fn scan(source: &str) -> Result<Vec<SqlSegment>, String> {
    let bytes = source.as_bytes();
    let mut segments = Vec::new();
    let mut index = 0;
    let mut code_start = 0;

    while index < bytes.len() {
        let start = index;
        let kind = match bytes[index] {
            b'\'' | b'"' => {
                let delimiter = bytes[index];
                let escaped = delimiter == b'\'' && is_escape_string(source, index);
                index += 1;
                loop {
                    let Some(character) = source[index..].chars().next() else {
                        return Err("unterminated SQL literal".to_string());
                    };
                    if escaped && character == '\\' {
                        index += character.len_utf8();
                        let Some(next) = source[index..].chars().next() else {
                            return Err("unterminated SQL literal".to_string());
                        };
                        index += next.len_utf8();
                        continue;
                    }
                    index += character.len_utf8();
                    if character == char::from(delimiter) {
                        if bytes.get(index) == Some(&delimiter) {
                            index += 1;
                        } else {
                            break;
                        }
                    }
                }
                if delimiter == b'\'' {
                    SqlSegmentKind::SingleQuoted
                } else {
                    SqlSegmentKind::DoubleQuoted
                }
            }
            b'$' => {
                let Some((delimiter_end, delimiter)) = dollar_delimiter(source, index) else {
                    index += 1;
                    continue;
                };
                let content_start = delimiter_end + 1;
                let Some(close_offset) = source[content_start..].find(delimiter) else {
                    return Err("unterminated dollar-quoted SQL literal".to_string());
                };
                index = content_start + close_offset + delimiter.len();
                SqlSegmentKind::DollarQuoted
            }
            b'-' if bytes.get(index + 1) == Some(&b'-') => {
                index = source[index..]
                    .find(['\r', '\n'])
                    .map_or(bytes.len(), |offset| index + offset + 1);
                SqlSegmentKind::LineComment
            }
            b'/' if bytes.get(index + 1) == Some(&b'*') => {
                let mut depth = 1usize;
                index += 2;
                while depth > 0 {
                    if bytes.get(index..index + 2) == Some(b"/*") {
                        depth += 1;
                        index += 2;
                    } else if bytes.get(index..index + 2) == Some(b"*/") {
                        depth -= 1;
                        index += 2;
                    } else {
                        let Some(character) = source[index..].chars().next() else {
                            return Err("unterminated SQL comment".to_string());
                        };
                        index += character.len_utf8();
                    }
                }
                SqlSegmentKind::BlockComment
            }
            _ => {
                let Some(character) = source[index..].chars().next() else {
                    return Err("invalid UTF-8 boundary while scanning SQL".to_string());
                };
                index += character.len_utf8();
                continue;
            }
        };

        if code_start < start {
            segments.push(SqlSegment {
                kind: SqlSegmentKind::Code,
                range: code_start..start,
            });
        }
        segments.push(SqlSegment {
            kind,
            range: start..index,
        });
        code_start = index;
    }

    if code_start < source.len() {
        segments.push(SqlSegment {
            kind: SqlSegmentKind::Code,
            range: code_start..source.len(),
        });
    }
    Ok(segments)
}

/// Returns one statement without its terminal semicolon or trailing comments.
pub(crate) fn single_statement(source: &str) -> Result<&str, String> {
    let source = source.trim();
    if source.is_empty() {
        return Err("empty SQL statement".to_string());
    }
    let segments = scan(source)?;
    let mut terminal_semicolon = None;

    for segment in segments {
        match segment.kind {
            SqlSegmentKind::Code => {
                for (offset, character) in source[segment.range.clone()].char_indices() {
                    if character == ';' {
                        if terminal_semicolon.is_some() {
                            return Err("multiple SQL statements are not supported".to_string());
                        }
                        terminal_semicolon = Some(segment.range.start + offset);
                    } else if terminal_semicolon.is_some() && !character.is_whitespace() {
                        return Err("multiple SQL statements are not supported".to_string());
                    }
                }
            }
            SqlSegmentKind::LineComment | SqlSegmentKind::BlockComment => {}
            _ if terminal_semicolon.is_some() => {
                return Err("multiple SQL statements are not supported".to_string());
            }
            _ => {}
        }
    }

    Ok(terminal_semicolon.map_or(source, |index| source[..index].trim_end()))
}

/// Replaces comments with whitespace while preserving source byte offsets and newlines.
pub(crate) fn mask_comments(source: &str) -> Result<String, String> {
    let mut masked = String::with_capacity(source.len());
    for segment in scan(source)? {
        if matches!(
            segment.kind,
            SqlSegmentKind::LineComment | SqlSegmentKind::BlockComment
        ) {
            for character in source[segment.range].chars() {
                if matches!(character, '\r' | '\n') {
                    masked.push(character);
                } else {
                    for _ in 0..character.len_utf8() {
                        masked.push(' ');
                    }
                }
            }
        } else {
            masked.push_str(&source[segment.range]);
        }
    }
    Ok(masked)
}

/// Returns uppercase identifier-like words that occur outside literals and comments.
pub(crate) fn code_words(source: &str, limit: usize) -> Result<Vec<String>, String> {
    let mut words = Vec::new();
    for segment in scan(source)? {
        if segment.kind != SqlSegmentKind::Code {
            continue;
        }
        let remaining = limit.saturating_sub(words.len());
        words.extend(
            source[segment.range]
                .split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
                .filter(|word| !word.is_empty())
                .take(remaining)
                .map(str::to_ascii_uppercase),
        );
        if words.len() == limit {
            break;
        }
    }
    Ok(words)
}

/// Returns uppercase identifier-like words outside literals and comments,
/// together with their parenthesis depth.
pub(crate) fn code_words_with_depth(source: &str) -> Result<Vec<(String, usize)>, String> {
    fn flush_word(word: &mut String, depth: usize, words: &mut Vec<(String, usize)>) {
        if !word.is_empty() {
            words.push((std::mem::take(word).to_ascii_uppercase(), depth));
        }
    }

    let mut words = Vec::new();
    let mut word = String::new();
    let mut depth = 0usize;

    for segment in scan(source)? {
        if segment.kind != SqlSegmentKind::Code {
            flush_word(&mut word, depth, &mut words);
            continue;
        }
        for character in source[segment.range].chars() {
            if character.is_ascii_alphanumeric() || character == '_' {
                word.push(character);
                continue;
            }
            flush_word(&mut word, depth, &mut words);
            match character {
                '(' => depth = depth.saturating_add(1),
                ')' => depth = depth.saturating_sub(1),
                _ => {}
            }
        }
        flush_word(&mut word, depth, &mut words);
    }

    Ok(words)
}

fn is_escape_string(source: &str, quote: usize) -> bool {
    let bytes = source.as_bytes();
    if quote > 0 && matches!(bytes[quote - 1], b'E' | b'e') {
        return quote == 1 || !is_identifier_byte(bytes[quote - 2]);
    }
    quote >= 2
        && matches!(bytes[quote - 2], b'U' | b'u')
        && bytes[quote - 1] == b'&'
        && (quote == 2 || !is_identifier_byte(bytes[quote - 3]))
}

fn dollar_delimiter(source: &str, start: usize) -> Option<(usize, &str)> {
    let end = start + 1 + source[start + 1..].find('$')?;
    let tag = &source[start + 1..end];
    let mut characters = tag.chars();
    let valid = characters.next().is_none_or(|character| {
        (character.is_ascii_alphabetic() || character == '_')
            && characters.all(|character| character.is_ascii_alphanumeric() || character == '_')
    });
    valid.then_some((end, &source[start..=end]))
}

fn is_identifier_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'$')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_escape_strings_and_preserves_structural_boundaries() {
        let source =
            "SELECT E'it\\'s @result_1; -- not a comment', U&'one\\0027two', 'plain;value';";
        let segments = scan(source).unwrap();

        assert_eq!(
            segments
                .iter()
                .map(|segment| segment.kind)
                .collect::<Vec<_>>(),
            [
                SqlSegmentKind::Code,
                SqlSegmentKind::SingleQuoted,
                SqlSegmentKind::Code,
                SqlSegmentKind::SingleQuoted,
                SqlSegmentKind::Code,
                SqlSegmentKind::SingleQuoted,
                SqlSegmentKind::Code,
            ]
        );
        assert_eq!(
            single_statement(source).unwrap(),
            source.trim_end_matches(';')
        );
    }

    #[test]
    fn skips_unicode_dollar_quotes_and_nested_comments() {
        let source =
            "/* café /* nested; */ 🙂 */ SELECT $body$one; @result_2$body$ AS value -- naïve;\n;";
        let statement = single_statement(source).unwrap();
        assert!(statement.ends_with("-- naïve;"));
        assert_eq!(code_words(statement, 4).unwrap(), ["SELECT", "AS", "VALUE"]);
        let masked = mask_comments(statement).unwrap();
        assert_eq!(masked.len(), statement.len());
        assert!(masked.contains("SELECT $body$one; @result_2$body$"));
        assert!(!masked.contains("nested"));
    }

    #[test]
    fn reports_parenthesis_depth_for_structural_words() {
        assert_eq!(
            code_words_with_depth(
                "UPDATE users SET active = EXISTS (SELECT 1 FROM audit WHERE audit.id = users.id)"
            )
            .unwrap(),
            [
                ("UPDATE".to_string(), 0),
                ("USERS".to_string(), 0),
                ("SET".to_string(), 0),
                ("ACTIVE".to_string(), 0),
                ("EXISTS".to_string(), 0),
                ("SELECT".to_string(), 1),
                ("1".to_string(), 1),
                ("FROM".to_string(), 1),
                ("AUDIT".to_string(), 1),
                ("WHERE".to_string(), 1),
                ("AUDIT".to_string(), 1),
                ("ID".to_string(), 1),
                ("USERS".to_string(), 1),
                ("ID".to_string(), 1),
            ]
        );
    }

    #[test]
    fn rejects_multiple_and_unterminated_statements() {
        for source in [
            "SELECT 1; SELECT 2",
            "SELECT $$one;two$$; SELECT 2",
            "SELECT 1 /* ignored; */; SELECT 2",
            "SELECT 'unterminated",
            "SELECT E'unterminated\\'",
            "SELECT $tag$unterminated",
            "SELECT /* unterminated",
        ] {
            assert!(single_statement(source).is_err(), "accepted: {source}");
        }
    }
}
