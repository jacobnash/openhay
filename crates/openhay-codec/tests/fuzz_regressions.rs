//! Inputs the fuzz job found (fuzz/, `cargo +nightly fuzz run <target>`),
//! minimised. Each must hold the fuzz targets' rule: a grid openhay decodes
//! must encode back to text it decodes again.

use libhaystack::val::Value;
use openhay_codec::{Format, decode_grid, encode_grid};

fn holds_roundtrip(format: Format, text: &str) {
    let Ok(grid) = decode_grid(format, text) else {
        return;
    };
    let again = encode_grid(format, &grid).unwrap();
    decode_grid(format, &again)
        .unwrap_or_else(|e| panic!("{format:?}: {e}\n--- input\n{text}\n--- re-encoded\n{again}"));
}

/// fuzz trio crash-178dc363: a coordinate whose longitude overflows to
/// infinity (`-77.55…e255…`) decoded as a Coord, and the writer's
/// `C(37.65,-inf)` didn't decode. Non-finite coordinates are not coordinates.
#[test]
fn trio_coord_that_overflows_to_infinity() {
    let text = "m\nfeoCoord: C(37.65,-77.5e400)\n";
    holds_roundtrip(Format::Trio, text);
    if let Ok(g) = decode_grid(Format::Trio, text) {
        let v = g.rows[0].get("feoCoord");
        assert!(!matches!(v, Some(Value::Coord(_))), "{v:?}");
    }
    // the same in Zinc and JSON v3
    assert!(decode_grid(Format::Zinc, "ver:\"3.0\"\nc\nC(1e400,2)\n").is_err());
    holds_roundtrip(
        Format::Json3,
        r#"{"meta":{"ver":"3.0"},"cols":[{"name":"c"}],"rows":[{"c":"c:inf,2"}]}"#,
    );
}

/// The fuzzer's input itself, as it came.
#[test]
fn trio_crash_178dc363_as_found() {
    let text = "m\nbt:t12:34\ngeoCoord: C(37.65,-77.61)\n_a: NA\nsymbol: ot-water\nidNew:@13b27fda-284fd8e2\n\
idComplex:@foo:bar\ndate: 2009-01-02\ntime1: 13:05\ntime2: 7:56\ntime3: 6:12:34\nfeoCoord: C(37.65,-77.5555555555555555555555\
555555555555555555555555555555555555555555555555555555555555555555555555555555558e25555555555561)\n_0: \"unnamed\"\n";
    holds_roundtrip(Format::Trio, text);
}

/// fuzz zinc crash-169f31c2: grid meta with a UTC DateTime followed by a tag
/// whose name looks like a zone (`U04:05:06`). Written as `...Z U04:05:06`,
/// the reader took `U04` for the DateTime's zone. The writer now always
/// names the zone (`...Z UTC`), as haxall does.
#[test]
fn zinc_utc_datetime_then_a_zone_like_tag() {
    let text = "ver:\"3.0\" a:2009-02-03T04:05:06Z UTC U04:05:06\nx\n1\n";
    let g = decode_grid(Format::Zinc, text).unwrap();
    let again = encode_grid(Format::Zinc, &g).unwrap();
    assert!(again.contains("2009-02-03T04:05:06Z UTC"), "{again}");
    let back = decode_grid(Format::Zinc, &again).unwrap();
    assert_eq!(back.meta, g.meta);
    holds_roundtrip(
        Format::Zinc,
        "ver:\"3.0\" Bia: 2009-02-03T04:05:06Z foo b: 2010-02-03U5:06Z foo \
b: 2010-02-03U04:05:06Z UTC bar c: 2009-12-03T04:05:06Zo03T04:05:06ZoL ndon baz\na\n3.81469726976name\n\
2010-12-18T14:11:30.924Z\n273\n7.15625E-4kWh/fvu\u{b2}",
    );
}
