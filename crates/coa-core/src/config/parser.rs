//! Line-preserving reader/writer for AzerothCore `.conf` files.
//!
//! Invariant: `parse(text).to_text() == text` for any valid UTF-8 input. Every line keeps its own line ending,
//! comments/blank lines/unknown keys are never touched, and `set` rewrites only the value part of one line.

use crate::error::{Error, Result};

#[derive(Debug, Clone)]
enum Kind {
    Other,
    Entry { key: String, prefix_len: usize },
}

#[derive(Debug, Clone)]
struct Line {
    /// Text without its line terminator.
    text: String,
    /// The terminator this line had ("\n", "\r\n" or "" for an unterminated last line).
    eol: String,
    kind: Kind,
}

#[derive(Debug, Clone)]
pub struct ConfFile {
    lines: Vec<Line>,
    bom: bool,
    default_eol: &'static str,
}

fn is_key_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-')
}

fn classify(text: &str) -> Kind {
    let t = text.trim_start();
    if t.is_empty() || t.starts_with('#') || t.starts_with(';') || t.starts_with('[') {
        return Kind::Other;
    }
    let Some(eq) = text.find('=') else {
        return Kind::Other;
    };
    let key = text[..eq].trim();
    if key.is_empty() || !key.chars().all(is_key_char) {
        return Kind::Other;
    }
    // prefix = everything up to and including '=' and the spaces after it
    let after = &text[eq + 1..];
    let spaces = after.len() - after.trim_start_matches([' ', '\t']).len();
    Kind::Entry {
        key: key.to_string(),
        prefix_len: eq + 1 + spaces,
    }
}

impl ConfFile {
    pub fn parse(input: &str) -> ConfFile {
        let (bom, body) = match input.strip_prefix('\u{feff}') {
            Some(rest) => (true, rest),
            None => (false, input),
        };
        let mut lines = Vec::new();
        let (mut crlf, mut lf) = (0usize, 0usize);
        let mut rest = body;
        while !rest.is_empty() {
            let (raw, eol, next) = match rest.find('\n') {
                Some(i) if i > 0 && rest.as_bytes()[i - 1] == b'\r' => {
                    (&rest[..i - 1], "\r\n", &rest[i + 1..])
                }
                Some(i) => (&rest[..i], "\n", &rest[i + 1..]),
                None => (rest, "", ""),
            };
            match eol {
                "\r\n" => crlf += 1,
                "\n" => lf += 1,
                _ => {}
            }
            lines.push(Line {
                text: raw.to_string(),
                eol: eol.to_string(),
                kind: classify(raw),
            });
            rest = next;
        }
        ConfFile {
            lines,
            bom,
            default_eol: if crlf > lf { "\r\n" } else { "\n" },
        }
    }

    pub fn parse_bytes(bytes: &[u8]) -> Result<ConfFile> {
        let text = std::str::from_utf8(bytes)
            .map_err(|_| Error::Invalid("configuration file is not valid UTF-8".into()))?;
        Ok(Self::parse(text))
    }

    pub fn to_text(&self) -> String {
        let mut out = String::new();
        if self.bom {
            out.push('\u{feff}');
        }
        for l in &self.lines {
            out.push_str(&l.text);
            out.push_str(&l.eol);
        }
        out
    }

