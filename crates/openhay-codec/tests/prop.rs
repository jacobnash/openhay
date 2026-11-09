//! Property tests: any grid of Haystack values survives encode -> decode in
//! every lossless format. Runs inside `cargo test` (256 cases per format by
//! default; `PROPTEST_CASES=5000 cargo test -p openhay-codec --test prop`
//! for a longer soak).

use chrono::{NaiveDate, NaiveTime};
use libhaystack::units::get_unit;
use libhaystack::val::*;
use openhay_codec::{Format, decode_grid, encode_grid};
use proptest::prelude::*;

const UNITS: [&str; 6] = ["°F", "°C", "kW", "%", "m³/h", "inH₂O"];
const TZS: [(&str, &str); 4] = [
    ("New_York", "-05:00"),
    ("UTC", "Z"),
    ("Berlin", "+01:00"),
    ("Kolkata", "+05:30"),
];

fn tag() -> impl Strategy<Value = String> {
    "[a-z][a-zA-Z0-9_]{0,8}"
}

fn text() -> impl Strategy<Value = String> {
    // Plain text plus the characters every codec has to escape, and strings
    // that look like JSON v3 prefixes ("m:", "r:x").
    prop_oneof![
        "[ -~]{0,16}",
        "[a-z]:[ -~]{0,6}",
        proptest::collection::vec(
            prop_oneof![
                Just('"'),
                Just('\\'),
                Just('\n'),
                Just('\t'),
                Just('$'),
                Just('é'),
                Just('°'),
                Just('😀'),
                Just(','),
                Just(' '),
            ],
            0..6
        )
        .prop_map(|v| v.into_iter().collect()),
    ]
}

fn number() -> impl Strategy<Value = Value> {
    let finite = prop_oneof![
        (-1_000_000i64..1_000_000).prop_map(|i| i as f64),
        (-1e6f64..1e6),
        Just(0.1),
        Just(-0.5),
        Just(1e-9),
    ];
    (finite, proptest::option::of(0..UNITS.len())).prop_map(|(x, u)| match u {
        Some(i) => Value::make_number_unit(x, get_unit(UNITS[i]).expect("db unit")),
        None => Value::make_number(x),
    })
}

fn scalar() -> impl Strategy<Value = Value> {
    let id = "[a-zA-Z0-9_][a-zA-Z0-9_:.~-]{0,10}";
    prop_oneof![
        Just(Value::make_marker()),
        Just(Value::make_na()),
        any::<bool>().prop_map(Value::make_bool),
        number(),
        text().prop_map(|s| Value::make_str(&s)),
        (id, proptest::option::of(text())).prop_map(|(i, d)| match d {
            Some(d) => Value::make_ref_with_dis(&i, &d),
            None => Value::make_ref(&i),
        }),
        "[a-z]{1,6}://[a-z0-9./]{0,10}".prop_map(|u| Value::make_uri(&u)),
        "[a-z][a-zA-Z0-9_-]{0,8}".prop_map(|s| Value::make_symbol(&s)),
        (1900i32..2100, 1u32..13, 1u32..29).prop_map(|(y, m, d)| Value::make_date(
            NaiveDate::from_ymd_opt(y, m, d).unwrap().into()
        )),
        (0u32..24, 0u32..60, 0u32..60).prop_map(|(h, m, s)| Value::make_time(
            NaiveTime::from_hms_opt(h, m, s).unwrap().into()
        )),
        (
            2000i32..2040,
            1u32..13,
            1u32..29,
            0u32..24,
            0u32..60,
            0..TZS.len()
        )
            .prop_map(|(y, mo, d, h, mi, z)| {
                // Pick a winter date so the fixed offsets in TZS are right.
                let (tz, off) = TZS[z];
                let mo = if tz == "UTC" {
                    mo
                } else {
                    [1, 2, 12][mo as usize % 3]
                };
                let iso = format!("{y:04}-{mo:02}-{d:02}T{h:02}:{mi:02}:00{off}");
                Value::make_datetime(
                    DateTime::parse_from_rfc3339_with_timezone(&iso, tz).expect(&iso),
                )
            }),
        (-90_000i32..=90_000, -180_000i32..=180_000)
            .prop_map(|(a, b)| Value::make_coord_from(a as f64 / 1000.0, b as f64 / 1000.0)),
        ("[A-Z][a-zA-Z]{0,6}", "[ -~]{0,10}").prop_map(|(t, v)| Value::make_xstr(XStr::make(t, v))),
    ]
}

fn value() -> impl Strategy<Value = Value> {
    scalar().prop_recursive(2, 12, 4, |inner| {
        prop_oneof![
            proptest::collection::vec(inner.clone(), 0..4).prop_map(Value::make_list),
            proptest::collection::btree_map(tag(), inner, 0..4).prop_map(|m| {
                let mut d = Dict::new();
                for (k, v) in m {
                    d.insert(k, v);
                }
                Value::make_dict(d)
            }),
        ]
    })
}

fn grid() -> impl Strategy<Value = Grid> {
    proptest::collection::vec(proptest::collection::btree_map(tag(), value(), 1..6), 0..5).prop_map(
        |rows| {
            Grid::make_from_dicts(
                rows.into_iter()
                    .map(|m| {
                        let mut d = Dict::new();
                        for (k, v) in m {
                            d.insert(k, v);
                        }
                        d
                    })
                    .collect(),
            )
        },
    )
}

fn roundtrip(format: Format, g: &Grid) -> Result<(), TestCaseError> {
    let text = encode_grid(format, g).map_err(|e| TestCaseError::fail(e.to_string()))?;
    let back = decode_grid(format, &text)
        .map_err(|e| TestCaseError::fail(format!("{e}\n--- encoded\n{text}")))?;
    prop_assert_eq!(back.rows.len(), g.rows.len(), "{}", text);
    for (a, b) in g.rows.iter().zip(&back.rows) {
        prop_assert_eq!(a, b, "{:?} encoded as\n{}", format, text);
    }
    Ok(())
}

proptest! {
    #[test]
    fn zinc_roundtrips(g in grid()) { roundtrip(Format::Zinc, &g)?; }

    #[test]
    fn hayson_roundtrips(g in grid()) { roundtrip(Format::Hayson, &g)?; }

    #[test]
    fn json3_roundtrips(g in grid()) { roundtrip(Format::Json3, &g)?; }

    #[test]
    fn trio_roundtrips(g in grid()) { roundtrip(Format::Trio, &g)?; }

    /// CSV is encode-only and lossy, but it must never fail on a valid grid,
    /// and it must have one line per row plus the header (newlines in cells
    /// are quoted, so count records the CSV way).
    #[test]
    fn csv_encodes_every_grid(g in grid()) {
        let csv = encode_grid(Format::Csv, &g).unwrap();
        prop_assert_eq!(csv_records(&csv), g.rows.len() + 1, "{}", csv);
    }
}

fn csv_records(csv: &str) -> usize {
    let (mut n, mut quoted) = (0, false);
    for c in csv.chars() {
        match c {
            '"' => quoted = !quoted,
            '\n' if !quoted => n += 1,
            _ => {}
        }
    }
    n
}
