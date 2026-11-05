//! Haystack 3 JSON encoding (<https://project-haystack.org/doc/docHaystack/Json>
//! as of Haystack 3): scalars are strings with a one-letter type prefix.
//!
//! | kind     | example                                   |
//! |----------|-------------------------------------------|
//! | Marker   | `"m:"`                                    |
//! | Remove   | `"-:"`                                    |
//! | NA       | `"z:"`                                    |
//! | Number   | `"n:72.5 °F"`, `"n:INF"`                  |
//! | Ref      | `"r:abc-123 Display"`                     |
//! | Str      | plain, or `"s:..."` when char 1 is a colon|
//! | Uri      | `"u:http://x"`                            |
//! | Symbol   | `"y:elec-meter"`                          |
//! | Date     | `"d:2024-01-02"`                          |
//! | Time     | `"h:13:45:00"`                            |
//! | DateTime | `"t:2024-01-02T13:45:00-05:00 New_York"`  |
//! | Coord    | `"c:37.55,-77.45"`                        |
//! | XStr     | `"x:Type:value"`, Bin `"b:mime"` → XStr   |
//!
//! Known limitation: numbers whose unit is not in the Haystack unit database
//! decode without a unit, because `libhaystack::Number` can only hold units
//! from that database.

use crate::{CodecError, Result, decode_zinc_scalar, encode_zinc_scalar};
use libhaystack::units::get_unit;
use libhaystack::val::{Column, Coord, Dict, Grid, Number, Value, XStr};
use serde_json::{Map, Value as J};

// ---------- encode ----------

pub fn encode_grid(grid: &Grid) -> J {
    let mut meta = grid.meta.as_ref().map(encode_dict_map).unwrap_or_default();
    meta.insert("ver".into(), J::String("3.0".into()));
    let cols: Vec<J> = grid
        .columns
        .iter()
        .map(|c| {
            let mut m = c.meta.as_ref().map(encode_dict_map).unwrap_or_default();
            m.insert("name".into(), J::String(c.name.clone()));
            J::Object(m)
        })
        .collect();
    let rows: Vec<J> = grid
        .rows
        .iter()
        .map(|r| J::Object(encode_dict_map(r)))
        .collect();
    let mut out = Map::new();
    out.insert("meta".into(), J::Object(meta));
    out.insert("cols".into(), J::Array(cols));
    out.insert("rows".into(), J::Array(rows));
    J::Object(out)
}

fn encode_dict_map(d: &Dict) -> Map<String, J> {
    d.iter()
        .filter(|(_, v)| !v.is_null())
        .map(|(k, v)| (k.to_string(), encode_value(v)))
        .collect()
}

