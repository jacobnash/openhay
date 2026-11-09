//! `Client::read` against fake servers that do and don't page.
//!
//! The paging server mimics Baler (jacobnash/Baler#244): read-by-filter is
//! cut at a page size, a truncated page carries `incomplete`, `limit` and
//! `after` (the last id it covered) in its grid meta, and a request that
//! sends `after` back reads on from there. The other servers stand in for
//! Haxall/SkySpark, which never set `after`.

use axum::Router;
use axum::extract::State;
use axum::http::header;
use axum::response::IntoResponse;
use axum::routing::post;
use libhaystack::val::{Dict, Grid, HaystackDict, Value};
use openhay_client::{Client, ClientError};
use openhay_codec::Format;
use std::sync::{Arc, Mutex};

#[derive(Clone, Copy)]
enum Mode {
    /// Baler: page at `size` unless the request names a `limit`.
    Paging { size: usize },
    /// Pages at `size` even when the request asks for a larger `limit`
    /// (a server with a hard cap).
    PagingCapped { size: usize },
    /// Like `Paging`, but a full page always has a cursor, so a walk that
    /// ends exactly on the last record gets one more, empty, page.
    PagingEmptyTail { size: usize },
    /// No paging at all: ignores `after`, honours `limit`, sets no meta.
    Plain,
    /// Haxall/SkySpark style: honours `limit` and says `incomplete`, but
    /// never sets `after`.
    IncompleteNoCursor,
    /// Broken: always hands back the same cursor.
    StuckCursor { size: usize },
}

struct Fake {
    mode: Mode,
    recs: Vec<Dict>,
    requests: Mutex<Vec<Dict>>,
}

fn rec(i: usize) -> Dict {
    let mut d = Dict::new();
    // Zero-padded, so string order is id order, as in Baler's keyset pages.
    d.insert("id".into(), Value::make_ref(&format!("p{i:05}")));
    d.insert("point".into(), Value::make_marker());
    d.insert("n".into(), Value::make_number(i as f64));
    d
}

fn id_of(d: &Dict) -> String {
    match d.get("id") {
        Some(Value::Ref(r)) => r.value().to_string(),
        other => panic!("no id: {other:?}"),
    }
}

async fn read(State(f): State<Arc<Fake>>, body: String) -> impl IntoResponse {
    let req = openhay_codec::decode_grid_auto(&body).expect("request grid");
    let row = req.rows.into_iter().next().unwrap_or_default();
    f.requests.lock().unwrap().push(row.clone());
    let limit = match row.get("limit") {
        Some(Value::Number(n)) => Some(n.value as usize),
        _ => None,
    };
    let after = match row.get("after") {
        Some(Value::Ref(r)) => Some(r.value().to_string()),
        _ => None,
    };
    let mut meta = Dict::new();
    let rows: Vec<Dict> = match f.mode {
        Mode::Paging { size } | Mode::PagingEmptyTail { size } | Mode::PagingCapped { size } => {
            let n = match f.mode {
                Mode::PagingCapped { .. } => limit.unwrap_or(size).min(size),
                _ => limit.unwrap_or(size),
            };
            let rest: Vec<&Dict> = f
                .recs
                .iter()
                .filter(|r| after.as_deref().is_none_or(|a| id_of(r).as_str() > a))
                .collect();
            let page: Vec<Dict> = rest.iter().take(n).map(|r| (*r).clone()).collect();
            let more = match f.mode {
                Mode::PagingEmptyTail { .. } => page.len() == n,
                _ => rest.len() > n,
            };
            if more {
                meta.insert(
                    "incomplete".into(),
                    Value::make_str(&format!("limit of {n} reached; pass after to read on")),
                );
                meta.insert("limit".into(), Value::make_number(n as f64));
                meta.insert(
                    "after".into(),
                    Value::make_ref(&id_of(page.last().unwrap())),
                );
            }
            page
        }
        Mode::Plain => f
            .recs
            .iter()
            .take(limit.unwrap_or(usize::MAX))
            .cloned()
            .collect(),
        Mode::IncompleteNoCursor => {
            let n = limit.unwrap_or(usize::MAX);
            if f.recs.len() > n {
                meta.insert("incomplete".into(), Value::make_str("limit reached"));
            }
            f.recs.iter().take(n).cloned().collect()
        }
        Mode::StuckCursor { size } => {
            meta.insert("after".into(), Value::make_ref("p00000"));
            f.recs.iter().take(size).cloned().collect()
        }
    };
    let mut g = Grid::make_from_dicts(rows);
    if !meta.is_empty() {
        g.meta = Some(meta);
    }
    (
        [(header::CONTENT_TYPE, "text/zinc; charset=utf-8")],
        openhay_codec::encode_grid(Format::Zinc, &g).unwrap(),
    )
}

async fn start(mode: Mode, n: usize) -> (String, Arc<Fake>) {
    let fake = Arc::new(Fake {
        mode,
        recs: (0..n).map(rec).collect(),
        requests: Mutex::new(Vec::new()),
    });
    let app = Router::new()
        .route("/api/read", post(read))
        .with_state(fake.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}/api"), fake)
}

fn ids(rows: &[Dict]) -> Vec<String> {
    rows.iter().map(id_of).collect()
}

fn expected(range: std::ops::Range<usize>) -> Vec<String> {
    range.map(|i| format!("p{i:05}")).collect()
}

fn requests(f: &Fake) -> Vec<Dict> {
    f.requests.lock().unwrap().clone()
}

