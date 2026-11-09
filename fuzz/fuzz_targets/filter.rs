#![no_main]

//! The filter parser, then the evaluator over a few records with every
//! value kind, so paths, comparisons and Ref lists all get exercised.

use libfuzzer_sys::fuzz_target;
use libhaystack::val::{Dict, Ref};
use openhay_filter::{Filter, Resolver};
use std::collections::HashMap;
use std::sync::OnceLock;

const RECS: &str = r#"ver:"3.0"
id,dis,site,equip,point,siteRef,equipRef,curVal,unit,tz,mod,date,time,geo,refs,tags,on,kind
@s,"HQ",M,,,,,,,"New_York",,,,C(37.5,-77.4),,,,
@e,"AHU-1",,M,,@s,,,,,,,,,[@s],{a:1 b:"x"},,
@p,"DAT",,,M,@s,@e,55.2°F,"°F","New_York",2024-01-02T13:45:00-05:00 New_York,2024-01-02,13:45:00,,[@e,@s],,T,"Number"
@q,"SP",,,M,@s,@e,72,,,,,,,,,F,"Bool"
"#;

struct Recs(HashMap<String, Dict>);

impl Resolver for Recs {
    fn deref(&self, r: &Ref) -> Option<Dict> {
        self.0.get(r.value()).cloned()
    }
}

fn recs() -> &'static (Vec<Dict>, Recs) {
    static R: OnceLock<(Vec<Dict>, Recs)> = OnceLock::new();
    R.get_or_init(|| {
        let g = openhay_codec::decode_grid(openhay_codec::Format::Zinc, RECS).expect("records");
        let by_id = g
            .rows
            .iter()
            .filter_map(|d| match d.get("id") {
                Some(libhaystack::val::Value::Ref(r)) => Some((r.value().to_string(), d.clone())),
                _ => None,
            })
            .collect();
        (g.rows, Recs(by_id))
    })
}

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(f) = Filter::parse(text) else {
        return;
    };
    let _ = f.tags();
    let (rows, cx) = recs();
    for r in rows {
        let _ = f.matches(r, cx);
    }
});
