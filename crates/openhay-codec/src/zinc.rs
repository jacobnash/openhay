//! Zinc grid encoder.
//!
//! Most scalars delegate to libhaystack. The grid framing is ours because
//! libhaystack 4.1's grid encoder puts grid meta on the line *after*
//! `ver:"3.0"` (and drops columns from zero-row grids), which other Zinc
//! parsers, including libhaystack's own, reject. Lists and Dicts are ours
//! too, so the values inside them get the same writers as top-level ones.

use libhaystack::encoding::zinc::encode::ToZinc;
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

/// One value in Zinc syntax.
pub fn encode_value(v: &Value) -> crate::Result<String> {
    let mut out = String::new();
    write_val(&mut out, v)?;
    Ok(out)
}

fn libhaystack_zinc(v: &Value) -> crate::Result<String> {
    v.to_zinc_string()
        .map_err(|e| crate::CodecError::Zinc(e.to_string()))
}

fn write_val(out: &mut String, v: &Value) -> crate::Result<()> {
    match v {
        Value::List(l) => {
            out.push('[');
            for (i, x) in l.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_val(out, x)?;
            }
            out.push(']');
        }
        Value::Dict(d) => {
            out.push('{');
            let mut first = true;
            for (k, x) in d.iter() {
                if x.is_null() {
                    continue;
                }
                if !first {
                    out.push(' ');
                }
                first = false;
                out.push_str(k);
                if !matches!(x, Value::Marker) {
                    out.push(':');
                    write_val(out, x)?;
                }
            }
            out.push('}');
        }
        // libhaystack writes a Ref's dis unescaped too.
        Value::Ref(r) => {
            out.push('@');
            out.push_str(r.value());
            if let Some(dis) = r.dis() {
                out.push(' ');
                out.push_str(&libhaystack_zinc(&Value::make_str(dis))?);
            }
        }
        // libhaystack writes the value unescaped, so `A("\")` came out as
        // an unterminated `A("\")`. The value is a Zinc Str.
        Value::XStr(x) => {
            let ty = x.r#type();
            let mut cs = ty.chars();
            if let Some(c) = cs.next() {
                out.extend(c.to_uppercase());
                out.push_str(cs.as_str());
            }
            out.push('(');
            out.push_str(&libhaystack_zinc(&Value::make_str(x.value()))?);
            out.push(')');
        }
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
        other => out.push_str(&libhaystack_zinc(other)?),
    }
    Ok(())
}
