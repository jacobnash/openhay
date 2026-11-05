//! Hayson (Haystack 4 JSON) encoder.
//!
//! Delegates to libhaystack's serde impls except for Numbers. libhaystack
//! 4.1 serializes a unitless NaN / ±INF through `serialize_f64`, which
//! serde_json turns into `null`, silently dropping the reading. Hayson
//! encodes those as `{"_kind":"number","val":"NaN"}`.

use libhaystack::val::{Grid, Number, Value};
use serde_json::{Map, Value as J, json};

pub fn encode_grid(grid: &Grid) -> J {
    let mut meta = Map::new();
    meta.insert("ver".into(), J::String("3.0".into()));
    if let Some(m) = &grid.meta {
        for (k, v) in m.iter() {
            if k != "ver" {
                meta.insert(k.to_string(), encode_value(v));
            }
        }
    }
    let cols: Vec<J> = grid
        .columns
        .iter()
        .map(|c| {
            let mut o = Map::new();
            o.insert("name".into(), J::String(c.name.clone()));
            if let Some(m) = &c.meta {
                o.insert(
                    "meta".into(),
                    J::Object(
                        m.iter()
                            .map(|(k, v)| (k.to_string(), encode_value(v)))
                            .collect(),
                    ),
                );
            }
            J::Object(o)
        })
        .collect();
    let rows: Vec<J> = grid
        .rows
        .iter()
        .map(|r| {
            J::Object(
                r.iter()
                    .filter(|(_, v)| !v.is_null())
                    .map(|(k, v)| (k.to_string(), encode_value(v)))
                    .collect(),
            )
        })
        .collect();
    json!({"_kind": "grid", "meta": meta, "cols": cols, "rows": rows})
}

pub fn encode_value(v: &Value) -> J {
    match v {
        Value::Number(n) => encode_number(n),
        Value::List(l) => J::Array(l.iter().map(encode_value).collect()),
        Value::Dict(d) => J::Object(
            d.iter()
                .map(|(k, v)| (k.to_string(), encode_value(v)))
                .collect(),
        ),
        Value::Grid(g) => encode_grid(g),
        Value::DateTime(_) => {
            // libhaystack omits `tz` for UTC (Hayson allows that), but some
            // clients (e.g. phable) index it unconditionally - always send it.
            let mut j = serde_json::to_value(v).unwrap_or(J::Null);
            if let J::Object(o) = &mut j {
                o.entry("tz").or_insert_with(|| J::String("UTC".into()));
            }
            j
        }
        other => serde_json::to_value(other).unwrap_or(J::Null),
    }
}

fn encode_number(n: &Number) -> J {
    let special = if n.value.is_nan() {
        Some("NaN")
    } else if n.value == f64::INFINITY {
        Some("INF")
    } else if n.value == f64::NEG_INFINITY {
        Some("-INF")
    } else {
        None
    };
    match (special, n.unit) {
        (Some(s), Some(u)) => json!({"_kind": "number", "val": s, "unit": u.symbol()}),
        (Some(s), None) => json!({"_kind": "number", "val": s}),
        _ => serde_json::to_value(Value::Number(*n)).unwrap_or(J::Null),
    }
}

// ---------- decode ----------

/// Decode a Hayson grid. Scalars go through libhaystack's serde impls except
/// string-valued numbers (`"NaN"`, `"INF"`, `"-INF"`), which it rejects.
pub fn decode_grid(text: &str) -> crate::Result<Grid> {
    let j: J = serde_json::from_str(text)?;
    match decode_value(&j)? {
        Value::Grid(g) => Ok(*g),
        other => Err(crate::CodecError::Invalid(format!(
            "expected hayson grid, got {other:?}"
        ))),
    }
}

pub fn decode_value(j: &J) -> crate::Result<Value> {
    use libhaystack::val::{Column, Dict};
    Ok(match j {
        J::Array(a) => Value::make_list(
            a.iter()
                .map(decode_value)
                .collect::<crate::Result<Vec<_>>>()?,
        ),
        J::Object(o) => match o.get("_kind").and_then(J::as_str) {
            None | Some("dict") => {
                let mut d = Dict::new();
                for (k, v) in o {
                    if k != "_kind" {
                        d.insert(k.clone(), decode_value(v)?);
                    }
                }
                Value::make_dict(d)
            }
            Some("number") if o.get("val").is_some_and(J::is_string) => {
                let x = match o["val"].as_str().unwrap_or_default() {
                    "NaN" => f64::NAN,
                    "INF" => f64::INFINITY,
                    "-INF" => f64::NEG_INFINITY,
                    s => s
                        .parse()
                        .map_err(|_| crate::CodecError::Invalid(format!("bad number {s:?}")))?,
                };
                match o
                    .get("unit")
                    .and_then(J::as_str)
                    .and_then(libhaystack::units::get_unit)
                {
                    Some(u) => Value::make_number_unit(x, u),
                    None => Value::make_number(x),
                }
            }
            Some("grid") => {
                let dict_of = |v: Option<&J>| -> crate::Result<Option<Dict>> {
                    match v.map(decode_value).transpose()? {
                        Some(Value::Dict(d)) if !d.is_empty() => Ok(Some(d)),
                        _ => Ok(None),
                    }
                };
                let mut meta = dict_of(o.get("meta"))?;
                if let Some(m) = meta.as_mut() {
                    m.remove("ver");
                    if m.is_empty() {
                        meta = None;
                    }
                }
                let mut columns = Vec::new();
                for c in o.get("cols").and_then(J::as_array).into_iter().flatten() {
                    let name = c
                        .get("name")
                        .and_then(J::as_str)
                        .ok_or_else(|| crate::CodecError::Invalid("column missing name".into()))?;
                    columns.push(Column {
                        name: name.to_string(),
                        meta: dict_of(c.get("meta"))?,
                    });
                }
                let mut rows = Vec::new();
                for r in o.get("rows").and_then(J::as_array).into_iter().flatten() {
                    match decode_value(r)? {
                        Value::Dict(mut d) => {
                            d.retain(|_, v| !v.is_null());
                            rows.push(d)
                        }
                        _ => {
                            return Err(crate::CodecError::Invalid("row must be an object".into()));
                        }
                    }
                }
                Value::make_grid(Grid {
                    meta,
                    columns,
                    rows,
                    ver: "3.0".into(),
                })
            }
            Some(_) => serde_json::from_value(j.clone())?,
        },
        other => serde_json::from_value(other.clone())?,
    })
}
