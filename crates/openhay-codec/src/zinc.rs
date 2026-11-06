//! Zinc grid encoder.
//!
//! Scalars delegate to libhaystack. The grid framing is ours because
//! libhaystack 4.1's grid encoder puts grid meta on the line *after*
//! `ver:"3.0"` (and drops columns from zero-row grids), which other Zinc
//! parsers, including libhaystack's own, reject.

use crate::encode_zinc_scalar;
use libhaystack::val::{Dict, Grid, Value};
use std::fmt::Write;

pub fn encode_grid(grid: &Grid) -> crate::Result<String> {
    let mut out = String::new();
    write_grid(&mut out, grid)?;
    Ok(out)
}

fn write_grid(out: &mut String, grid: &Grid) -> crate::Result<()> {
    out.push_str("ver:\"3.0\"");
    if let Some(meta) = &grid.meta {
        write_tags(out, meta)?;
    }
    out.push('\n');
    if grid.columns.is_empty() {
        out.push_str("empty\n");
        return Ok(());
    }
    for (i, c) in grid.columns.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&c.name);
        if let Some(m) = &c.meta {
            write_tags(out, m)?;
        }
    }
    out.push('\n');
    for row in &grid.rows {
        for (i, c) in grid.columns.iter().enumerate() {
            if i > 0 {
                out.push(',');
            }
            match row.get(&c.name) {
                Some(v) => write_val(out, v)?,
                // A row of all-empty cells in a one-column grid would be a
                // blank line, which readers skip - so the row would vanish.
                None if grid.columns.len() == 1 => out.push('N'),
                None => {}
            }
        }
        out.push('\n');
    }
    Ok(())
}

fn write_tags(out: &mut String, d: &Dict) -> crate::Result<()> {
    for (k, v) in d.iter() {
        if k == "ver" || v.is_null() {
            continue;
        }
        let _ = write!(out, " {k}");
        if !matches!(v, Value::Marker) {
            out.push(':');
            write_val(out, v)?;
        }
    }
    Ok(())
}

fn write_val(out: &mut String, v: &Value) -> crate::Result<()> {
    match v {
        Value::Grid(g) => {
            out.push_str("<<\n");
            write_grid(out, g)?;
            out.push_str(">>");
        }
        // Ours rather than libhaystack's: its DateTime writer shifts Etc-style
        // zones (GMT-10) and its Uri writer doubles every backslash.
        Value::DateTime(_) => {
            let j = crate::hayson::encode_value(v);
            let val = j.get("val").and_then(|x| x.as_str()).unwrap_or_default();
            let tz = j.get("tz").and_then(|x| x.as_str()).unwrap_or("UTC");
            out.push_str(val);
            if !(tz == "UTC" && val.ends_with('Z')) {
                out.push(' ');
                out.push_str(tz);
            }
        }
        Value::Uri(u) => {
            out.push('`');
            for c in u.value().chars() {
                match c {
                    '`' => out.push_str("\\`"),
                    '\n' => out.push_str("\\n"),
                    c => out.push(c),
                }
            }
            out.push('`');
        }
        other => out.push_str(&encode_zinc_scalar(other)?),
    }
    Ok(())
}
