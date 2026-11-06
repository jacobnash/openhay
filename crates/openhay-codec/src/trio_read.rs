//! Trio reader: records of `name: value` lines separated by `---`.
//!
//! openhay's own implementation of the Trio rules (`docHaystack/Trio`),
//! matching what haxall's reference reader does on its own test cases
//! (`openhay.vectors`). In particular an unquoted value is a **Str** unless
//! the whole of it is a scalar: `geoAddr: 13 main st` is the string
//! "13 main st", not the number 13 (libhaystack's reader truncated it).

use crate::zinc_read;
use libhaystack::val::{Dict, Grid, Value};

pub type Result<T> = std::result::Result<T, String>;

/// Every record becomes a row; columns are the union of tag names in order
/// of first appearance.
pub fn read_grid(text: &str) -> Result<Grid> {
    let dicts = read_dicts(text)?;
    Ok(Grid::make_from_dicts(dicts))
}

pub fn read_dicts(text: &str) -> Result<Vec<Dict>> {
    let lines: Vec<&str> = text.lines().collect();
    let mut r = Reader { lines, i: 0 };
    let mut out = Vec::new();
    while let Some(d) = r.dict()? {
        out.push(d);
    }
    Ok(out)
}

struct Reader<'a> {
    lines: Vec<&'a str>,
    i: usize,
}

fn is_blank_or_comment(line: &str) -> bool {
    let t = line.trim();
    t.is_empty() || line.starts_with("//")
}