#[tokio::test]
async fn read_follows_pages_to_the_full_set() {
    let (base, fake) = start(Mode::Paging { size: 1000 }, 2500).await;
    for fmt in [Format::Zinc, Format::Json3, Format::Hayson] {
        fake.requests.lock().unwrap().clear();
        let mut c = Client::new(&base);
        c.format = fmt;
        let rows = c.read("point", None).await.unwrap();
        assert_eq!(ids(&rows), expected(0..2500), "{fmt:?}");
        let reqs = requests(&fake);
        assert_eq!(reqs.len(), 3, "{fmt:?}");
        // The first request is a plain Haystack read: no `after`, no `limit`.
        assert!(!reqs[0].has("after") && !reqs[0].has("limit"), "{fmt:?}");
        assert_eq!(
            reqs[1].get("after"),
            Some(&Value::make_ref("p00999")),
            "{fmt:?}"
        );
        assert_eq!(
            reqs[2].get("after"),
            Some(&Value::make_ref("p01999")),
            "{fmt:?}"
        );
        assert!(
            reqs.iter()
                .all(|r| r.get("filter") == Some(&Value::make_str("point")))
        );
    }
}

#[tokio::test]
async fn read_stops_at_the_callers_limit() {
    // The server's page is smaller than the caller's limit: follow until
    // `limit` records are in, asking only for what's still missing.
    let (base, fake) = start(Mode::PagingCapped { size: 1000 }, 2500).await;
    let c = Client::new(&base);
    let rows = c.read("point", Some(1500)).await.unwrap();
    assert_eq!(ids(&rows), expected(0..1500));
    let reqs = requests(&fake);
    assert_eq!(reqs.len(), 2);
    assert_eq!(reqs[0].get("limit"), Some(&Value::make_number(1500.0)));
    assert_eq!(reqs[1].get("limit"), Some(&Value::make_number(500.0)));

    // A limit the server honours in one page (Baler takes `limit` as the
    // page size): one request, as before, even though the page has `after`.
    let (base, fake) = start(Mode::Paging { size: 1000 }, 2500).await;
    let c = Client::new(&base);
    for n in [10, 1500] {
        fake.requests.lock().unwrap().clear();
        let rows = c.read("point", Some(n)).await.unwrap();
        assert_eq!(ids(&rows), expected(0..n));
        assert_eq!(requests(&fake).len(), 1);
    }
}

#[tokio::test]
async fn read_page_surfaces_the_meta() {
    let (base, _fake) = start(Mode::Paging { size: 1000 }, 2500).await;
    let c = Client::new(&base);
    let p1 = c.read_page("point", None, None).await.unwrap();
    assert_eq!(p1.rows.len(), 1000);
    assert!(!p1.is_complete());
    assert_eq!(p1.after(), Some(&Value::make_ref("p00999")));
    assert!(p1.incomplete().unwrap().contains("limit of 1000"));
    assert_eq!(p1.meta.get("limit"), Some(&Value::make_number(1000.0)));

    let p2 = c.read_page("point", None, p1.after()).await.unwrap();
    assert_eq!(ids(&p2.rows), expected(1000..2000));
    let p3 = c.read_page("point", None, p2.after()).await.unwrap();
    assert_eq!(ids(&p3.rows), expected(2000..2500));
    assert!(p3.is_complete() && p3.after().is_none());
}

#[tokio::test]
async fn an_empty_last_page_ends_the_walk() {
    let (base, fake) = start(Mode::PagingEmptyTail { size: 1000 }, 2000).await;
    let rows = Client::new(&base).read("point", None).await.unwrap();
    assert_eq!(ids(&rows), expected(0..2000));
    assert_eq!(requests(&fake).len(), 3);
}

#[tokio::test]
async fn a_server_that_doesnt_page_is_read_once_as_before() {
    let (base, fake) = start(Mode::Plain, 2500).await;
    let c = Client::new(&base);
    let rows = c.read("point", None).await.unwrap();
    assert_eq!(ids(&rows), expected(0..2500));
    let rows = c.read("point", Some(7)).await.unwrap();
    assert_eq!(ids(&rows), expected(0..7));
    let reqs = requests(&fake);
    assert_eq!(reqs.len(), 2);
    assert!(reqs.iter().all(|r| !r.has("after")));
    let page = c.read_page("point", None, None).await.unwrap();
    assert!(page.is_complete() && page.meta.is_empty());
}

#[tokio::test]
async fn incomplete_without_a_cursor_is_one_page() {
    // Haxall marks a limit-truncated grid `incomplete` but has no cursor:
    // there's nothing to follow, so `read` returns that page.
    let (base, fake) = start(Mode::IncompleteNoCursor, 2500).await;
    let c = Client::new(&base);
    let rows = c.read("point", Some(100)).await.unwrap();
    assert_eq!(rows.len(), 100);
    assert_eq!(requests(&fake).len(), 1);
    let page = c.read_page("point", Some(100), None).await.unwrap();
    assert_eq!(page.incomplete(), Some("limit reached"));
    assert!(page.after().is_none() && !page.is_complete());
}

#[tokio::test]
async fn a_cursor_that_doesnt_move_is_an_error_not_a_loop() {
    let (base, fake) = start(Mode::StuckCursor { size: 10 }, 2500).await;
    let err = Client::new(&base).read("point", None).await.unwrap_err();
    assert!(matches!(err, ClientError::Paging(_)), "{err}");
    assert_eq!(requests(&fake).len(), 2);
}
