//! Every haxall Zinc and Trio test case that decodes must also round-trip
//! through openhay's own writer in the same format. (The Python suite checks
//! the decoded values against haxall's; this checks openhay can send them
//! back.)

use openhay_codec::{Format, decode_grid, encode_grid};
use serde_json::Value as J;

fn vectors() -> J {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../openhay-py/python/openhay/vectors/haxall/testHaystack-4.0.6.json"
    );
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn hayson_rows(g: &libhaystack::val::Grid) -> Vec<J> {
    g.rows
        .iter()
        .map(|r| {
            openhay_codec::hayson::encode_value(&libhaystack::val::Value::make_dict(r.clone()))
        })
        .collect()
}

fn check(format: Format, key: &str, field: &str) {
    let v = vectors();
    let mut n = 0;
    for case in v[key].as_array().unwrap() {
        let text = case[field].as_str().unwrap();
        let g = decode_grid(format, text).unwrap_or_else(|e| panic!("{}: {e}", case["src"]));
        let again = encode_grid(format, &g).unwrap();
        let back = decode_grid(format, &again)
            .unwrap_or_else(|e| panic!("{}: {e}\n--- re-encoded\n{again}", case["src"]));
        // Compare as Hayson, so NaN equals NaN.
        assert_eq!(
            hayson_rows(&back),
            hayson_rows(&g),
            "{}\n--- re-encoded\n{again}",
            case["src"]
        );
        n += 1;
    }
    assert!(n > 10, "{key}: only {n} cases");
}

#[test]
fn haxall_zinc_grids_roundtrip() {
    check(Format::Zinc, "zinc_grids", "zinc");
}

#[test]
fn haxall_trio_roundtrips() {
    check(Format::Trio, "trio", "trio");
}
