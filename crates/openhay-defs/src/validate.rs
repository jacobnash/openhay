//! Is a tag set a legal Haystack 4 entity? A deterministic check, for
//! vetting guessed tags (openhay#8): the tags exist, the entity type is
//! clear, choices hold, and the markers form a proto the standard defines.
//!
//! ```
//! use openhay_defs::validate::{validate, Level};
//! let ok = validate(&openhay_defs::validate::markers(&["discharge", "air", "temp", "sensor", "point"]));
//! assert!(ok.is_empty());
//! let bad = validate(&openhay_defs::validate::markers(&["discharge", "air", "temp", "sensor", "cmd", "point"]));
//! assert!(bad.iter().any(|p| p.kind == "choice_conflict" && p.level == Level::Error));
//! ```

use crate::namespace;
use crate::protos::{self, Proto};
use libhaystack::val::{Dict, HaystackDict, Symbol, Value};
use std::collections::BTreeSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// Not a legal Haystack 4 entity: veto it.
    Error,
    /// Legal, but not a shape the standard defines, or a recommended
    /// choice is missing: worth a look.
    Warning,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Error => "error",
            Level::Warning => "warning",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub level: Level,
    /// `unknown_tag`, `unknown_marker`, `no_entity`, `multiple_entities`,
    /// `choice_conflict`, `choice_missing` or `no_proto`.
    pub kind: &'static str,
    /// The tag or choice concerned, if one.
    pub tag: Option<String>,
    /// For a choice: its options present (conflict) or all of them
    /// (missing). For `no_proto`: the nearest protos.
    pub options: Vec<String>,
    pub message: String,
}

/// A dict of marker tags, for callers that only have names.
pub fn markers(names: &[&str]) -> Dict {
    let mut d = Dict::new();
    for n in names {
        d.insert((*n).to_string(), Value::make_marker());
    }
    d
}

fn marker_set(tags: &Dict) -> BTreeSet<String> {
    tags.iter()
        .filter(|(_, v)| matches!(v, Value::Marker))
        .map(|(k, _)| k.to_string())
        .collect()
}

fn def_name(d: &Dict) -> Option<&str> {
    match d.get("def") {
        Some(Value::Symbol(s)) => Some(s.value()),
        _ => None,
    }
}

