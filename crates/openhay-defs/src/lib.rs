//! The Project Haystack 4 defs namespace, bundled, plus the three ops that
//! serve it (`defs`, `libs`, `filetypes`) and a filter evaluator that knows
//! it, so `is` subtyping works (`def==^ahu`, `lib==^lib:phIoT`, ...).
//!
//! ```
//! let g = openhay_defs::op("libs", None, None).unwrap();
//! assert_eq!(g.rows.len(), 4);
//! ```

use libhaystack::defs::namespace::Namespace;
use libhaystack::filter::eval::EvalContext;
use libhaystack::filter::path::Path;
use libhaystack::filter::{Eval, Filter, PathResolver};
use libhaystack::val::{Dict, Grid, Ref, Symbol, Value};
use std::sync::LazyLock;

const DEFS_ZINC: &str = include_str!("../defs/ph-4.0.0.zinc");

/// The bundled defs version (the `version` of `lib:ph`).
pub const VERSION: &str = "4.0.0";

#[derive(Debug, thiserror::Error)]
pub enum DefsError {
    #[error("filter: {0}")]
    Filter(String),
    #[error("unknown defs op {0:?}")]
    UnknownOp(String),
}

static GRID: LazyLock<Grid> = LazyLock::new(|| {
    openhay_codec::decode_grid(openhay_codec::Format::Zinc, DEFS_ZINC).expect("bundled defs decode")
});

static NS: LazyLock<Namespace> = LazyLock::new(|| Namespace::make(GRID.clone()));

/// Every def, in the official file's order.
pub fn defs() -> &'static [Dict] {
    &GRID.rows
}

/// The normalized namespace (subtypes, libs, features, ...).
pub fn namespace() -> &'static Namespace {
    &NS
}

/// Paths walk nested dicts; defs refer to each other by Symbol, never Ref,
/// so there is nothing to dereference.
struct DictPaths;

impl PathResolver for DictPaths {
    fn resolve_for(&self, root: &Dict, path: &Path) -> Value {
        let mut cur = Value::Dict(root.clone());
        for seg in path.iter() {
            cur = match &cur {
                Value::Dict(d) => d.get(&seg.to_string()).cloned().unwrap_or(Value::Null),
                _ => return Value::Null,
            };
        }
        cur
    }

    fn resolve(&self, _path: &Path) -> Value {
        Value::Null
    }

    fn resolve_ref(&self, _reference: &Ref) -> Option<Dict> {
        None
    }
}

/// Indices of `rows` that match `filter`, evaluated against the bundled
/// namespace. Refs aren't followed (`a->b` only walks nested dicts).
pub fn filter_indices(filter: &str, rows: &[Dict]) -> Result<Vec<usize>, DefsError> {
    let f = Filter::try_from(filter).map_err(|e| DefsError::Filter(e.to_string()))?;
    Ok(rows
        .iter()
        .enumerate()
        .filter(|(_, d)| f.eval(&EvalContext::make(d, &NS, &DictPaths)))
        .map(|(i, _)| i)
        .collect())
}

/// `d` is a def strictly beneath `base` (the `lib` def itself isn't a lib).
fn under(d: &Dict, base: &str) -> bool {
    match d.get("def") {
        Some(Value::Symbol(s)) => s.value() != base && NS.fits(s, &Symbol::from(base)),
        _ => false,
    }
}

/// The `defs`, `libs` and `filetypes` ops: every def, the lib defs, or the
/// filetype defs, optionally narrowed by a filter and a limit.
pub fn op(name: &str, filter: Option<&str>, limit: Option<usize>) -> Result<Grid, DefsError> {
    let base: Vec<Dict> = match name {
        "defs" => defs().to_vec(),
        "libs" => defs().iter().filter(|d| under(d, "lib")).cloned().collect(),
        "filetypes" => defs()
            .iter()
            .filter(|d| under(d, "filetype"))
            .cloned()
            .collect(),
        other => return Err(DefsError::UnknownOp(other.to_string())),
    };
    let rows: Vec<Dict> = match filter.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => filter_indices(f, &base)?
            .into_iter()
            .map(|i| base[i].clone())
            .collect(),
        None => base,
    };
    let max = limit.unwrap_or(usize::MAX);
    Ok(Grid::make_from_dicts(rows.into_iter().take(max).collect()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(g: &Grid) -> Vec<String> {
        g.rows
            .iter()
            .filter_map(|d| match d.get("def") {
                Some(Value::Symbol(s)) => Some(s.value().to_string()),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn bundled_defs_load() {
        assert_eq!(defs().len(), 719);
        let ph = NS.get_by_name("lib:ph").expect("lib:ph");
        assert_eq!(ph.get("version"), Some(&Value::make_str(VERSION)));
    }

    #[test]
    fn libs_and_filetypes() {
        let mut libs = names(&op("libs", None, None).unwrap());
        libs.sort();
        assert_eq!(libs, ["lib:ph", "lib:phIct", "lib:phIoT", "lib:phScience"]);
        let ft = names(&op("filetypes", None, None).unwrap());
        assert_eq!(
            ft,
            [
                "filetype:csv",
                "filetype:json",
                "filetype:jsonld",
                "filetype:trio",
                "filetype:turtle",
                "filetype:zinc"
            ]
        );
    }

    #[test]
    fn filters_limits_and_errors() {
        let iot = op("defs", Some("lib==^lib:phIoT"), None).unwrap();
        assert!(iot.rows.len() > 100 && iot.rows.len() < 719);
        assert_eq!(
            names(&op("defs", Some("def==^site"), None).unwrap()),
            ["site"]
        );
        assert_eq!(op("defs", None, Some(3)).unwrap().rows.len(), 3);
        assert!(matches!(
            op("defs", Some("a =="), None),
            Err(DefsError::Filter(_))
        ));
        assert!(matches!(
            op("nope", None, None),
            Err(DefsError::UnknownOp(_))
        ));
    }

    #[test]
    fn subtyping_knows_the_namespace() {
        assert!(NS.fits(&Symbol::from("ahu"), &Symbol::from("equip")));
    }
}