fn is_tag_name(s: &str) -> bool {
    let mut cs = s.chars();
    matches!(cs.next(), Some(c) if c.is_ascii_lowercase() || c == '_')
        && cs.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl<'a> Reader<'a> {
    fn next_line(&mut self) -> Option<&'a str> {
        let l = self.lines.get(self.i).copied();
        if l.is_some() {
            self.i += 1;
        }
        l
    }

    fn err(&self, msg: &str) -> String {
        format!("{msg} [line {}]", self.i)
    }

    /// Next record, or None at end of input.
    fn dict(&mut self) -> Result<Option<Dict>> {
        let mut d = Dict::new();
        while let Some(line) = self.next_line() {
            if line.starts_with('-') {
                if d.is_empty() {
                    continue; // leading or repeated separators
                }
                break;
            }
            if is_blank_or_comment(line) {
                continue;
            }
            let (name, val) = match line.find(':') {
                None => (line.trim().to_string(), Value::make_marker()),
                Some(colon) => {
                    let name = line[..colon].trim().to_string();
                    let raw = line[colon + 1..].trim();
                    let val = if raw.is_empty() {
                        Value::make_str(&self.indented_text())
                    } else {
                        self.scalar(raw)
                            .map_err(|e| self.err(&format!("invalid value for {name}: {e}")))?
                    };
                    (name, val)
                }
            };
            if !is_tag_name(&name) {
                return Err(self.err(&format!("invalid name: {name}")));
            }
            if d.contains_key(&name) {
                return Err(self.err(&format!("duplicate tag: {name}")));
            }
            d.insert(name, val);
        }
        Ok((!d.is_empty()).then_some(d))
    }

    /// Lines indented under a `name:` line: the minimum indent is stripped,
    /// blank lines are kept, and the block ends at the first line that
    /// starts in column 0 (a `-` separator, the next tag, ...).
    fn indented_text(&mut self) -> String {
        let mut block: Vec<&str> = Vec::new();
        while let Some(line) = self.lines.get(self.i).copied() {
            if line.chars().count() > 1 && !line.starts_with(char::is_whitespace) {
                break;
            }
            self.i += 1;
            block.push(line.trim_end());
        }
        let indent = block
            .iter()
            .filter_map(|l| l.find(|c: char| !c.is_whitespace()))
            .min()
            .unwrap_or(0);
        let mut out = String::new();
        for line in block {
            let part = if line.len() <= indent {
                ""
            } else {
                &line[indent..]
            };
            if !out.is_empty() {
                out.push('\n');
            }
            out.push_str(part);
        }
        out
    }

    fn scalar(&mut self, s: &str) -> Result<Value> {
        let first = s.chars().next().unwrap_or(' ');
        let last = s.chars().last().unwrap_or(' ');
        if first.is_ascii_digit() || first == '-' {
            let b = s.as_bytes();
            // Date / DateTime / Time only when the *whole* value is one;
            // otherwise it's a plain string.
            if s.len() == 10 && b[4] == b'-' {
                return Ok(zinc_read::read_value(s).unwrap_or_else(|_| Value::make_str(s)));
            }
            if s.len() > 20 && b[4] == b'-' {
                return Ok(zinc_read::read_value(s).unwrap_or_else(|_| Value::make_str(s)));
            }
            if s.len() > 3 && (b[1] == b':' || b[2] == b':') {
                return Ok(zinc_read::read_value(s).unwrap_or_else(|_| Value::make_str(s)));
            }
            if !s.contains(' ') {
                return zinc_read::read_value(s);
            }
            return Ok(Value::make_str(s));
        }
        if first == '"' || first == '`' {
            if last != first {
                return Err(format!("invalid quoted literal: {s}"));
            }
            return zinc_read::read_value(s);
        }
        if first == '@' || first == '^' || first == '{' || last == ')' {
            return zinc_read::read_value(s);
        }
        if first == '[' {
            if s == "[" {
                // an indented list: the lines below, until the closing ]
                let body = self.indented_text();
                let joined: Vec<&str> = body
                    .lines()
                    .filter(|l| !l.trim().starts_with("//"))
                    .collect();
                return zinc_read::read_value(&format!("[{}", joined.join(" ")));
            }
            return zinc_read::read_value(s);
        }
        Ok(match s {
            "true" | "T" => Value::make_true(),
            "false" | "F" => Value::make_false(),
            "NA" => Value::make_na(),
            "NaN" => Value::make_number(f64::NAN),
            "INF" => Value::make_number(f64::INFINITY),
            "R" => Value::make_remove(),
            "M" => Value::make_marker(),
            "Zinc:" => {
                let body = self.indented_text();
                return zinc_read::read_grid(&body)
                    .map(Value::make_grid)
                    .or_else(|_| zinc_read::read_value(&body));
            }
            "Trio:" => {
                let body = self.indented_text();
                let mut nested = read_dicts(&body)?;
                return Ok(if nested.is_empty() {
                    Value::Null
                } else {
                    Value::make_dict(nested.remove(0))
                });
            }
            other if other.ends_with(':') => {
                return Err(format!("unsupported indentation format: {other}"));
            }
            other => Value::make_str(other),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn one(text: &str) -> Dict {
        let mut d = read_dicts(text).unwrap();
        assert_eq!(d.len(), 1);
        d.remove(0)
    }

    #[test]
    fn unquoted_strings_are_not_truncated() {
        let d = one("geoAddr: 13 main st\nwhen: 2015-12-29 14:22:39.364Z\n");
        assert_eq!(d.get("geoAddr"), Some(&Value::make_str("13 main st")));
        assert_eq!(
            d.get("when"),
            Some(&Value::make_str("2015-12-29 14:22:39.364Z"))
        );
    }

    #[test]
    fn scalars_bools_and_markers() {
        let d = one("t: true\nf: F\nn: 72\nm\nname: plain words\n");
        assert_eq!(d.get("t"), Some(&Value::make_true()));
        assert_eq!(d.get("f"), Some(&Value::make_false()));
        assert!(matches!(d.get("n"), Some(Value::Number(_))));
        assert_eq!(d.get("m"), Some(&Value::make_marker()));
        assert_eq!(d.get("name"), Some(&Value::make_str("plain words")));
    }

    #[test]
    fn multiline_keeps_trailing_blank_line() {
        let d = one("doc:\n  hello\n  world\n\ncode: x\n");
        assert_eq!(d.get("doc"), Some(&Value::make_str("hello\nworld\n")));
    }

    #[test]
    fn records_and_comments() {
        let v = read_dicts("// header\na\n---\nb: 1\n---\n").unwrap();
        assert_eq!(v.len(), 2);
    }
}
