//! An in-memory [`Backend`]: load records from Trio/Zinc and you have a
//! working Haystack server. Used by `hay serve`, tests and demos.

use crate::backend::{Backend, HisItem, OpError, OpResult, PointWrite};
use libhaystack::val::{DateTime, Dict, Grid, HaystackDict, Ref, Value};
use std::collections::HashMap;
use std::sync::RwLock;

pub const PRIORITY_LEVELS: usize = 17; // index 1..=16 used; 0 unused

#[derive(Default)]
struct State {
    order: Vec<String>,
    recs: HashMap<String, Dict>,
    his: HashMap<String, Vec<HisItem>>,
    priority: HashMap<String, Vec<Option<(Value, String)>>>,
}

#[derive(Default)]
pub struct MemBackend {
    state: RwLock<State>,
    about: Dict,
}

pub fn rec_id(d: &Dict) -> Option<String> {
    match d.get("id") {
        Some(Value::Ref(r)) => Some(r.value().to_string()),
        _ => None,
    }
}

struct Resolver<'a>(&'a HashMap<String, Dict>);

impl openhay_filter::Resolver for Resolver<'_> {
    fn deref(&self, r: &Ref) -> Option<Dict> {
        self.0.get(r.value()).cloned()
    }
}

impl MemBackend {
    pub fn new(records: Vec<Dict>) -> Self {
        let b = Self::default();
        for r in records {
            b.upsert(r);
        }
        b
    }

    pub fn with_about(mut self, about: Dict) -> Self {
        self.about = about;
        self
    }

    /// Insert or replace a record. Records without an `id` get a generated one.
    pub fn upsert(&self, mut rec: Dict) -> String {
        let id = rec_id(&rec).unwrap_or_else(|| {
            let v = Value::make_ref_gen();
            let id = match &v {
                Value::Ref(r) => r.value().to_string(),
                _ => unreachable!(),
            };
            rec.insert("id".into(), v);
            id
        });
        let mut s = self.state.write().unwrap();
        if !s.recs.contains_key(&id) {
            s.order.push(id.clone());
        }
        s.recs.insert(id.clone(), rec);
        id
    }

    /// Set a tag on a record (e.g. `curVal` from a live feed).
    pub fn set_tag(&self, id: &str, tag: &str, val: Value) -> bool {
        let mut s = self.state.write().unwrap();
        match s.recs.get_mut(id) {
            Some(r) => {
                r.insert(tag.into(), val);
                true
            }
            None => false,
        }
    }

