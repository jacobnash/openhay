//! JSON v3 (prefixed strings) and CSV, value by value.

use libhaystack::units::get_unit;
use libhaystack::val::*;
use openhay_codec::{Format, decode_grid, encode_grid, json3};
use serde_json::json;

fn dec(s: &str) -> Value {
    json3::decode_value(&json!(s)).unwrap()
}

#[test]
fn every_prefix_decodes() {
    assert_eq!(dec("m:"), Value::make_marker());
    assert_eq!(dec("-:"), Value::make_remove());
    assert_eq!(dec("z:"), Value::make_na());
    assert_eq!(dec("s:m:"), Value::make_str("m:"));
    assert_eq!(dec("u:http://x/y"), Value::make_uri("http://x/y"));
    assert_eq!(dec("y:elec-meter"), Value::make_symbol("elec-meter"));
    assert_eq!(dec("r:abc"), Value::make_ref("abc"));
    let r = dec("r:abc AHU 1 DAT");
    assert!(matches!(&r, Value::Ref(x) if x.value() == "abc" && x.dis() == Some("AHU 1 DAT")));
    assert_eq!(
        dec("n:72.5 °F"),
        Value::make_number_unit(72.5, get_unit("°F").unwrap())
    );
    assert_eq!(dec("n:-3"), Value::make_number(-3.0));
    assert!(matches!(dec("n:NaN"), Value::Number(n) if n.value.is_nan()));
    assert!(matches!(dec("n:-INF"), Value::Number(n) if n.value == f64::NEG_INFINITY));
    assert!(matches!(dec("d:2024-01-02"), Value::Date(_)));
    assert!(matches!(dec("h:13:45:00"), Value::Time(_)));
    assert!(matches!(
        dec("t:2024-01-02T13:45:00-05:00 New_York"),
        Value::DateTime(_)
    ));
    assert!(matches!(dec("c:37.55,-77.45"), Value::Coord(c) if c.lat == 37.55 && c.long == -77.45));
    assert!(
        matches!(dec("x:Span:today"), Value::XStr(x) if x.r#type() == "Span" && x.value() == "today")
    );
    assert!(matches!(dec("b:text/plain"), Value::XStr(x) if x.r#type() == "Bin"));
}

#[test]
fn strings_that_only_look_prefixed() {
    // An unknown letter before the colon is just a string.
    assert_eq!(dec("a:b"), Value::make_str("a:b"));
    assert_eq!(dec("m"), Value::make_str("m"));
    assert_eq!(dec(""), Value::make_str(""));
    assert_eq!(dec("é:x"), Value::make_str("é:x"));
    // And the encoder escapes any string whose second char is a colon, so
    // it can't come back as a Marker or a Ref.
    for s in ["m:", "r:abc", "a:b", "n:72"] {
        let j = json3::encode_value(&Value::make_str(s));
        assert_eq!(j, json!(format!("s:{s}")));
        assert_eq!(json3::decode_value(&j).unwrap(), Value::make_str(s));
    }
    assert_eq!(json3::encode_value(&Value::make_str("ab:c")), json!("ab:c"));
}

#[test]
fn bad_prefixed_values_are_errors() {
    for s in [
        "n:abc",
        "d:13:45:00",
        "h:2024-01-02",
        "t:nope",
        "c:1",
        "c:a,b",
        "x:NoColon",
    ] {
        assert!(json3::decode_value(&json!(s)).is_err(), "{s}");
    }
}

#[test]
fn bare_json_and_nesting() {
    // Bare numbers and bools aren't legal v3 but turn up in the wild.
    assert_eq!(
        json3::decode_value(&json!(5)).unwrap(),
        Value::make_number(5.0)
    );
    assert_eq!(
        json3::decode_value(&json!(true)).unwrap(),
        Value::make_bool(true)
    );
    let v = json3::decode_value(&json!({"a": "m:", "b": ["n:1", {"c": "r:x"}]})).unwrap();
    let Value::Dict(d) = v else { panic!("dict") };
    assert_eq!(d.get("a"), Some(&Value::make_marker()));
    // An object with meta/cols/rows is a nested grid.
    let g = json3::decode_value(
        &json!({"meta": {"ver": "3.0"}, "cols": [{"name": "x"}], "rows": [{"x": "n:1"}]}),
    )
    .unwrap();
    assert!(matches!(g, Value::Grid(g) if g.rows.len() == 1));
}

#[test]
fn grid_shape_errors() {
    for bad in [
        "[]",
        "42",
        r#"{"meta":{},"cols":["x"],"rows":[]}"#,
        r#"{"meta":{},"cols":[{"nm":"x"}],"rows":[]}"#,
        r#"{"meta":{},"cols":[{"name":"x"}],"rows":[1]}"#,
    ] {
        assert!(decode_grid(Format::Json3, bad).is_err(), "{bad}");
    }
}

#[test]
fn grid_meta_column_meta_and_nulls() {
    let g = decode_grid(
        Format::Json3,
        r#"{"meta":{"ver":"3.0","hisStart":"d:2024-01-01"},
            "cols":[{"name":"ts","dis":"Time"},{"name":"v"}],
            "rows":[{"ts":"d:2024-01-01","v":null}]}"#,
    )
    .unwrap();
    let meta = g.meta.as_ref().unwrap();
    assert!(!meta.has("ver") && meta.has("hisStart"));
    assert_eq!(
        g.columns[0].meta.as_ref().unwrap().get("dis"),
        Some(&Value::make_str("Time"))
    );
    assert!(!g.rows[0].has("v"), "null cells are absent");
    let back = decode_grid(Format::Json3, &encode_grid(Format::Json3, &g).unwrap()).unwrap();
    assert_eq!(back.rows, g.rows);
    assert_eq!(back.columns[0].meta, g.columns[0].meta);
}

#[test]
fn utc_datetime_names_its_zone() {
    let dt = Value::make_datetime(
        DateTime::parse_from_rfc3339_with_timezone("2024-01-02T13:45:00Z", "UTC").unwrap(),
    );
    let j = json3::encode_value(&dt);
    assert!(j.as_str().unwrap().ends_with(" UTC"), "{j}");
    assert_eq!(json3::decode_value(&j).unwrap(), dt);
}

#[test]
fn csv_quotes_and_dis_headers() {
    let mut d = Dict::new();
    d.insert("a".into(), Value::make_str("x,\"y\"\nz"));
    d.insert("m".into(), Value::make_marker());
    d.insert("r".into(), Value::make_ref_with_dis("p1", "AHU 1"));
    let mut g = Grid::make_from_dicts(vec![d, Dict::new()]);
    let mut cm = Dict::new();
    cm.insert("dis".into(), Value::make_str("Col, A"));
    g.columns.iter_mut().find(|c| c.name == "a").unwrap().meta = Some(cm);
    let csv = encode_grid(Format::Csv, &g).unwrap();
    let mut lines = csv.splitn(2, '\n');
    let header = lines.next().unwrap();
    assert!(header.contains("\"Col, A\""), "{header}");
    let body = lines.next().unwrap();
    assert!(body.contains("\"x,\"\"y\"\"\nz\""), "{body}");
    assert!(
        body.contains('\u{2713}') && body.contains("@p1 AHU 1"),
        "{body}"
    );
    // The empty second row is still a row.
    assert!(csv.ends_with(",,\n") || csv.ends_with("\n\n"), "{csv:?}");
    assert!(
        decode_grid(Format::Csv, &csv).is_err(),
        "CSV is encode-only"
    );
}