/// Check `tags` (markers, plus any value tags) against the Haystack 4 defs.
/// Errors first. An empty list means a legal entity whose markers are
/// exactly a standard proto.
pub fn validate(tags: &Dict) -> Vec<Problem> {
    let ns = namespace();
    let mut out = Vec::new();
    let marks = marker_set(tags);

    // 1. Every tag is defined.
    for (k, v) in tags.iter() {
        if v.is_null() || ns.has_name(k) {
            continue;
        }
        let marker = matches!(v, Value::Marker);
        out.push(Problem {
            level: if marker { Level::Error } else { Level::Warning },
            kind: if marker {
                "unknown_marker"
            } else {
                "unknown_tag"
            },
            tag: Some(k.to_string()),
            options: Vec::new(),
            message: if marker {
                format!("`{k}` is not a Haystack 4 marker")
            } else {
                format!("`{k}` is not a Haystack 4 tag (custom tags are allowed)")
            },
        });
    }

    // 2. Exactly one entity type (point, equip, site, space, ...).
    let entities: Vec<&String> = marks
        .iter()
        .filter(|m| {
            ns.get_by_name(m).is_some_and(|d| {
                d.get_list("is")
                    .into_iter()
                    .flatten()
                    .any(|v| v == &Value::make_symbol("entity"))
            })
        })
        .collect();
    match entities.len() {
        0 => out.push(Problem {
            level: Level::Error,
            kind: "no_entity",
            tag: None,
            options: Vec::new(),
            message: "no entity type marker (point, equip, site, space, ...)".into(),
        }),
        1 => {}
        _ => out.push(Problem {
            level: Level::Error,
            kind: "multiple_entities",
            tag: None,
            options: entities.iter().map(|s| s.to_string()).collect(),
            message: format!(
                "more than one entity type: {}",
                entities
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }),
    }

    // 3. Choices that apply to this entity: at most one option, and one
    //    recommended. Choices defined with `of` (pointQuantity of quantity,
    //    pointSubject of phenomenon, ...) aren't checked: their option sets
    //    overlap (a quantity is a phenomenon), so they'd report conflicts
    //    the standard doesn't intend.
    let known = markers_dict(&marks);
    let refl = ns.reflect(&known);
    for d in crate::defs() {
        let Some(name) = def_name(d) else { continue };
        let is_choice = d
            .get_list("is")
            .into_iter()
            .flatten()
            .any(|v| v == &Value::make_symbol("choice"));
        if !is_choice || d.has("of") {
            continue;
        }
        let applies = d.get_list("tagOn").into_iter().flatten().any(|t| match t {
            Value::Symbol(s) => refl.fits(s),
            _ => false,
        });
        if !applies {
            continue;
        }
        let opts: Vec<String> = ns
            .choices_for(&Symbol::from(name))
            .iter()
            .filter_map(|o| def_name(o).map(str::to_string))
            .collect();
        let chosen: Vec<String> = opts
            .iter()
            .filter(|o| refl.fits(&Symbol::from(o.as_str())))
            .cloned()
            .collect();
        if chosen.len() > 1 {
            out.push(Problem {
                level: Level::Error,
                kind: "choice_conflict",
                tag: Some(name.to_string()),
                message: format!(
                    "`{name}` allows one of its options, found {}",
                    chosen.join(", ")
                ),
                options: chosen,
            });
        } else if chosen.is_empty() {
            out.push(Problem {
                level: Level::Warning,
                kind: "choice_missing",
                tag: Some(name.to_string()),
                message: format!("no `{name}` option ({})", opts.join(", ")),
                options: opts,
            });
        }
    }

    // 4. The markers (the defined ones) are a standard proto.
    let defined: BTreeSet<String> = marks.iter().filter(|m| ns.has_name(m)).cloned().collect();
    let shaped = entities.len() == 1 && !defined.is_empty();
    if shaped && !is_proto(&defined) {
        let near: Vec<String> = nearest(&defined, 3).iter().map(|p| p.join(" ")).collect();
        out.push(Problem {
            level: Level::Warning,
            kind: "no_proto",
            tag: None,
            message: format!(
                "`{}` is not a Haystack 4 proto; nearest: {}",
                defined.iter().cloned().collect::<Vec<_>>().join(" "),
                near.join(" | ")
            ),
            options: near,
        });
    }

    out.sort_by_key(|p| p.level);
    out
}

fn markers_dict(m: &BTreeSet<String>) -> Dict {
    let mut d = Dict::new();
    for k in m {
        d.insert(k.clone(), Value::make_marker());
    }
    d
}

fn is_proto(m: &BTreeSet<String>) -> bool {
    let v: Proto = m.iter().cloned().collect();
    protos::all().binary_search(&v).is_ok()
}

/// Protos the tags fit: every proto whose markers are all present, most
/// specific (most markers) first. The first is exact when the tag set is
/// itself a proto.
pub fn protos_matching(tags: &Dict) -> Vec<&'static Proto> {
    let m = marker_set(tags);
    let mut v: Vec<&Proto> = protos::all()
        .iter()
        .filter(|p| p.iter().all(|t| m.contains(t)))
        .collect();
    v.sort_by(|a, b| b.len().cmp(&a.len()).then(a.cmp(b)));
    v
}

/// The `n` protos closest to the tags: fewest markers to add or remove,
/// then fewest to remove. A suggestion list for a tag set that isn't one.
pub fn nearest_protos(tags: &Dict, n: usize) -> Vec<&'static Proto> {
    nearest(&marker_set(tags), n)
}

