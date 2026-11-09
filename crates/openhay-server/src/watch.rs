//! Server-side watches, implemented once for every backend: a watch is a set
//! of ids plus the last snapshot sent, and `watchPoll` returns the records
//! whose content changed. Memory is bounded by `max_watches` x `max_ids`.

use libhaystack::val::{Dict, Ref};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub struct Watch {
    pub dis: String,
    pub lease: Duration,
    pub expires: Instant,
    pub ids: Vec<Ref>,
    pub last: HashMap<String, Dict>,
}

pub struct WatchRegistry {
    pub max_watches: usize,
    pub max_ids: usize,
    pub default_lease: Duration,
    watches: Mutex<HashMap<String, Watch>>,
}

impl Default for WatchRegistry {
    fn default() -> Self {
        Self {
            max_watches: 1_000,
            max_ids: 10_000,
            default_lease: Duration::from_secs(60),
            watches: Mutex::new(HashMap::new()),
        }
    }
}

impl WatchRegistry {
    fn purge(map: &mut HashMap<String, Watch>) {
        let now = Instant::now();
        map.retain(|_, w| w.expires > now);
    }

    /// Create a watch, or add ids to an existing one. Returns the watch id
    /// and the effective lease.
    pub fn subscribe(
        &self,
        watch_id: Option<&str>,
        dis: Option<&str>,
        lease: Option<Duration>,
        ids: &[Ref],
    ) -> Result<(String, Duration), String> {
        let mut map = self.watches.lock().unwrap();
        Self::purge(&mut map);
        let id = match watch_id {
            Some(w) if map.contains_key(w) => w.to_string(),
            Some(w) => return Err(format!("unknown watch: {w}")),
            None => {
                if map.len() >= self.max_watches {
                    return Err(format!("too many open watches (max {})", self.max_watches));
                }
                let id = format!("w-{}", openhay_auth::random_token(12));
                let lease = lease.unwrap_or(self.default_lease);
                map.insert(
                    id.clone(),
                    Watch {
                        dis: dis.unwrap_or("watch").to_string(),
                        lease,
                        expires: Instant::now() + lease,
                        ids: Vec::new(),
                        last: HashMap::new(),
                    },
                );
                id
            }
        };
        let w = map.get_mut(&id).expect("present");
        if let Some(l) = lease {
            w.lease = l;
        }
        for r in ids {
            if !w.ids.iter().any(|x| x.value() == r.value()) {
                if w.ids.len() >= self.max_ids {
                    return Err(format!("watch id limit reached (max {})", self.max_ids));
                }
                w.ids.push(r.clone());
            }
        }
        w.expires = Instant::now() + w.lease;
        Ok((id, w.lease))
    }

    /// Remove ids, or the whole watch when `ids` is empty / `close`.
    pub fn unsubscribe(&self, watch_id: &str, ids: &[Ref], close: bool) {
        let mut map = self.watches.lock().unwrap();
        if close || ids.is_empty() {
            map.remove(watch_id);
        } else if let Some(w) = map.get_mut(watch_id) {
            w.ids
                .retain(|x| !ids.iter().any(|r| r.value() == x.value()));
            for r in ids {
                w.last.remove(r.value());
            }
        }
    }

    /// Ids for a poll, renewing the lease.
    pub fn ids_for_poll(&self, watch_id: &str) -> Result<Vec<Ref>, String> {
        let mut map = self.watches.lock().unwrap();
        Self::purge(&mut map);
        let w = map
            .get_mut(watch_id)
            .ok_or_else(|| format!("unknown watch: {watch_id}"))?;
        w.expires = Instant::now() + w.lease;
        Ok(w.ids.clone())
    }

    /// Given fresh records, return those changed since the last poll (all of
    /// them when `refresh`), and remember the new snapshot.
    pub fn diff(&self, watch_id: &str, fresh: Vec<Dict>, refresh: bool) -> Vec<Dict> {
        let mut map = self.watches.lock().unwrap();
        let Some(w) = map.get_mut(watch_id) else {
            return Vec::new();
        };
        let mut out = Vec::new();
        for rec in fresh {
            let Some(id) = crate::mem::rec_id(&rec) else {
                continue;
            };
            let changed = refresh || w.last.get(&id) != Some(&rec);
            if changed {
                w.last.insert(id, rec.clone());
                out.push(rec);
            }
        }
        out
    }

