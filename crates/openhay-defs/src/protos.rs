//! Haystack 4 protos, computed from the bundled defs: every entity shape the
//! standard defines (`discharge air temp sensor point`, `chilled water
//! leaving pipe equip`, ...), as sets of marker tags.
//!
//! A def's `children` are the protos under an entity that has that tag. The
//! def's `childrenFlatten` names the parent tags that carry down into each
//! child: a `discharge duct` gives `discharge air temp sensor point`.
//! Conjuncts count as tags on both sides. A `fan-motor`'s children are
//! protos, and a parent with `chilled` and `water` has the `chilled-water`
//! fluid that a `pipe` flattens into its children. libhaystack's
//! `Namespace::protos` looks at single tags only, so it misses both.
//!
//! The full set is the closure of children from every entity type
//! (`ahu` with its mandatory `equip`, `site`, ...) down.

use crate::namespace;
use libhaystack::val::{Dict, HaystackDict, Symbol, Value};
use std::collections::{BTreeSet, VecDeque};
use std::sync::LazyLock;

/// A proto: its marker tags, sorted.
pub type Proto = Vec<String>;

static ALL: LazyLock<Vec<Proto>> = LazyLock::new(compute_all);

/// Every proto the bundled defs define, each as sorted marker names.
pub fn all() -> &'static [Proto] {
    &ALL
}

/// Conjunct def names (`chilled-water`) whose parts are all in `markers`.
fn conjuncts_in(markers: &BTreeSet<String>) -> Vec<String> {
    crate::defs()
        .iter()
        .filter_map(|d| match d.get("def") {
            Some(Value::Symbol(s)) if s.value().contains('-') && !s.value().contains(':') => {
                Some(s.value().to_string())
            }
            _ => None,
        })
        .filter(|name| name.split('-').all(|p| markers.contains(p)))
        .collect()
}

fn children_of(def: &Dict) -> Vec<Dict> {
    match def.get("children") {
        Some(Value::List(l)) => l
            .iter()
            .filter_map(|v| match v {
                Value::Dict(d) => Some(d.clone()),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    }
}

/// The protos directly under `parent` (a set of marker names).
pub fn children(parent: &BTreeSet<String>) -> Vec<BTreeSet<String>> {
    let ns = namespace();
    let mut names: Vec<String> = parent.iter().cloned().collect();
    names.extend(conjuncts_in(parent));
    let mut out = Vec::new();
    for name in &names {
        let Some(def) = ns.get_by_name(name) else {
            continue;
        };
        let kids = children_of(def);
        if kids.is_empty() {
            continue;
        }
        // Parent tags (or conjuncts) that fit a childrenFlatten def.
        let mut flat: BTreeSet<String> = BTreeSet::new();
        for f in def.get_list("childrenFlatten").into_iter().flatten() {
            let Value::Symbol(f) = f else { continue };
            for n in &names {
                if ns.fits(&Symbol::from(n.as_str()), f) {
                    flat.extend(n.split('-').map(str::to_string));
                }
            }
        }
        for k in kids {
            let mut p: BTreeSet<String> = k
                .iter()
                .filter(|(_, v)| matches!(v, Value::Marker))
                .map(|(k, _)| k.to_string())
                .collect();
            p.extend(flat.iter().cloned());
            out.push(p);
        }
    }
    out
}

fn compute_all() -> Vec<Proto> {
    let ns = namespace();
    let entity = Symbol::from("entity");
    let mut seen: BTreeSet<BTreeSet<String>> = BTreeSet::new();
    let mut queue: VecDeque<(BTreeSet<String>, usize)> = VecDeque::new();
    for d in crate::defs() {
        let Some(Value::Symbol(s)) = d.get("def") else {
            continue;
        };
        if s.value() == "entity" || s.value().contains(':') || !ns.fits(s, &entity) {
            continue;
        }
        let mut p: BTreeSet<String> = s.value().split('-').map(str::to_string).collect();
        for sup in ns.inheritance(s) {
            if sup.has("mandatory")
                && let Some(Value::Symbol(m)) = sup.get("def")
            {
                p.insert(m.value().to_string());
            }
        }
        queue.push_back((p, 0));
    }
    // Protos nest a few levels (ahu > discharge duct > ... ); the depth cap
    // only guards against a cycle in future defs.
    while let Some((p, depth)) = queue.pop_front() {
        for c in children(&p) {
            if seen.insert(c.clone()) && depth < 8 {
                queue.push_back((c, depth + 1));
            }
        }
    }
    seen.into_iter().map(|s| s.into_iter().collect()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has(t: &str) -> bool {
        let mut v: Proto = t.split(' ').map(str::to_string).collect();
        v.sort();
        all().binary_search(&v).is_ok()
    }

    #[test]
    fn closure_finds_flattened_and_conjunct_protos() {
        assert!(all().len() > 900, "{}", all().len());
        assert!(
            all().windows(2).all(|w| w[0] < w[1]),
            "sorted, no duplicates"
        );
        // duct children flattened with the duct's section
        assert!(has("discharge air temp sensor point"));
        assert!(has("return air temp sensor point"));
        // pipe children flattened with the `chilled-water` conjunct fluid
        assert!(has("chilled water leaving temp sensor point"));
        assert!(has("hot water entering temp sensor point"));
        // children of a conjunct def (`fan-motor`), flattened with its fan
        assert!(has("discharge fan run sensor point"));
        assert!(has("outside damper cmd point"));
        assert!(has("zone air temp occ cooling sp point"));
        assert!(has("zone air co2 concentration sensor point"));
        // and what Haystack 4 doesn't define
        assert!(!has("zone co2 sensor point"));
        assert!(!has("temp point"));
    }
}
