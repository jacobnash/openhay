//! Messy BAS point names to Haystack tags, deterministically: no model, no
//! learning, the same answer every time.
//!
//! ```
//! let t = openhay_tag::Tagger::default();
//! let r = t.tag_point("CHWSupTmp3", "", None, None);
//! assert_eq!(r.rule.as_deref(), Some("chilled-water-supply-temp"));
//! assert!(r.tags.contains(&"leaving".to_string()));
//! assert_eq!(r.tokens, ["chw", "sup", "tmp"]);
//! ```
//!
//! Four parts, three of them plain data you can read and extend:
//!
//! * [`tokenize`] splits on delimiters, camelCase and digit boundaries
//!   (`CHWSupTmp3` becomes `chw sup tmp 3`).
//! * `data/lexicon.toml` maps BAS abbreviations to canonical words
//!   (`sat` is supply air temp, `stpt` is sp, `sta` is status, static or
//!   start).
//! * `data/units.toml` maps BACnet engineering units to Haystack units, and
//!   Haystack units to the words they're evidence for (°F is temp).
//! * `data/rules.toml` maps canonical words to Haystack 4 tags. The rule
//!   that explains the most of the name wins, and the result carries the
//!   rule id and the tokens it matched as evidence.
//!
//! Per-site overrides layer on top with [`Tagger::with_overrides`], in the
//! same TOML shapes.

use serde::Deserialize;
use std::collections::{BTreeMap, HashMap};

const LEXICON: &str = include_str!("../data/lexicon.toml");
const RULES: &str = include_str!("../data/rules.toml");
const UNITS: &str = include_str!("../data/units.toml");

/// The function tags; a result without one gets it from the object type.
const FUNCTIONS: [&str; 3] = ["sensor", "cmd", "sp"];

#[derive(Debug, thiserror::Error)]
pub enum TagError {
    #[error("bad tagger data: {0}")]
    Data(#[from] toml::de::Error),
    #[error("bad tagger data: {0}")]
    Invalid(String),
}

#[derive(Debug, Clone, Deserialize)]
pub struct Rule {
    pub id: String,
    /// Canonical words that must all be present.
    #[serde(rename = "match")]
    pub words: Vec<String>,
    /// Haystack marker tags it produces (empty: claims the words, no tags).
    pub tags: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
struct Data {
    #[serde(default)]
    words: BTreeMap<String, Vec<String>>,
    #[serde(default)]
    rule: Vec<Rule>,
    #[serde(default)]
    bacnet: BTreeMap<String, String>,
    #[serde(default)]
    signal: BTreeMap<String, Vec<String>>,
}

/// What [`Tagger::tag_point`] concluded and why.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TagResult {
    /// Haystack marker tags, in rule order (empty: no rule matched).
    pub tags: Vec<String>,
    /// The id of the rule that produced them.
    pub rule: Option<String>,
    /// The name tokens (from label and description) the rule matched.
    pub tokens: Vec<String>,
    /// Other evidence it used: `units:°F`, `object:analog-input`.
    pub signals: Vec<String>,
}

/// The tokenizer, lexicon, unit table and rules, ready to tag points.
#[derive(Debug, Clone)]
pub struct Tagger {
    words: HashMap<String, Vec<String>>,
    rules: Vec<Rule>,
    bacnet: HashMap<String, String>,
    signal: HashMap<String, Vec<String>>,
    /// Every word some rule matches on, which stands for itself.
    vocab: std::collections::HashSet<String>,
}

impl Default for Tagger {
    /// The bundled lexicon, units and rules.
    fn default() -> Self {
        let mut t = Self {
            words: HashMap::new(),
            rules: Vec::new(),
            bacnet: HashMap::new(),
            signal: HashMap::new(),
            vocab: Default::default(),
        };
        for src in [LEXICON, RULES, UNITS] {
            t.layer(src).expect("bundled tagger data is valid");
        }
        t
    }
}

impl Tagger {
    /// The defaults with site overrides on top. `overrides` is TOML in the
    /// shapes of the bundled files (`[words]`, `[[rule]]`, `[bacnet]`,
    /// `[signal]`), all optional. A word entry replaces the default and an
    /// empty list deletes it. A rule with an existing id replaces that rule,
    /// and a new id is added after the defaults.
    pub fn with_overrides(overrides: &str) -> Result<Self, TagError> {
        let mut t = Self::default();
        t.layer(overrides)?;
        Ok(t)
    }