    pub fn len(&self) -> usize {
        self.watches.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use libhaystack::val::Value;

    fn r(id: &str) -> Ref {
        Ref::make(id, None)
    }

    fn rec(id: &str, v: f64) -> Dict {
        let mut d = Dict::new();
        d.insert("id".into(), Value::make_ref(id));
        d.insert("curVal".into(), Value::make_number(v));
        d
    }

    #[test]
    fn subscribe_dedupes_and_extends() {
        let w = WatchRegistry::default();
        let (id, lease) = w
            .subscribe(None, Some("t"), None, &[r("a"), r("a")])
            .unwrap();
        assert_eq!(lease, w.default_lease);
        assert_eq!(w.ids_for_poll(&id).unwrap().len(), 1);
        let (same, _) = w
            .subscribe(Some(&id), None, None, &[r("a"), r("b")])
            .unwrap();
        assert_eq!(same, id);
        assert_eq!(w.ids_for_poll(&id).unwrap().len(), 2);
        assert!(w.subscribe(Some("w-nope"), None, None, &[]).is_err());
    }

    #[test]
    fn memory_is_bounded() {
        let w = WatchRegistry {
            max_watches: 2,
            max_ids: 3,
            ..WatchRegistry::default()
        };
        let (a, _) = w.subscribe(None, None, None, &[]).unwrap();
        w.subscribe(None, None, None, &[]).unwrap();
        let err = w.subscribe(None, None, None, &[]).unwrap_err();
        assert!(err.contains("too many open watches"), "{err}");
        let err = w
            .subscribe(Some(&a), None, None, &[r("1"), r("2"), r("3"), r("4")])
            .unwrap_err();
        assert!(err.contains("id limit"), "{err}");
        // Closing one frees its slot.
        w.unsubscribe(&a, &[], true);
        assert_eq!(w.len(), 1);
        assert!(w.subscribe(None, None, None, &[]).is_ok());
    }

    #[test]
    fn expired_leases_are_purged() {
        let w = WatchRegistry::default();
        let (id, _) = w
            .subscribe(None, None, Some(Duration::from_millis(1)), &[r("a")])
            .unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(w.ids_for_poll(&id).is_err());
        assert!(w.is_empty());
    }

    #[test]
    fn diff_returns_only_changes_unless_refresh() {
        let w = WatchRegistry::default();
        let (id, _) = w.subscribe(None, None, None, &[r("a"), r("b")]).unwrap();
        let first = w.diff(&id, vec![rec("a", 1.0), rec("b", 2.0)], false);
        assert_eq!(first.len(), 2, "first poll sends everything");
        assert!(
            w.diff(&id, vec![rec("a", 1.0), rec("b", 2.0)], false)
                .is_empty()
        );
        let changed = w.diff(&id, vec![rec("a", 1.0), rec("b", 3.0)], false);
        assert_eq!(changed, vec![rec("b", 3.0)]);
        assert_eq!(
            w.diff(&id, vec![rec("a", 1.0), rec("b", 3.0)], true).len(),
            2
        );
        // Records without an id are skipped; unknown watches get nothing.
        assert!(w.diff(&id, vec![Dict::new()], true).is_empty());
        assert!(w.diff("w-nope", vec![rec("a", 1.0)], true).is_empty());
    }

    #[test]
    fn unsubscribe_some_ids_forgets_their_snapshot() {
        let w = WatchRegistry::default();
        let (id, _) = w.subscribe(None, None, None, &[r("a"), r("b")]).unwrap();
        w.diff(&id, vec![rec("a", 1.0), rec("b", 2.0)], false);
        w.unsubscribe(&id, &[r("a")], false);
        assert_eq!(w.ids_for_poll(&id).unwrap().len(), 1);
        // Re-adding `a` sends it again on the next poll.
        w.subscribe(Some(&id), None, None, &[r("a")]).unwrap();
        assert_eq!(
            w.diff(&id, vec![rec("a", 1.0), rec("b", 2.0)], false).len(),
            1
        );
    }
}
