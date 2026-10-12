//! SQL syntax tokens for the SQL editor's highlighting.

use std::ops::Range;

const KEYWORDS: &[&str] = &[
    "SELECT",
    "DISTINCT",
    "FROM",
    "WHERE",
    "AND",
    "OR",
    "NOT",
    "IN",
    "BETWEEN",
    "LIKE",
    "ILIKE",
    "IS",
    "NULL",
    "GROUP",
    "BY",
    "HAVING",
    "ORDER",
    "ASC",
    "DESC",
    "LIMIT",
    "OFFSET",
    "JOIN",
    "INNER",
    "LEFT",
    "OUTER",
    "ON",
    "AS",
    "COUNT",
    "SUM",
    "AVG",
    "MIN",
    "MAX",
    "CASE",
    "WHEN",
    "THEN",
    "ELSE",
    "END",
    "TRUE",
    "FALSE",
    "DATE",
    "TIMESTAMP",
];

/// What a piece of SQL text is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SqlToken {
    Keyword,
    /// A `'string'` literal.
    String,
    /// A `"quoted identifier"`.
    QuotedName,
    Number,
    /// A `-- comment` to the end of the line.
    Comment,
    /// Operators, commas and brackets.
    Punctuation,
}

/// The tokens of one line of SQL as byte ranges; text between them (names, spaces) is
/// plain. An unterminated string or quoted name runs to the end of the line.
pub fn tokenize_line(line: &str) -> Vec<(Range<usize>, SqlToken)> {
    let bytes = line.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        let c = bytes[i];
        let kind = if c == b'-' && bytes.get(i + 1) == Some(&b'-') {
            i = bytes.len();
            Some(SqlToken::Comment)
        } else if c == b'\'' || c == b'"' {
            i += 1;
            while i < bytes.len() && bytes[i] != c {
                i += 1;
            }
            i = (i + 1).min(bytes.len());
            Some(if c == b'\'' {
                SqlToken::String
            } else {
                SqlToken::QuotedName
            })
        } else if c.is_ascii_digit() {
            while i < bytes.len() && (bytes[i].is_ascii_digit() || bytes[i] == b'.') {
                i += 1;
            }
            Some(SqlToken::Number)
        } else if c.is_ascii_alphabetic() || c == b'_' {
            while i < bytes.len()
                && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_' || bytes[i] == b'.')
            {
                i += 1;
            }
            let word = line[start..i].to_ascii_uppercase();
            KEYWORDS
                .contains(&word.as_str())
                .then_some(SqlToken::Keyword)
        } else {
            i += line[i..].chars().next().map_or(1, char::len_utf8);
            (!line[start..i].trim().is_empty()).then_some(SqlToken::Punctuation)
        };
        if let Some(kind) = kind {
            tokens.push((start..i, kind));
        }
    }
    tokens
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(line: &str) -> Vec<(&str, SqlToken)> {
        tokenize_line(line)
            .into_iter()
            .map(|(range, kind)| (&line[range], kind))
            .collect()
    }

    #[test]
    fn colors_keywords_literals_and_comments() {
        assert_eq!(
            kinds("select a.b, 'x y' from t -- note"),
            [
                ("select", SqlToken::Keyword),
                (",", SqlToken::Punctuation),
                ("'x y'", SqlToken::String),
                ("from", SqlToken::Keyword),
                ("-- note", SqlToken::Comment),
            ]
        );
        assert_eq!(
            kinds("WHERE \"my col\" >= 1.5"),
            [
                ("WHERE", SqlToken::Keyword),
                ("\"my col\"", SqlToken::QuotedName),
                (">", SqlToken::Punctuation),
                ("=", SqlToken::Punctuation),
                ("1.5", SqlToken::Number),
            ]
        );
    }

    #[test]
    fn unterminated_literals_run_to_the_line_end_and_unicode_is_safe() {
        assert_eq!(
            kinds("x = 'abc"),
            [("=", SqlToken::Punctuation), ("'abc", SqlToken::String)]
        );
        assert_eq!(kinds("é → 1").len(), 3);
        assert!(tokenize_line("").is_empty());
    }
}