    pub fn len(&self) -> usize {
        self.state.read().unwrap().order.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

fn level_dis(level: usize) -> String {
    match level {
        1 => "1 (emergency)".into(),
        8 => "8 (manual)".into(),
        16 => "16 (default)".into(),
        l => l.to_string(),
    }
}

impl Backend for MemBackend {
    fn about(&self) -> Dict {
        self.about.clone()
    }

    async fn read_filter(&self, filter: &str, limit: Option<usize>) -> OpResult<Vec<Dict>> {
        let f = openhay_filter::Filter::parse(filter)
            .map_err(|e| OpError::BadRequest(e.to_string()))?;
        let s = self.state.read().unwrap();
        let res = Resolver(&s.recs);
        let max = limit.unwrap_or(usize::MAX);
        Ok(s.order
            .iter()
            .filter_map(|id| s.recs.get(id))
            .filter(|d| f.matches(d, &res))
            .take(max)
            .cloned()
            .collect())
    }

    async fn read_ids(&self, ids: &[Ref]) -> OpResult<Vec<Option<Dict>>> {
        let s = self.state.read().unwrap();
        Ok(ids.iter().map(|r| s.recs.get(r.value()).cloned()).collect())
    }

    async fn nav(&self, nav_id: Option<&str>) -> OpResult<Grid> {
        // site -> equip -> point
        let (filter, parent_tag) = match nav_id {
            None => ("site".to_string(), None),
            Some(id) => {
                let s = self.state.read().unwrap();
                let rec = s
                    .recs
                    .get(id)
                    .ok_or_else(|| OpError::UnknownRec(id.into()))?;
                if rec.has("site") {
                    ("equip".to_string(), Some(("siteRef", id.to_string())))
                } else if rec.has("equip") {
                    ("point".to_string(), Some(("equipRef", id.to_string())))
                } else {
                    return Ok(Grid::make_empty());
                }
            }
        };
        let rows = self.read_filter(&filter, None).await?;
        let rows: Vec<Dict> = rows
            .into_iter()
            .filter(|r| match &parent_tag {
                None => true,
                Some((tag, id)) => matches!(r.get(tag), Some(Value::Ref(x)) if x.value() == id),
            })
            .map(|mut r| {
                if !r.has("point")
                    && let Some(id) = rec_id(&r)
                {
                    r.insert("navId".into(), Value::make_str(&id));
                }
                r
            })
            .collect();
        Ok(if rows.is_empty() {
            Grid::make_empty()
        } else {
            Grid::make_from_dicts(rows)
        })
    }

    async fn his_read(
        &self,
        rec: &Dict,
        start: &DateTime,
        end: &DateTime,
    ) -> OpResult<Vec<HisItem>> {
        let id = rec_id(rec).ok_or_else(|| OpError::BadRequest("rec has no id".into()))?;
        let s = self.state.read().unwrap();
        Ok(s.his
            .get(&id)
            .map(|v| {
                v.iter()
                    .filter(|i| &i.ts >= start && &i.ts < end)
                    .cloned()
                    .collect()
            })
            .unwrap_or_default())
    }

    async fn his_write(&self, rec: &Dict, items: Vec<HisItem>) -> OpResult<()> {
        let id = rec_id(rec).ok_or_else(|| OpError::BadRequest("rec has no id".into()))?;
        let mut s = self.state.write().unwrap();
        let his = s.his.entry(id).or_default();
        for item in items {
            match his.binary_search_by(|x| x.ts.cmp(&item.ts)) {
                Ok(i) => his[i] = item,
                Err(i) => his.insert(i, item),
            }
        }
        Ok(())
    }

    async fn point_write(&self, req: PointWrite) -> OpResult<()> {
        if !(1..=16).contains(&req.level) {
            return Err(OpError::BadRequest(format!(
                "level must be 1-16, got {}",
                req.level
            )));
        }
        let id = req.id.value().to_string();
        let mut s = self.state.write().unwrap();
        if !s.recs.get(&id).is_some_and(|r| r.has("writable")) {
            return Err(OpError::BadRequest(format!("not a writable point: {id}")));
        }
        let arr = s
            .priority
            .entry(id.clone())
            .or_insert_with(|| vec![None; PRIORITY_LEVELS]);
        arr[req.level as usize] = req.val.map(|v| (v, req.who));
        let winner = arr
            .iter()
            .enumerate()
            .skip(1)
            .find_map(|(l, x)| x.clone().map(|(v, _)| (l, v)));
        let rec = s.recs.get_mut(&id).expect("checked above");
        match winner {
            Some((level, v)) => {
                rec.insert("writeVal".into(), v.clone());
                rec.insert("writeLevel".into(), Value::make_number(level as f64));
                rec.insert("curVal".into(), v);
            }
            None => {
                rec.remove("writeVal");
                rec.remove("writeLevel");
            }
        }
        Ok(())
    }

    async fn point_write_array(&self, id: &Ref) -> OpResult<Grid> {
        let s = self.state.read().unwrap();
        if !s.recs.contains_key(id.value()) {
            return Err(OpError::UnknownRec(id.value().into()));
        }
        let arr = s.priority.get(id.value());
        let rows = (1..=16)
            .map(|l| {
                let mut d = Dict::new();
                d.insert("level".into(), Value::make_number(l as f64));
                d.insert("levelDis".into(), Value::make_str(&level_dis(l)));
                if let Some(Some((v, who))) = arr.map(|a| &a[l]) {
                    d.insert("val".into(), v.clone());
                    d.insert("who".into(), Value::make_str(who));
                }
                d
            })
            .collect();
        Ok(Grid::make_from_dicts(rows))
    }
}