pub fn encode_value(v: &Value) -> J {
    match v {
        Value::Null => J::Null,
        Value::Marker => s("m:"),
        Value::Remove => s("-:"),
        Value::Na => s("z:"),
        Value::Bool(b) => J::Bool(b.value),
        Value::Number(n) => s(&format!("n:{}", encode_number(n))),
        Value::Str(st) => {
            let st = st.value();
            if st.as_bytes().get(1) == Some(&b':') {
                s(&format!("s:{st}"))
            } else {
                s(st)
            }
        }
        Value::Uri(u) => s(&format!("u:{}", u.value())),
        Value::Ref(r) => match r.dis() {
            Some(dis) => s(&format!("r:{} {dis}", r.value())),
            None => s(&format!("r:{}", r.value())),
        },
        Value::Symbol(y) => s(&format!("y:{}", y.value())),
        Value::Date(d) => s(&format!("d:{d}")),
        Value::Time(t) => s(&format!("h:{t}")),
        Value::DateTime(_) => {
            let z = encode_zinc_scalar(v).unwrap_or_default();
            // Zinc writes UTC as a bare `...Z`; JSON v3 always names the zone.
            if z.contains(' ') {
                s(&format!("t:{z}"))
            } else {
                s(&format!("t:{z} UTC"))
            }
        }
        Value::Coord(c) => s(&format!("c:{},{}", c.lat, c.long)),
        Value::XStr(x) => s(&format!("x:{}:{}", x.r#type(), x.value())),
        Value::List(l) => J::Array(l.iter().map(encode_value).collect()),
        Value::Dict(d) => J::Object(encode_dict_map(d)),
        Value::Grid(g) => encode_grid(g),
    }
}

fn encode_number(n: &Number) -> String {
    if n.value.is_nan() {
        return "NaN".into();
    }
    if n.value.is_infinite() {
        return if n.value < 0.0 {
            "-INF".into()
        } else {
            "INF".into()
        };
    }
    match n.unit {
        Some(u) => format!("{} {}", n.value, u),
        None => format!("{}", n.value),
    }
}

fn s(x: &str) -> J {
    J::String(x.to_string())
}

// ---------- decode ----------

pub fn decode_grid(text: &str) -> Result<Grid> {
    let j: J = serde_json::from_str(text)?;
    match &j {
        J::Object(m) => grid_from_map(m),
        _ => Err(CodecError::Invalid("JSON v3 grid must be an object".into())),
    }
}

fn is_grid_map(m: &Map<String, J>) -> bool {
    m.contains_key("meta") && m.contains_key("cols") && m.contains_key("rows")
}

fn grid_from_map(m: &Map<String, J>) -> Result<Grid> {
    let meta = match m.get("meta") {
        Some(J::Object(mm)) => {
            let mut d = decode_dict(mm)?;
            d.remove("ver");
            if d.is_empty() { None } else { Some(d) }
        }
        _ => None,
    };
    let mut columns = Vec::new();
    if let Some(J::Array(cols)) = m.get("cols") {
        for c in cols {
            let J::Object(cm) = c else {
                return Err(CodecError::Invalid("column must be an object".into()));
            };
            let name = cm
                .get("name")
                .and_then(J::as_str)
                .ok_or_else(|| CodecError::Invalid("column missing name".into()))?
                .to_string();
            let mut cmeta = Dict::new();
            for (k, v) in cm {
                if k != "name" {
                    cmeta.insert(k.clone(), decode_value(v)?);
                }
            }
            columns.push(Column {
                name,
                meta: if cmeta.is_empty() { None } else { Some(cmeta) },
            });
        }
    }
    let mut rows = Vec::new();
    if let Some(J::Array(rs)) = m.get("rows") {
        for r in rs {
            let J::Object(rm) = r else {
                return Err(CodecError::Invalid("row must be an object".into()));
            };
            rows.push(decode_dict(rm)?);
        }
    }
    Ok(Grid {
        meta,
        columns,
        rows,
        ver: "3.0".into(),
    })
}

fn decode_dict(m: &Map<String, J>) -> Result<Dict> {
    let mut d = Dict::new();
    for (k, v) in m {
        let v = decode_value(v)?;
        if !v.is_null() {
            d.insert(k.clone(), v);
        }
    }
    Ok(d)
}

pub fn decode_value(j: &J) -> Result<Value> {
    Ok(match j {
        J::Null => Value::Null,
        J::Bool(b) => Value::make_bool(*b),
        // Bare JSON numbers aren't legal v3 but appear in the wild.
        J::Number(n) => Value::make_number(n.as_f64().unwrap_or(f64::NAN)),
        J::String(st) => decode_str(st)?,
        J::Array(a) => Value::make_list(a.iter().map(decode_value).collect::<Result<Vec<_>>>()?),
        J::Object(m) if is_grid_map(m) => Value::make_grid(grid_from_map(m)?),
        J::Object(m) => Value::make_dict(decode_dict(m)?),
    })
}

fn decode_str(st: &str) -> Result<Value> {
    let b = st.as_bytes();
    if b.len() < 2 || b[1] != b':' {
        return Ok(Value::make_str(st));
    }
    let body = &st[2..];
    let bad = |what: &str| CodecError::Invalid(format!("bad JSON v3 {what}: {st:?}"));
    Ok(match b[0] {
        b'm' => Value::make_marker(),
        b'-' => Value::make_remove(),
        b'z' => Value::make_na(),
        b's' => Value::make_str(body),
        b'u' => Value::make_uri(body),
        b'y' => Value::make_symbol(body),
        b'r' => match body.split_once(' ') {
            Some((id, dis)) => Value::make_ref_with_dis(id, dis),
            None => Value::make_ref(body),
        },
        b'n' => decode_number(body).ok_or_else(|| bad("number"))?,
        b'd' | b'h' | b't' => {
            let v = decode_zinc_scalar(body).map_err(|_| bad("date/time"))?;
            let ok = matches!(
                (b[0], &v),
                (b'd', Value::Date(_)) | (b'h', Value::Time(_)) | (b't', Value::DateTime(_))
            );
            if !ok {
                return Err(bad("date/time"));
            }
            v
        }
        b'c' => {
            let (lat, lng) = body.split_once(',').ok_or_else(|| bad("coord"))?;
            let lat: f64 = lat.trim().parse().map_err(|_| bad("coord"))?;
            let lng: f64 = lng.trim().parse().map_err(|_| bad("coord"))?;
            Value::make_coord(Coord::make(lat, lng))
        }
        b'x' => {
            let (ty, val) = body.split_once(':').ok_or_else(|| bad("xstr"))?;
            Value::make_xstr(XStr::make(ty, val))
        }
        b'b' => Value::make_xstr(XStr::make("Bin", body)),
        // Unknown single-letter prefix: it's just a string that happens to
        // have a colon in position 1 (e.g. "a:b").
        _ => Value::make_str(st),
    })
}

fn decode_number(body: &str) -> Option<Value> {
    let (num, unit) = match body.split_once(' ') {
        Some((n, u)) => (n, Some(u.trim())),
        None => (body, None),
    };
    let x = match num {
        "INF" => f64::INFINITY,
        "-INF" => f64::NEG_INFINITY,
        "NaN" => f64::NAN,
        _ => num.parse().ok()?,
    };
    Some(match unit.and_then(get_unit) {
        Some(u) => Value::make_number_unit(x, u),
        None => Value::make_number(x),
    })
}
