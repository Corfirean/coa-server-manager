//! The typed SQL encoder: the **only** way data enters an import script.
//!
//! An untrusted string or blob (names, item texts, pet names, settings, macro blobs, ...) can only be a
//! [`Val::Text`] / [`Val::Bytes`], and those are rendered **exclusively as hex literals**: `_utf8mb4 0x48656C6C6F` /
//! `0xDEADBEEF`. Nothing is ever escaped, quoted or interpolated, so there is nothing to get wrong: no quote, backslash,
//! NUL, newline or `;` of any input can reach the SQL text. Numbers are rendered from typed integers. The only
//! free-form SQL is [`Val::Expr`], which takes a `&'static str`: it can only be a literal in this crate's source.
//!
//! Table and column names are `&'static str` for the same reason.

use super::super::error::{PortableError, Result};

/// A value of one column.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Val {
    Null,
    Uint(u64),
    Int(i64),
    /// Character data, always rendered as `_utf8mb4 0x<hex>`.
    Text(String),
    /// Binary data, always rendered as `0x<hex>`.
    Bytes(Vec<u8>),
    /// The item at this index of the import plan (`@item_base + index`).
    ItemRef(u32),
    /// The pet at this index of the import plan (`@pet_base + index`).
    PetRef(u32),
    /// Fixed SQL written in this crate's source (a session variable such as `@char`, or `UNIX_TIMESTAMP()`).
    Expr(&'static str),
}

impl Val {
    pub fn u<T: Into<u64>>(v: T) -> Val {
        Val::Uint(v.into())
    }
    pub fn i<T: Into<i64>>(v: T) -> Val {
        Val::Int(v.into())
    }
    pub fn text(s: impl Into<String>) -> Val {
        Val::Text(s.into())
    }
    pub fn opt_text(s: Option<&str>) -> Val {
        s.map_or(Val::Null, |s| Val::Text(s.to_string()))
    }

    pub fn sql(&self) -> String {
        match self {
            Val::Null => "NULL".into(),
            Val::Uint(v) => v.to_string(),
            Val::Int(v) => v.to_string(),
            Val::Text(s) if s.is_empty() => "_utf8mb4 X''".into(),
            Val::Text(s) => format!("_utf8mb4 0x{}", hex::encode(s.as_bytes())),
            Val::Bytes(b) if b.is_empty() => "X''".into(),
            Val::Bytes(b) => format!("0x{}", hex::encode(b)),
            Val::ItemRef(i) => format!("(@item_base + {i})"),
            Val::PetRef(i) => format!("(@pet_base + {i})"),
            Val::Expr(e) => (*e).to_string(),
        }
    }
}

/// One `INSERT` with a fixed table and fixed columns.
#[derive(Clone, Debug)]
pub struct Insert {
    table: &'static str,
    columns: &'static [&'static str],
    rows: Vec<Vec<Val>>,
}