    /// Active (uncommented) entries in file order.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.lines.iter().filter_map(|l| match &l.kind {
            Kind::Entry { key, prefix_len } => {
                Some((key.as_str(), l.text[*prefix_len..].trim_end()))
            }
            Kind::Other => None,
        })
    }

    /// Raw value (quotes included) of the last active occurrence of `key` (the one the server ends up using).
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries()
            .filter(|(k, _)| *k == key)
            .map(|(_, v)| v)
            .last()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.get(key).is_some()
    }

    /// Set `raw` (already formatted, quotes included) as the value of `key`. Returns true if the key was appended.
    /// `doc` lines (without `#`) are written above a newly appended key.
    pub fn set(&mut self, key: &str, raw: &str, doc: &[&str]) -> bool {
        if let Some(l) = self
            .lines
            .iter_mut()
            .rev()
            .find(|l| matches!(&l.kind, Kind::Entry { key: k, .. } if k == key))
        {
            if let Kind::Entry { prefix_len, .. } = l.kind {
                let prefix = l.text[..prefix_len].to_string();
                l.text = format!("{prefix}{raw}");
            }
            return false;
        }
        let eol = self.default_eol;
        // Make sure the previous line is terminated before appending.
        if let Some(last) = self.lines.last_mut() {
            if last.eol.is_empty() {
                last.eol = eol.to_string();
            }
        }
        for d in doc {
            self.lines.push(Line {
                text: format!("# {d}"),
                eol: eol.to_string(),
                kind: Kind::Other,
            });
        }
        let text = format!("{key} = {raw}");
        self.lines.push(Line {
            kind: classify(&text),
            text,
            eol: eol.to_string(),
        });
        true
    }

    /// The comment block directly above the last active occurrence of `key` (without the leading `#`).
    pub fn doc_for(&self, key: &str) -> Vec<String> {
        let Some(idx) = self
            .lines
            .iter()
            .rposition(|l| matches!(&l.kind, Kind::Entry { key: k, .. } if k == key))
        else {
            return Vec::new();
        };
        let mut doc = Vec::new();
        for l in self.lines[..idx].iter().rev() {
            let t = l.text.trim();
            if !t.starts_with('#') || t.trim_start_matches('#').trim().is_empty() {
                break;
            }
            doc.push(t.trim_start_matches('#').trim().to_string());
        }
        doc.reverse();
        doc
    }

    pub fn append_comment_block(&mut self, doc: &[&str]) {
        let eol = self.default_eol;
        if let Some(last) = self.lines.last_mut() {
            if last.eol.is_empty() {
                last.eol = eol.to_string();
            }
        }
        for d in doc {
            self.lines.push(Line {
                text: d.to_string(),
                eol: eol.to_string(),
                kind: Kind::Other,
            });
        }
    }
}

/// Strip one pair of surrounding double quotes.
pub fn unquote(raw: &str) -> &str {
    let t = raw.trim();
    t.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(t)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "# header\r\n[worldserver]\r\n\r\n# doc for a\r\nA.Key = 5\r\nB.Key=\"text with # hash\"\r\n#Commented = 1\r\nC   =   7\r\nA.Key = 6\r\nlast = 1";

    #[test]
    fn roundtrip_is_byte_identical_including_odd_input() {
        for s in [
            SAMPLE,
            "",
            "\n",
            "no newline",
            "a=1\n\n\nb = 2\n",
            "\u{feff}k = v\r\n",
            "mixed=1\nendings=2\r\nhere=3\n",
            "=novalue\nweird key = 1\n",
        ] {
            assert_eq!(ConfFile::parse(s).to_text(), s, "input: {s:?}");
        }
    }

    #[test]
    fn get_returns_last_active_value_and_ignores_comments() {
        let f = ConfFile::parse(SAMPLE);
        assert_eq!(f.get("A.Key"), Some("6"));
        assert_eq!(f.get("B.Key"), Some("\"text with # hash\""));
        assert_eq!(f.get("Commented"), None);
        assert_eq!(f.get("C"), Some("7"));
        assert_eq!(unquote(f.get("B.Key").unwrap()), "text with # hash");
    }

    #[test]
    fn set_rewrites_only_the_value_and_keeps_spacing() {
        let mut f = ConfFile::parse(SAMPLE);
        assert!(!f.set("C", "9", &[]));
        assert!(!f.set("A.Key", "10", &[]));
        let out = f.to_text();
        assert!(out.contains("C   =   9\r\n"), "{out:?}");
        // last duplicate changed, first untouched
        assert!(out.contains("A.Key = 5\r\n") && out.contains("A.Key = 10\r\n"));
        // everything else byte-identical
        assert_eq!(
            out.replace("C   =   9", "C   =   7")
                .replace("A.Key = 10", "A.Key = 6"),
            SAMPLE
        );
    }

    #[test]
    fn set_appends_missing_key_with_doc_using_file_eol() {
        let mut f = ConfFile::parse("[worldserver]\r\nA = 1");
        assert!(f.set("New.Key", "\"x\"", &["Added by CoA Server Manager"]));
        assert_eq!(
            f.to_text(),
            "[worldserver]\r\nA = 1\r\n# Added by CoA Server Manager\r\nNew.Key = \"x\"\r\n"
        );
        assert_eq!(f.get("New.Key"), Some("\"x\""));
    }

    #[test]
    fn non_utf8_is_refused_rather_than_mangled() {
        assert!(ConfFile::parse_bytes(&[0x41, 0xFF, 0xFE]).is_err());
    }
}
