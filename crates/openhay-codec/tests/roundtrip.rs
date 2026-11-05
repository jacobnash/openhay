use libhaystack::val::*;
use openhay_codec::{Format, decode_grid, decode_grid_auto, encode_grid, json3};
use serde_json::json;

fn sample() -> Grid {
    let mut r1 = Dict::new();
    r1.insert("id".into(), Value::make_ref_with_dis("p1", "AHU-1 DAT"));
    r1.insert("point".into(), Value::make_marker());
    r1.insert("his".into(), Value::make_marker());
    r1.insert(
        "curVal".into(),
        Value::make_number_unit(55.2, libhaystack::units::get_unit("°F").unwrap()),
    );
    r1.insert("dis".into(), Value::make_str("a:b colon string"));
    r1.insert("tz".into(), Value::make_str("New_York"));
    r1.insert("geoCoord".into(), Value::make_coord_from(37.55, -77.45));
    r1.insert(
        "mod".into(),
        Value::make_datetime(
            DateTime::parse_from_rfc3339_with_timezone("2024-01-02T13:45:00-05:00", "New_York")
                .unwrap(),
        ),
    );
    r1.insert(
        "tags".into(),
        Value::make_list(vec![Value::make_str("x"), Value::make_number(1.0)]),
    );
    let mut r2 = Dict::new();
    r2.insert("id".into(), Value::make_ref("p2"));
    r2.insert("enabled".into(), Value::make_bool(true));
    r2.insert("na".into(), Value::make_na());
    r2.insert("uri".into(), Value::make_uri("http://example.com/x"));
    r2.insert("sym".into(), Value::make_symbol("elec-meter"));
    Grid::make_from_dicts(vec![r1, r2])
}

fn same_rows(a: &Grid, b: &Grid) {
    assert_eq!(a.rows.len(), b.rows.len());
    for (x, y) in a.rows.iter().zip(&b.rows) {
        assert_eq!(x, y, "row mismatch");
    }
}

#[test]
fn every_lossless_format_roundtrips() {
    let g = sample();
    for f in [Format::Zinc, Format::Json3, Format::Hayson, Format::Trio] {
        let text = encode_grid(f, &g).unwrap();
        let back = decode_grid(f, &text).unwrap_or_else(|e| panic!("{f:?}: {e}\n{text}"));
        same_rows(&g, &back);
        assert_eq!(
            decode_grid_auto(&text).map(|b| b.rows.len()).ok(),
            Some(2),
            "sniff {f:?}"
        );
    }
}

#[test]
fn json3_wire_shape() {
    let j = json3::encode_grid(&sample());
    let r = &j["rows"][0];
    assert_eq!(r["point"], "m:");
    assert_eq!(r["id"], "r:p1 AHU-1 DAT");
    assert_eq!(r["curVal"], "n:55.2 °F");
    assert_eq!(r["dis"], "s:a:b colon string");
    assert_eq!(r["geoCoord"], "c:37.55,-77.45");
    assert_eq!(r["mod"], "t:2024-01-02T13:45:00-05:00 New_York");
    assert_eq!(j["rows"][1]["na"], "z:");
    assert_eq!(j["meta"]["ver"], "3.0");
}

#[test]
fn json3_decodes_skyspark_style_payload() {
    let text = json!({
        "meta": {"ver": "3.0"},
        "cols": [{"name": "id"}, {"name": "dis"}, {"name": "ts"}, {"name": "val"}],
        "rows": [
            {"id": "r:@ignored", "dis": "plain", "ts": "t:2024-01-02T18:45:00Z UTC", "val": "n:INF"},
            {"id": "r:abc", "ts": "t:2024-01-02T13:45:00-05:00 New_York", "val": "n:12 kW", "bin": "b:text/plain", "x": "x:Span:today"}
        ]
    })
    .to_string();
    let g = decode_grid(Format::Json3, &text).unwrap();
    assert_eq!(g.columns.len(), 4);
    let r = &g.rows[1];
    assert!(matches!(r.get("ts"), Some(Value::DateTime(_))));
    match r.get("val") {
        Some(Value::Number(n)) => {
            assert_eq!(n.value, 12.0);
            assert!(n.unit.is_some());
        }
        other => panic!("{other:?}"),
    }
    assert!(matches!(r.get("bin"), Some(Value::XStr(_))));
    assert!(g.rows[0].get("dis").is_some());
}

#[test]
fn negotiate() {
    assert_eq!(Format::negotiate(None, Format::Hayson), Format::Zinc);
    assert_eq!(
        Format::negotiate(Some("application/json"), Format::Json3),
        Format::Json3
    );
    assert_eq!(
        Format::negotiate(Some("text/csv;q=1, text/zinc"), Format::Hayson),
        Format::Csv
    );
}

#[test]
fn csv_encode() {
    let csv = encode_grid(Format::Csv, &sample()).unwrap();
    let first = csv.lines().next().unwrap();
    assert!(first.contains("id") && first.contains("curVal"));
    assert!(csv.contains('\u{2713}'));
}

#[test]
fn zinc_meta_and_empty_grids_roundtrip() {
    let mut meta = Dict::new();
    meta.insert("err".into(), Value::make_marker());
    meta.insert("dis".into(), Value::make_str("boom"));
    let g = Grid {
        meta: Some(meta.clone()),
        columns: vec![
            Column {
                name: "ts".into(),
                meta: None,
            },
            Column {
                name: "val".into(),
                meta: None,
            },
        ],
        rows: vec![],
        ver: "3.0".into(),
    };
    let z = encode_grid(Format::Zinc, &g).unwrap();
    assert!(z.starts_with("ver:\"3.0\" "), "{z}");
    let back = decode_grid(Format::Zinc, &z).unwrap();
    assert_eq!(back.meta, Some(meta));
    assert_eq!(back.columns.len(), 2);
    let back = decode_grid(
        Format::Zinc,
        &encode_grid(Format::Zinc, &Grid::make_empty()).unwrap(),
    )
    .unwrap();
    assert!(back.rows.is_empty());
}

#[test]
fn special_numbers_survive_every_format() {
    let mut d = Dict::new();
    d.insert("id".into(), Value::make_ref("x"));
    d.insert("nan".into(), Value::make_number(f64::NAN));
    d.insert("inf".into(), Value::make_number(f64::INFINITY));
    d.insert("ninf".into(), Value::make_number(f64::NEG_INFINITY));
    let g = Grid::make_from_dicts(vec![d]);
    for f in [Format::Zinc, Format::Json3, Format::Hayson, Format::Trio] {
        let text = encode_grid(f, &g).unwrap();
        let back = decode_grid(f, &text).unwrap_or_else(|e| panic!("{f:?}: {e}\n{text}"));
        let r = &back.rows[0];
        let num = |k: &str| match r.get(k) {
            Some(Value::Number(n)) => n.value,
            other => panic!("{f:?} {k}: {other:?}\n{text}"),
        };
        assert!(num("nan").is_nan(), "{f:?}");
        assert_eq!(num("inf"), f64::INFINITY, "{f:?}");
        assert_eq!(num("ninf"), f64::NEG_INFINITY, "{f:?}");
    }
}

#[test]
fn hayson_utc_datetime_always_carries_tz() {
    let mut d = Dict::new();
    d.insert(
        "ts".into(),
        Value::make_datetime_from_iso("2024-01-02T18:45:00Z").unwrap(),
    );
    let text = encode_grid(Format::Hayson, &Grid::make_from_dicts(vec![d])).unwrap();
    assert!(text.contains(r#""tz":"UTC""#), "{text}");
    assert!(decode_grid(Format::Hayson, &text).is_ok());
}