    fn layer(&mut self, src: &str) -> Result<(), TagError> {
        let d: Data = toml::from_str(src)?;
        for (k, v) in d.words {
            let k = k.to_lowercase();
            if v.is_empty() {
                self.words.remove(&k);
            } else {
                self.words.insert(k, v);
            }
        }
        for r in d.rule {
            if r.words.is_empty() {
                return Err(TagError::Invalid(format!("rule {} matches nothing", r.id)));
            }
            match self.rules.iter_mut().find(|x| x.id == r.id) {
                Some(x) => *x = r,
                None => self.rules.push(r),
            }
        }
        for (k, v) in d.bacnet {
            self.bacnet.insert(norm_unit(&k), v);
        }
        self.signal.extend(d.signal);
        self.vocab = self.rules.iter().flat_map(|r| r.words.clone()).collect();
        Ok(())
    }

    /// The rules in match order, defaults first.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// The canonical words a token stands for (empty: none known).
    pub fn words_for(&self, token: &str) -> Vec<String> {
        if let Some(w) = self.words.get(token) {
            return w.clone();
        }
        if self.vocab.contains(token) {
            return vec![token.to_string()];
        }
        Vec::new()
    }

    /// A BACnet engineering unit (`degreesFahrenheit`, `degrees-fahrenheit`)
    /// or a Haystack unit (`°F`) as the Haystack unit symbol.
    pub fn haystack_unit(&self, units: &str) -> Option<String> {
        if self.signal.contains_key(units) || self.bacnet.values().any(|v| v == units) {
            return Some(units.to_string());
        }
        self.bacnet.get(&norm_unit(units)).cloned()
    }

    /// Tag one point from its label, description, units (BACnet or
    /// Haystack) and BACnet object type (`analog-input`, `analogInput`,
    /// `AI`). Only the label is required.
    pub fn tag_point(
        &self,
        label: &str,
        description: &str,
        units: Option<&str>,
        object_type: Option<&str>,
    ) -> TagResult {
        // Name tokens, each with the canonical words it stands for.
        let mut names: Vec<(String, Vec<String>)> = Vec::new();
        for text in [label, description] {
            for tok in tokenize_with(text, |t| !self.words_for(t).is_empty()) {
                let w = self.words_for(&tok);
                if !names.iter().any(|(t, _)| *t == tok) {
                    names.push((tok, w));
                }
            }
        }
        // Signals: what the units say.
        let mut signals: Vec<(String, Vec<String>)> = Vec::new();
        if let Some(u) = units.and_then(|u| self.haystack_unit(u))
            && let Some(w) = self.signal.get(&u)
        {
            signals.push((format!("units:{u}"), w.clone()));
        }

        // (rule, score, name tokens it used, signals it used)
        type Candidate<'a> = (&'a Rule, (usize, usize, usize), Vec<usize>, Vec<usize>);
        let mut best: Option<Candidate> = None;
        for rule in &self.rules {
            let mut name_hits: Vec<usize> = Vec::new();
            let mut sig_hits: Vec<usize> = Vec::new();
            let mut ok = true;
            for w in &rule.words {
                let n: Vec<usize> = (0..names.len())
                    .filter(|&i| names[i].1.contains(w))
                    .collect();
                if !n.is_empty() {
                    name_hits.extend(n);
                    continue;
                }
                let s: Vec<usize> = (0..signals.len())
                    .filter(|&i| signals[i].1.contains(w))
                    .collect();
                if s.is_empty() {
                    ok = false;
                    break;
                }
                sig_hits.extend(s);
            }
            if !ok {
                continue;
            }
            name_hits.sort_unstable();
            name_hits.dedup();
            sig_hits.sort_unstable();
            sig_hits.dedup();
            let score = (name_hits.len(), rule.words.len(), sig_hits.len());
            if best.as_ref().is_none_or(|b| score > b.1) {
                best = Some((rule, score, name_hits, sig_hits));
            }
        }

