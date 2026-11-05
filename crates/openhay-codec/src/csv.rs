//! CSV encoding per the Haystack spec: header row of column names (or `dis`
//! from column meta), then one row per record. Markers render as `✓`, other
//! scalars as their Zinc form without quotes. Lossy and encode-only.

use crate::encode_zinc_scalar;
use libhaystack::val::{Grid, Value};

pub fn encode_grid(grid: &Grid) -> String {
    let mut out = String::new();
    let header: Vec<String> = grid
        .columns
        .iter()
        .map(|c| {
            let dis = c.meta.as_ref().and_then(|m| match m.get("dis") {
                Some(Value::Str(s)) => Some(s.value().to_string()),
                _ => None,
            });
            quote(&dis.unwrap_or_else(|| c.name.clone()))
        })
        .collect();
    out.push_str(&header.join(","));
    out.push('\n');
    for row in &grid.rows {
        let cells: Vec<String> = grid
            .columns
            .iter()
            .map(|c| quote(&cell(row.get(&c.name).unwrap_or(&Value::Null))))
            .collect();
        out.push_str(&cells.join(","));
        out.push('\n');
    }
    out
}

fn cell(v: &Value) -> String {
    match v {
        Value::Null => String::new(),
        Value::Marker => "\u{2713}".into(),
        Value::Str(s) => s.value().to_string(),
        Value::Uri(u) => u.value().to_string(),
        Value::Ref(r) => match r.dis() {
            Some(d) => format!("@{} {d}", r.value()),
            None => format!("@{}", r.value()),
        },
        other => encode_zinc_scalar(other).unwrap_or_default(),
    }
}

fn quote(s: &str) -> String {
    if s.contains([',', '"', '\n', '\r']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}