fn nearest(m: &BTreeSet<String>, n: usize) -> Vec<&'static Proto> {
    let mut v: Vec<(usize, usize, &Proto)> = protos::all()
        .iter()
        .map(|p| {
            let missing = p.iter().filter(|t| !m.contains(*t)).count();
            let extra = m.iter().filter(|t| !p.contains(t)).count();
            (missing + extra, extra, p)
        })
        .collect();
    v.sort();
    v.into_iter().take(n).map(|(_, _, p)| p).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(names: &[&str]) -> Vec<Problem> {
        validate(&markers(names))
    }

    fn kinds(names: &[&str]) -> Vec<&'static str> {
        check(names).iter().map(|p| p.kind).collect()
    }

    #[test]
    fn legal_protos_pass_clean() {
        for t in [
            "discharge air temp sensor point",
            "zone air temp sensor point",
            "chilled water leaving temp sensor point",
            "discharge fan run sensor point",
            "zone air temp occ cooling sp point",
            "zone air co2 concentration sensor point",
            "run cmd point",
        ] {
            let names: Vec<&str> = t.split(' ').collect();
            assert_eq!(check(&names), vec![], "{t}");
        }
        // Equipment gets the standard's recommended choices as warnings
        // (ductConfig, coolingProcess, ...), never errors.
        for t in ["ahu equip", "vav equip", "site"] {
            let names: Vec<&str> = t.split(' ').collect();
            let p = check(&names);
            assert!(p.iter().all(|p| p.level == Level::Warning), "{t}: {p:?}");
        }
        let vav = check(&["vav", "equip"]);
        assert!(
            vav.iter()
                .any(|p| p.tag.as_deref() == Some("vavModulation")),
            "{vav:?}"
        );
    }

    #[test]
    fn unknown_tags() {
        let p = check(&["zone", "air", "tmep", "sensor", "point"]);
        assert_eq!(p[0].kind, "unknown_marker");
        assert_eq!(p[0].tag.as_deref(), Some("tmep"));
        assert_eq!(p[0].level, Level::Error);
        // A custom value tag is allowed, with a warning.
        let mut d = markers(&["zone", "air", "temp", "sensor", "point"]);
        d.insert("windrowTagRule".into(), Value::make_str("zone-air-temp"));
        d.insert("dis".into(), Value::make_str("Zone temp"));
        let p = validate(&d);
        assert_eq!(p.len(), 1);
        assert_eq!((p[0].kind, p[0].level), ("unknown_tag", Level::Warning));
    }

    #[test]
    fn entity_type() {
        assert!(kinds(&["discharge", "air", "temp", "sensor"]).contains(&"no_entity"));
        assert!(kinds(&["point", "equip", "temp"]).contains(&"multiple_entities"));
    }

    #[test]
    fn choices() {
        let p = check(&["zone", "air", "temp", "sensor", "sp", "point"]);
        let c = p.iter().find(|p| p.kind == "choice_conflict").unwrap();
        assert_eq!(c.tag.as_deref(), Some("pointFunction"));
        assert_eq!(c.options, ["sensor", "sp"]);
        assert_eq!(c.level, Level::Error);
        let p = check(&["zone", "air", "temp", "point"]);
        let m = p.iter().find(|p| p.kind == "choice_missing").unwrap();
        assert_eq!(m.tag.as_deref(), Some("pointFunction"));
        assert_eq!(m.level, Level::Warning);
        // Choices on conjunct entities apply too.
        assert!(
            check(&[
                "chiller",
                "chiller-absorption",
                "absorption",
                "centrifugal",
                "equip"
            ])
            .iter()
            .any(|p| p.kind == "choice_conflict" && p.tag.as_deref() == Some("chillerMechanism"))
        );
    }

    #[test]
    fn not_a_proto_is_a_warning_with_suggestions() {
        let p = check(&["zone", "co2", "sensor", "point"]);
        assert_eq!(p.len(), 1, "{p:?}");
        assert_eq!((p[0].kind, p[0].level), ("no_proto", Level::Warning));
        assert!(
            p[0].options
                .iter()
                .any(|o| o.contains("co2") && o.contains("concentration")),
            "{:?}",
            p[0].options
        );
    }

    #[test]
    fn protos_matching_most_specific_first() {
        let m = protos_matching(&markers(&[
            "discharge",
            "air",
            "temp",
            "sensor",
            "point",
            "ahu",
        ]));
        assert_eq!(m[0].join(" "), "air discharge point sensor temp");
        assert!(m.iter().all(|p| p.len() <= m[0].len()));
        assert!(protos_matching(&markers(&["nothing"])).is_empty());
    }

    #[test]
    fn errors_sort_first() {
        let p = check(&["zone", "bogus", "temp", "sensor", "cmd", "point"]);
        let levels: Vec<Level> = p.iter().map(|p| p.level).collect();
        let mut sorted = levels.clone();
        sorted.sort();
        assert_eq!(levels, sorted);
        assert_eq!(levels[0], Level::Error);
    }
}
