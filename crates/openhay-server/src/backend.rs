//! The one trait a storage system implements to become a Haystack server.

use chrono::{NaiveDate, TimeZone};
use libhaystack::timezone::find_timezone;
use libhaystack::val::{DateTime, Dict, Grid, Ref, Value};
use std::future::Future;

/// An op-level failure. Rendered as a Haystack error grid (HTTP 200 with
/// `err` meta) per the spec, except [`OpError::NotFound`] for unknown ops.
#[derive(Debug, thiserror::Error)]
pub enum OpError {
    #[error("{0}")]
    BadRequest(String),
    #[error("unknown rec: {0}")]
    UnknownRec(String),
    #[error("op not supported: {0}")]
    Unsupported(&'static str),
    #[error("{0}")]
    Internal(String),
}

pub type OpResult<T> = Result<T, OpError>;

/// A `hisRead` range, already parsed from its Zinc string. Resolve it to
/// concrete instants with [`HisRange::resolve`] using the point's `tz`.
#[derive(Debug, Clone, PartialEq)]
pub enum HisRange {
    Today,
    Yesterday,
    Date(NaiveDate),
    Dates(NaiveDate, NaiveDate),
    /// End `None` means "until now".
    DateTimes(DateTime, Option<DateTime>),
}

impl HisRange {
    pub fn parse(text: &str) -> OpResult<Self> {
        let t = text.trim().trim_matches('"');
        match t {
            "today" => return Ok(Self::Today),
            "yesterday" => return Ok(Self::Yesterday),
            _ => {}
        }
        let bad = || OpError::BadRequest(format!("invalid range: {text:?}"));
        let parts: Vec<&str> = split_top_comma(t);
        let parse_one = |s: &str| -> OpResult<Value> {
            openhay_codec::decode_zinc_scalar(s.trim()).map_err(|_| bad())
        };
        match parts.as_slice() {
            [one] => match parse_one(one)? {
                Value::Date(d) => Ok(Self::Date(*d)),
                Value::DateTime(dt) => Ok(Self::DateTimes(dt, None)),
                _ => Err(bad()),
            },
            [a, b] => match (parse_one(a)?, parse_one(b)?) {
                (Value::Date(a), Value::Date(b)) => Ok(Self::Dates(*a, *b)),
                (Value::DateTime(a), Value::DateTime(b)) => Ok(Self::DateTimes(a, Some(b))),
                _ => Err(bad()),
            },
            _ => Err(bad()),
        }
    }

    /// Concrete half-open `[start, end)` in the given Haystack timezone name.
    pub fn resolve(&self, tz: &str) -> OpResult<(DateTime, DateTime)> {
        let zone = find_timezone(tz).map_err(|e| OpError::BadRequest(format!("tz {tz}: {e}")))?;
        let midnight = |d: NaiveDate| -> OpResult<DateTime> {
            let local = d.and_hms_opt(0, 0, 0).expect("valid midnight");
            zone.from_local_datetime(&local)
                .earliest()
                .map(DateTime::from)
                .ok_or_else(|| OpError::Internal(format!("no local midnight for {d} in {tz}")))
        };
        let today = chrono::Utc::now().with_timezone(&zone).date_naive();
        let next = |d: NaiveDate| d.succ_opt().expect("date in range");
        match self {
            Self::Today => Ok((midnight(today)?, midnight(next(today))?)),
            Self::Yesterday => {
                let y = today.pred_opt().expect("date in range");
                Ok((midnight(y)?, midnight(today)?))
            }
            Self::Date(d) => Ok((midnight(*d)?, midnight(next(*d))?)),
            Self::Dates(a, b) => Ok((midnight(*a)?, midnight(next(*b))?)),
            Self::DateTimes(a, b) => Ok((*a, b.unwrap_or_else(DateTime::utc_now))),
        }
    }
}

/// Split on commas that are not inside a quoted string.
fn split_top_comma(s: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0;
    let mut in_q = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => in_q = !in_q,
            ',' if !in_q => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// One history sample.
#[derive(Debug, Clone, PartialEq)]
pub struct HisItem {
    pub ts: DateTime,
    pub val: Value,
}

/// A `pointWrite` request. `level` and `val` are `None` for a read of the
/// priority array.
#[derive(Debug, Clone)]
pub struct PointWrite {
    pub id: Ref,
    pub level: u8,
    /// `None` releases the level (Haystack "auto").
    pub val: Option<Value>,
    pub who: String,
    pub duration: Option<Value>,
}

/// Storage-side contract. Implement this; [`crate::router`] provides HTTP,
/// content negotiation, auth, error grids and watches.
///
/// Methods with default bodies are optional: the default reports
/// "op not supported" to the client.
pub trait Backend: Send + Sync + 'static {
    /// Extra tags merged into the `about` response (e.g. `productName`).
    fn about(&self) -> Dict {
        Dict::new()
    }

    /// Records matching a Haystack filter, at most `limit`.
    fn read_filter(
        &self,
        filter: &str,
        limit: Option<usize>,
    ) -> impl Future<Output = OpResult<Vec<Dict>>> + Send;

    /// Records by id; `None` where an id doesn't exist (order preserved).
    fn read_ids(&self, ids: &[Ref]) -> impl Future<Output = OpResult<Vec<Option<Dict>>>> + Send;

    fn nav(&self, _nav_id: Option<&str>) -> impl Future<Output = OpResult<Grid>> + Send {
        async { Err(OpError::Unsupported("nav")) }
    }

    /// History for one point in `[start, end)`. The `rec` is the point
    /// record (already looked up), so implementations can read `tz` etc.
    fn his_read(
        &self,
        _rec: &Dict,
        _start: &DateTime,
        _end: &DateTime,
    ) -> impl Future<Output = OpResult<Vec<HisItem>>> + Send {
        async { Err(OpError::Unsupported("hisRead")) }
    }

    fn his_write(
        &self,
        _rec: &Dict,
        _items: Vec<HisItem>,
    ) -> impl Future<Output = OpResult<()>> + Send {
        async { Err(OpError::Unsupported("hisWrite")) }
    }

    fn point_write(&self, _req: PointWrite) -> impl Future<Output = OpResult<()>> + Send {
        async { Err(OpError::Unsupported("pointWrite")) }
    }

    /// The 17-row priority-array grid (`level`, `levelDis`, `val`, `who`).
    fn point_write_array(&self, _id: &Ref) -> impl Future<Output = OpResult<Grid>> + Send {
        async { Err(OpError::Unsupported("pointWrite")) }
    }

    fn invoke_action(
        &self,
        _id: &Ref,
        _action: &str,
        _args: Dict,
    ) -> impl Future<Output = OpResult<Grid>> + Send {
        async { Err(OpError::Unsupported("invokeAction")) }
    }
}