        let Some((rule, _, name_hits, sig_hits)) = best else {
            return TagResult::default();
        };
        let mut tags = rule.tags.clone();
        let mut sigs: Vec<String> = sig_hits.iter().map(|&i| signals[i].0.clone()).collect();
        if !tags.is_empty()
            && !tags.iter().any(|t| FUNCTIONS.contains(&t.as_str()))
            && let Some((f, ot)) = object_type.and_then(|o| function_of(o).map(|f| (f, o)))
        {
            tags.push(f.to_string());
            sigs.push(format!("object:{ot}"));
        }
        TagResult {
            tags,
            rule: Some(rule.id.clone()),
            tokens: name_hits.iter().map(|&i| names[i].0.clone()).collect(),
            signals: sigs,
        }
    }
}

/// `sensor` for a BACnet input object, `cmd` for an output, None for a
/// value object (which can be either, or a setpoint).
pub fn function_of(object_type: &str) -> Option<&'static str> {
    let o: String = object_type
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    match o.as_str() {
        "ai" | "bi" | "mi" | "analoginput" | "binaryinput" | "multistateinput" => Some("sensor"),
        "ao" | "bo" | "mo" | "analogoutput" | "binaryoutput" | "multistateoutput" => Some("cmd"),
        _ => None,
    }
}

fn norm_unit(u: &str) -> String {
    u.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Split a BAS point name into lowercase tokens: on any non-alphanumeric
/// character, at camelCase humps (`SupTmp`), after an acronym
/// (`CHWSup` is `chw sup`), and between letters and digits (`AHU1`).
///
/// ```
/// assert_eq!(openhay_tag::tokenize("CHWSupTmp3"), ["chw", "sup", "tmp", "3"]);
/// assert_eq!(openhay_tag::tokenize("AHU-1_ZN-T"), ["ahu", "1", "zn", "t"]);
/// assert_eq!(openhay_tag::tokenize("ZoneCO2"), ["zone", "co2"]);
/// ```
///
/// A split inside one chunk that would break a known term is undone:
/// `CO2` stays `co2` and `kWh` stays `kwh`. [`Tagger::tag_point`] uses its
/// own lexicon for that; this function uses the bundled one.
pub fn tokenize(text: &str) -> Vec<String> {
    let t = default_tagger();
    tokenize_with(text, |s| !t.words_for(s).is_empty())
}

fn default_tagger() -> &'static Tagger {
    static T: std::sync::OnceLock<Tagger> = std::sync::OnceLock::new();
    T.get_or_init(Tagger::default)
}

fn tokenize_with(text: &str, known: impl Fn(&str) -> bool) -> Vec<String> {
    let mut out = Vec::new();
    for chunk in text.split(|c: char| !c.is_alphanumeric()) {
        if chunk.is_empty() {
            continue;
        }
        let parts = split_chunk(chunk);
        // Rejoin neighbours whose concatenation is a known term and at
        // least one of which isn't (`co` + `2`, `k` + `wh`).
        let mut i = 0;
        while i < parts.len() {
            let mut took = 1;
            for n in (2..=3.min(parts.len() - i)).rev() {
                let joined: String = parts[i..i + n].concat();
                if known(&joined) && !parts[i..i + n].iter().all(|p| known(p)) {
                    out.push(joined);
                    took = n;
                    break;
                }
            }
            if took == 1 {
                out.push(parts[i].clone());
            }
            i += took;
        }
    }
    out
}