impl Insert {
    pub fn new(table: &'static str, columns: &'static [&'static str]) -> Self {
        Self { table, columns, rows: Vec::new() }
    }

    /// A row must have exactly one value per column.
    pub fn row(&mut self, values: Vec<Val>) -> Result<()> {
        if values.len() != self.columns.len() {
            return Err(PortableError::Invalid(format!("internal: {} values for {} columns of {}", values.len(), self.columns.len(), self.table)));
        }
        self.rows.push(values);
        Ok(())
    }

    pub fn table(&self) -> &'static str {
        self.table
    }
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    /// `None` when there is nothing to insert.
    pub fn sql(&self) -> Option<String> {
        if self.rows.is_empty() {
            return None;
        }
        let columns = self.columns.iter().map(|c| format!("`{c}`")).collect::<Vec<_>>().join(", ");
        let rows = self.rows.iter().map(|r| format!("({})", r.iter().map(Val::sql).collect::<Vec<_>>().join(", "))).collect::<Vec<_>>().join(",\n  ");
        Some(format!("INSERT INTO acore_characters.`{}` ({columns}) VALUES\n  {rows};", self.table))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Characters that must never appear in generated SQL because of untrusted data.
    const HOSTILE: &[char] = &['\'', '"', '\\', '\0', '\n', '\r', '\t', ';', '`', '-', '#', '/', '*', '%', ' '];

    #[test]
    fn text_is_a_hex_literal_whatever_it_contains() {
        for s in [
            "plain",
            "O'Brien",
            "back\\slash",
            "\"; DROP TABLE characters; --",
            "'); DELETE FROM item_instance; --",
            "line1\nline2\r\n\ttabbed",
            "nul\0inside",
            "emoji \u{1F409} and \u{4e2d}\u{6587} and \u{0414}",
            "/* comment */ # hash",
            "%_wildcards",
            "",
        ] {
            let sql = Val::text(s).sql();
            let literal = sql.strip_prefix("_utf8mb4 ").unwrap();
            if s.is_empty() {
                assert_eq!(literal, "X''");
                continue;
            }
            let hex = literal.strip_prefix("0x").expect("a hex literal");
            assert!(hex.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')), "{sql}");
            assert_eq!(hex::decode(hex).unwrap(), s.as_bytes(), "the hex decodes to exactly the input");
            assert!(!literal.contains(HOSTILE), "no hostile character may reach the SQL for {s:?}: {sql}");
        }
    }

    #[test]
    fn bytes_are_a_hex_literal_too() {
        assert_eq!(Val::Bytes(vec![]).sql(), "X''");
        assert_eq!(Val::Bytes(vec![0, 255, 39, 92]).sql(), "0x00ff275c");
        let all: Vec<u8> = (0..=255).collect();
        let sql = Val::Bytes(all.clone()).sql();
        assert!(sql[2..].chars().all(|c| c.is_ascii_hexdigit()));
        assert_eq!(hex::decode(&sql[2..]).unwrap(), all);
    }

    #[test]
    fn numbers_come_from_typed_integers() {
        assert_eq!(Val::u(7u8).sql(), "7");
        assert_eq!(Val::u(u64::MAX).sql(), "18446744073709551615");
        assert_eq!(Val::i(-57i32).sql(), "-57");
        assert_eq!(Val::i(i64::MIN).sql(), "-9223372036854775808");
        assert_eq!(Val::Null.sql(), "NULL");
        assert_eq!(Val::ItemRef(3).sql(), "(@item_base + 3)");
        assert_eq!(Val::PetRef(0).sql(), "(@pet_base + 0)");
        assert_eq!(Val::Expr("@char").sql(), "@char");
        assert_eq!(Val::opt_text(None).sql(), "NULL");
    }

    #[test]
    fn an_insert_is_fixed_columns_plus_encoded_values() {
        let mut insert = Insert::new("character_settings", &["guid", "source", "data"]);
        assert!(insert.sql().is_none(), "no rows, no statement");
        insert.row(vec![Val::Expr("@char"), Val::text("it's \"a\" source"), Val::text("1 2 3 ")]).unwrap();
        insert.row(vec![Val::Expr("@char"), Val::text("x"), Val::Null]).unwrap();
        let sql = insert.sql().unwrap();
        assert!(sql.starts_with("INSERT INTO acore_characters.`character_settings` (`guid`, `source`, `data`) VALUES\n  (@char, _utf8mb4 0x"));
        assert!(!sql.contains("it's") && !sql.contains("\"a\""), "{sql}");
        assert_eq!(sql.matches('\'').count(), 0, "the generated INSERT contains no quote at all");
        assert!(sql.ends_with(';'));
        assert!(insert.row(vec![Val::Null]).is_err(), "arity is checked");
    }

    #[test]
    fn the_whole_alphabet_of_a_generated_insert_is_small() {
        // a hostile value produces nothing but hex digits, hex-literal syntax and the fixed scaffold
        let mut insert = Insert::new("characters", &["name"]);
        insert.row(vec![Val::text("Robert'); DROP TABLE characters;--\\\0\n")]).unwrap();
        let sql = insert.sql().unwrap();
        let allowed = |c: char| c.is_ascii_alphanumeric() || " _`(),.\n;".contains(c);
        assert!(sql.chars().all(allowed), "{sql}");
    }
}
