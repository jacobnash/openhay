//! Scored against Windrow's mock hospital (`windrow/mock_hospital.py`): every
//! point kind it generates, in all four of its naming styles, labelled by
//! hand with the Haystack 4 tags it should get. The label alone is used,
//! with no description or units, as in Windrow#21's check.
//!
//! The bar: every tag set produced is exactly right (no wrong guesses), and
//! coverage stays at or above the numbers below. A miss is fine; a wrong
//! answer is not.

use openhay_tag::Tagger;

/// (mock_hospital tokens, expected Haystack tags; empty = no tags expected).
const KINDS: &[(&[&str], &[&str])] = &[
    // AHU
    (
        &["discharge", "air", "temp"],
        &["discharge", "air", "temp", "sensor"],
    ),
    (
        &["return", "air", "temp"],
        &["return", "air", "temp", "sensor"],
    ),
    (
        &["mixed", "air", "temp"],
        &["mixed", "air", "temp", "sensor"],
    ),
    (
        &["outside", "air", "temp"],
        &["outside", "air", "temp", "sensor"],
    ),
    (
        &["supply", "fan", "status"],
        &["discharge", "fan", "run", "sensor"],
    ),
    (
        &["supply", "fan", "cmd"],
        &["discharge", "fan", "run", "cmd"],
    ),
    (
        &["static", "pressure"],
        &["discharge", "air", "pressure", "sensor"],
    ),
    // VAV
    (&["zone", "air", "temp"], &["zone", "air", "temp", "sensor"]),
    (
        &["zone", "temp", "occ", "sp"],
        &["zone", "air", "temp", "occ", "sp"],
    ),
    (
        &["zone", "temp", "unocc", "sp"],
        &["zone", "air", "temp", "unocc", "sp"],
    ),
    (&["damper", "cmd"], &["damper", "cmd"]),
    (&["damper", "pos"], &["damper", "sensor"]),
    (&["zone", "airflow"], &["air", "flow", "sensor"]),
    (&["zone", "co2"], &["zone", "co2", "sensor"]),
    (
        &["zone", "humidity"],
        &["zone", "air", "humidity", "sensor"],
    ),
    // chiller
    (
        &["chw", "sup", "temp"],
        &["chilled", "water", "leaving", "temp", "sensor"],
    ),
    (
        &["chw", "ret", "temp"],
        &["chilled", "water", "entering", "temp", "sensor"],
    ),
    (
        &["cond", "wtr", "temp"],
        &["condenser", "water", "temp", "sensor"],
    ),
    (&["chiller", "run", "status"], &["run", "sensor"]),
    (&["chiller", "start", "cmd"], &["run", "cmd"]),
    (&["kw", "input"], &["elec", "power", "sensor"]),
    // boiler
    (
        &["hw", "sup", "temp"],
        &["hot", "water", "leaving", "temp", "sensor"],
    ),
    (
        &["hw", "ret", "temp"],
        &["hot", "water", "entering", "temp", "sensor"],
    ),
    (&["boiler", "run", "status"], &["run", "sensor"]),
    (&["boiler", "start", "cmd"], &["run", "cmd"]),
    (&["firing", "rate"], &[]), // no Haystack 4 tag for it
    // pumps
    (&["pump", "run", "status"], &["run", "sensor"]),
    (&["pump", "start", "cmd"], &["run", "cmd"]),
    (&["pump", "spd", "fdbk"], &["speed", "sensor"]),
    // meters
    (&["kw", "demand"], &["elec", "demand", "sensor"]),
    (&["kwh", "total"], &["elec", "energy", "sensor"]),
    // exhaust fans
    (&["fan", "status"], &["fan", "run", "sensor"]),
    (&["fan", "cmd"], &["fan", "run", "cmd"]),
];

/// mock_hospital's `_render_label`, all four styles.
fn render(tokens: &[&str], style: &str) -> String {
    let cap = |t: &str| {
        let mut c = t.chars();
        c.next()
            .map(|f| f.to_uppercase().collect::<String>() + c.as_str())
            .unwrap_or_default()
    };
    match style {
        // mock_hospital joins verbose labels with spaces, then Windrow turns
        // them into underscores for the topic; both tokenize the same.
        "verbose" => tokens.iter().map(|t| cap(t)).collect::<Vec<_>>().join(" "),
        "underscore" => tokens.join("_"),
        "camel" => tokens.iter().map(|t| cap(t)).collect(),
        "cryptic" => {
            let parts: Vec<String> = tokens
                .iter()
                .map(|t| t.chars().take(3).collect::<String>().to_uppercase())
                .collect();
            format!("{}-17", parts.join("-"))
        }
        _ => unreachable!(),
    }
}

#[test]
fn mock_hospital_every_kind_every_style() {
    let t = Tagger::default();
    let mut report = String::new();
    let mut wrong = Vec::new();
    for style in ["verbose", "underscore", "camel", "cryptic"] {
        let (mut right, mut missed) = (0, Vec::new());
        for (tokens, want) in KINDS {
            let label = render(tokens, style);
            let got = t.tag_point(&label, "", None, None);
            let mut g = got.tags.clone();
            g.sort();
            let mut w: Vec<String> = want.iter().map(|s| s.to_string()).collect();
            w.sort();
            if g == w {
                right += 1;
            } else if g.is_empty() {
                missed.push(label);
            } else {
                wrong.push(format!("{label}: got {g:?} ({:?}), want {w:?}", got.rule));
            }
        }
        report += &format!(
            "{style}: {right}/{} right, missed {missed:?}\n",
            KINDS.len()
        );
        let floor = match style {
            // `ZON-AIR-17` (zone airflow, cut to 3 letters) is genuinely
            // ambiguous with zone air temp minus its last word.
            "cryptic" => KINDS.len() - 1,
            _ => KINDS.len(),
        };
        assert!(right >= floor, "{style} below its floor\n{report}");
    }
    assert!(
        wrong.is_empty(),
        "wrong tags:\n{}\n{report}",
        wrong.join("\n")
    );
    println!("{report}");
}

#[test]
fn windrow_21_check_ahu_and_vav_cryptic_and_camel() {
    // Windrow#21's test: ≥80% of the AHU and VAV points named in the
    // cryptic and camel styles get tags, from the label alone.
    let t = Tagger::default();
    let ahu_vav = &KINDS[..15];
    let mut n = 0;
    let mut tagged = 0;
    for style in ["cryptic", "camel"] {
        for (tokens, _) in ahu_vav {
            n += 1;
            if !t
                .tag_point(&render(tokens, style), "", None, None)
                .tags
                .is_empty()
            {
                tagged += 1;
            }
        }
    }
    assert!(tagged * 100 >= n * 95, "{tagged}/{n}");
}

#[test]
fn the_description_rescues_the_ambiguous_cryptic_one() {
    // mock_hospital gives 70% of points a description: the words in full.
    let r = Tagger::default().tag_point("ZON-AIR-17", "Zone airflow", None, None);
    assert_eq!(r.tags, ["air", "flow", "sensor"]);
    // And units do too.
    let r = Tagger::default().tag_point("ZON-AIR-17", "", Some("cubicFeetPerMinute"), None);
    assert_eq!(r.tags, ["air", "flow", "sensor"]);
}
