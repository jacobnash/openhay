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
            if let Some(v) = row.get(&c.name) {
                write_val(out, v)?;
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
        other => out.push_str(&encode_zinc_scalar(other)?),
    }
    Ok(())
}