fn split_chunk(chunk: &str) -> Vec<String> {
    let cs: Vec<char> = chunk.chars().collect();
    let mut parts = Vec::new();
    let mut cur = String::new();
    for i in 0..cs.len() {
        let c = cs[i];
        if i > 0 {
            let p = cs[i - 1];
            let next_lower = cs.get(i + 1).is_some_and(|n| n.is_lowercase());
            let boundary = (p.is_lowercase() && c.is_uppercase())
                || (p.is_alphabetic() && c.is_numeric())
                || (p.is_numeric() && c.is_alphabetic())
                || (p.is_uppercase() && c.is_uppercase() && next_lower);
            if boundary && !cur.is_empty() {
                parts.push(std::mem::take(&mut cur));
            }
        }
        cur.extend(c.to_lowercase());
    }
    if !cur.is_empty() {
        parts.push(cur);
    }
    parts
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tag(label: &str) -> TagResult {
        Tagger::default().tag_point(label, "", None, None)
    }

    #[test]
    fn tokenizer() {
        assert_eq!(tokenize("CHWSupTmp3"), ["chw", "sup", "tmp", "3"]);
        assert_eq!(tokenize("SupplyAirTemp"), ["supply", "air", "temp"]);
        assert_eq!(tokenize("SUP-AIR-TEM-12"), ["sup", "air", "tem", "12"]);
        assert_eq!(tokenize("AHU1SAT"), ["ahu", "1", "sat"]);
        assert_eq!(tokenize("zone_air temp,x"), ["zone", "air", "temp", "x"]);
        assert_eq!(tokenize("ZoneCo2"), ["zone", "co2"]);
        assert_eq!(tokenize("KwhTotal"), ["kwh", "total"]);
        assert_eq!(tokenize(""), Vec::<String>::new());
        assert_eq!(tokenize("--__"), Vec::<String>::new());
        assert_eq!(tokenize("Température"), ["température"]);
    }

    #[test]
    fn the_issues_examples() {
        let r = tag("CHWSupTmp3");
        assert_eq!(r.tags, ["chilled", "water", "leaving", "temp", "sensor"]);
        assert_eq!(r.tokens, ["chw", "sup", "tmp"]);
        assert_eq!(tag("AHU1-SAT").rule.as_deref(), Some("supply-air-temp"));
        assert_eq!(tag("ZN-T-3").tags, ["zone", "air", "temp", "sensor"]);
        assert_eq!(tag("ZN-T-STPT").tags, ["zone", "air", "temp", "sp"]);
        assert_eq!(tag("DMPR-FDBK").tags, ["damper", "sensor"]);
    }

    #[test]
    fn ambiguous_tokens_resolve_by_coverage() {
        assert_eq!(
            tag("SUP-FAN-STA").rule.as_deref(),
            Some("supply-fan-status")
        );
        assert_eq!(tag("STA-PRE").rule.as_deref(), Some("static-pressure"));
        assert_eq!(tag("CHI-STA-CMD").tags, ["run", "cmd"]);
        // `rat` alone is return air temp; with `fir` it's a firing rate,
        // which has no Haystack tags (and isn't a wrong guess).
        assert_eq!(tag("AHU-RAT").rule.as_deref(), Some("return-air-temp"));
        let fr = tag("FIR-RAT-5");
        assert_eq!(fr.rule.as_deref(), Some("boiler-firing-rate"));
        assert!(fr.tags.is_empty());
    }

    #[test]
    fn units_and_object_type_are_evidence() {
        let t = Tagger::default();
        assert!(t.tag_point("AI-17", "", None, None).tags.is_empty());
        let r = t.tag_point("AI-17", "", Some("degreesFahrenheit"), None);
        assert_eq!(r.tags, ["temp", "sensor"]);
        assert_eq!(r.signals, ["units:°F"]);
        assert!(r.tokens.is_empty());
        // The name wins over a unit, and a unit completes a name.
        let r = t.tag_point("ZN-3", "", Some("°F"), None);
        assert_eq!(r.rule.as_deref(), Some("zone-air-temp"));
        assert_eq!(r.tokens, ["zn"]);
        let r = t.tag_point("VAV-2-AF", "", Some("cubicFeetPerMinute"), None);
        assert_eq!(r.tags, ["air", "flow", "sensor"]);
        // An object type supplies a missing function tag.
        let r = t.tag_point("DMPR-3", "", None, Some("analog-output"));
        assert_eq!(r.tags, ["damper", "cmd"]);
        assert_eq!(r.signals, ["object:analog-output"]);
        assert_eq!(
            t.tag_point("DMPR-3", "", None, Some("AI")).tags,
            ["damper", "sensor"]
        );
        assert_eq!(
            t.tag_point("DMPR-3", "", None, Some("analog-value")).tags,
            ["damper"]
        );
    }

    #[test]
    fn description_helps_a_cryptic_label() {
        let t = Tagger::default();
        assert!(t.tag_point("AI-4", "", None, None).tags.is_empty());
        let r = t.tag_point("AI-4", "Outside air temperature", None, None);
        assert_eq!(r.rule.as_deref(), Some("outside-air-temp"));
        assert_eq!(r.tokens, ["outside", "temperature"]);
    }

    #[test]
    fn unknown_names_get_nothing() {
        for l in ["", "AI-17", "Point 42", "XYZZY", "ahu-1"] {
            assert_eq!(tag(l), TagResult::default(), "{l}");
        }
    }

    #[test]
    fn haystack_units() {
        let t = Tagger::default();
        assert_eq!(t.haystack_unit("degreesFahrenheit").as_deref(), Some("°F"));
        assert_eq!(t.haystack_unit("degrees-fahrenheit").as_deref(), Some("°F"));
        assert_eq!(
            t.haystack_unit("percentRelativeHumidity").as_deref(),
            Some("%RH")
        );
        assert_eq!(t.haystack_unit("°C").as_deref(), Some("°C"));
        assert_eq!(t.haystack_unit("noUnits"), None);
    }

    #[test]
    fn site_overrides() {
        let t = Tagger::with_overrides(
            r#"
            [words]
            sat = []             # this site's SAT is a saturation temp; forget it
            lat = ["leaving", "air", "temp"]

            [[rule]]
            id = "leaving-air-temp"
            match = ["leaving", "air", "temp"]
            tags = ["discharge", "air", "temp", "sensor"]

            [[rule]]
            id = "zone-air-temp"   # replaces the default
            match = ["zone", "temp"]
            tags = ["zone", "air", "temp", "sensor", "space"]
            "#,
        )
        .unwrap();
        assert!(t.tag_point("SAT", "", None, None).tags.is_empty());
        assert_eq!(
            t.tag_point("AHU1-LAT", "", None, None).rule.as_deref(),
            Some("leaving-air-temp")
        );
        assert!(
            t.tag_point("ZN-T", "", None, None)
                .tags
                .contains(&"space".to_string())
        );
        assert_eq!(t.rules().len(), Tagger::default().rules().len() + 1);
        assert!(Tagger::with_overrides("[[rule]]\nid='x'\nmatch=[]\ntags=[]").is_err());
        assert!(Tagger::with_overrides("not toml [").is_err());
    }

    #[test]
    fn every_rule_word_is_reachable_and_every_tag_is_lowercase_camel() {
        let t = Tagger::default();
        for r in t.rules() {
            for w in &r.words {
                assert_eq!(
                    t.words_for(w),
                    std::slice::from_ref(w),
                    "rule {} word {w}",
                    r.id
                );
            }
            for tag in &r.tags {
                assert!(tag.chars().next().unwrap().is_ascii_lowercase(), "{}", r.id);
                assert_ne!(tag, "supply", "{}: Haystack 4 uses discharge/leaving", r.id);
            }
        }
        // Lexicon expansions only produce words some rule understands.
        for (k, ws) in &t.words {
            for w in ws {
                assert!(t.vocab.contains(w), "lexicon {k} -> {w}: no rule uses {w}");
            }
        }
    }
}
