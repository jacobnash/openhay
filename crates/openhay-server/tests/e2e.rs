use libhaystack::val::*;
use openhay_auth::ScramCredentials;
use openhay_client::{Client, ClientError};
use openhay_codec::Format;
use openhay_server::{Config, MemBackend, Server, router};
use std::collections::HashMap;
use std::sync::Arc;

fn rec(pairs: &[(&str, Value)]) -> Dict {
    let mut d = Dict::new();
    for (k, v) in pairs {
        d.insert((*k).into(), v.clone());
    }
    d
}

fn site() -> Vec<Dict> {
    let m = Value::make_marker;
    vec![
        rec(&[
            ("id", Value::make_ref("s1")),
            ("site", m()),
            ("dis", Value::make_str("HQ")),
            ("tz", Value::make_str("New_York")),
        ]),
        rec(&[
            ("id", Value::make_ref("e1")),
            ("equip", m()),
            ("ahu", m()),
            ("siteRef", Value::make_ref("s1")),
            ("dis", Value::make_str("AHU-1")),
        ]),
        rec(&[
            ("id", Value::make_ref("p1")),
            ("point", m()),
            ("his", m()),
            ("temp", m()),
            ("sensor", m()),
            ("equipRef", Value::make_ref("e1")),
            ("siteRef", Value::make_ref("s1")),
            ("tz", Value::make_str("New_York")),
            ("kind", Value::make_str("Number")),
        ]),
        rec(&[
            ("id", Value::make_ref("p2")),
            ("point", m()),
            ("writable", m()),
            ("sp", m()),
            ("equipRef", Value::make_ref("e1")),
            ("siteRef", Value::make_ref("s1")),
            ("kind", Value::make_str("Number")),
        ]),
    ]
}

async fn start(auth: bool) -> String {
    let users = auth.then(|| {
        let mut u = HashMap::new();
        u.insert("admin".to_string(), ScramCredentials::new_random("pencil"));
        u
    });
    let cfg = Config {
        users,
        ..Config::default()
    };
    let server = Arc::new(Server::new(MemBackend::new(site()), cfg));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum_serve(listener, server).await });
    format!("http://{addr}/api/demo")
}

async fn axum_serve(l: tokio::net::TcpListener, s: Arc<Server<MemBackend>>) {
    axum::serve(l, router(s)).await.unwrap();
}

#[tokio::test]
async fn auth_required_and_scram_login() {
    let base = start(true).await;
    let mut c = Client::new(&base);
    assert!(matches!(
        c.about().await,
        Err(ClientError::Status { status: 401, .. })
    ));
    assert!(c.login("admin", "wrong").await.is_err());
    assert!(c.login("nobody", "pencil").await.is_err());
    c.login("admin", "pencil").await.unwrap();
    let about = c.about().await.unwrap();
    assert!(matches!(about.get("productName"), Some(Value::Str(_))));
}

#[tokio::test]
async fn read_filter_with_ref_paths() {
    let base = start(false).await;
    for fmt in [Format::Zinc, Format::Json3, Format::Hayson] {
        let mut c = Client::new(&base);
        c.format = fmt;
        let pts = c.read("point and equipRef->ahu", None).await.unwrap();
        assert_eq!(pts.len(), 2, "{fmt:?}");
        let one = c.read("point", Some(1)).await.unwrap();
        assert_eq!(one.len(), 1);
        let by_id = c
            .read_ids(&[Ref::make("p1", None), Ref::make("nope", None)])
            .await
            .unwrap();
        assert!(by_id[0].is_some() && by_id[1].is_none(), "{fmt:?}");
    }
}

#[tokio::test]
async fn bad_filter_is_error_grid() {
    let c = Client::new(&start(false).await);
    assert!(matches!(
        c.read("point and (", None).await,
        Err(ClientError::Server(_))
    ));
}

#[tokio::test]
async fn his_write_then_read() {
    let c = Client::new(&start(false).await);
    let id = Ref::make("p1", None);
    let ts = |s: &str| DateTime::parse_from_rfc3339_with_timezone(s, "New_York").unwrap();
    c.his_write(
        &id,
        &[
            (ts("2024-01-02T10:00:00-05:00"), Value::make_number(70.0)),
            (ts("2024-01-02T10:15:00-05:00"), Value::make_number(71.5)),
            (ts("2024-01-03T10:00:00-05:00"), Value::make_number(99.0)),
        ],
    )
    .await
    .unwrap();
    let g = c.his_read(&id, "2024-01-02").await.unwrap();
    assert_eq!(g.rows.len(), 2);
    let g = c.his_read(&id, "2024-01-02,2024-01-03").await.unwrap();
    assert_eq!(g.rows.len(), 3);
    assert_eq!(
        g.columns
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["ts", "val"]
    );
}

#[tokio::test]
async fn point_write_priority_and_watch() {
    let c = Client::new(&start(false).await);
    let p2 = Ref::make("p2", None);
    let (wid, initial) = c
        .watch_sub("test", &[p2.clone()], Some(30.0))
        .await
        .unwrap();
    assert_eq!(initial.len(), 1);
    assert!(
        c.watch_poll(&wid, false).await.unwrap().is_empty(),
        "nothing changed yet"
    );

    c.point_write(&p2, 16, Some(Value::make_number(68.0)), "sched")
        .await
        .unwrap();
    c.point_write(&p2, 8, Some(Value::make_number(72.0)), "operator")
        .await
        .unwrap();
    let changed = c.watch_poll(&wid, false).await.unwrap();
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0].get("curVal"), Some(&Value::make_number(72.0)));

    // releasing level 8 falls back to level 16
    c.point_write(&p2, 8, None, "operator").await.unwrap();
    let rec = c
        .read_ids(&[p2.clone()])
        .await
        .unwrap()
        .pop()
        .flatten()
        .unwrap();
    assert_eq!(rec.get("curVal"), Some(&Value::make_number(68.0)));

    let arr = c.point_write_array(&p2).await.unwrap();
    assert_eq!(arr.rows.len(), 16);
    assert!(arr.rows[15].get("val").is_some());

    // not writable
    assert!(
        c.point_write(
            &Ref::make("p1", None),
            8,
            Some(Value::make_number(1.0)),
            "x"
        )
        .await
        .is_err()
    );

    c.watch_close(&wid).await.unwrap();
    assert!(c.watch_poll(&wid, false).await.is_err());
}

#[tokio::test]
async fn nav_tree() {
    let c = Client::new(&start(false).await);
    let sites = c.nav(None).await.unwrap();
    assert_eq!(sites.len(), 1);
    let equips = c.nav(Some("s1")).await.unwrap();
    assert_eq!(equips.len(), 1);
    let points = c.nav(Some("e1")).await.unwrap();
    assert_eq!(points.len(), 2);
}

#[tokio::test]
async fn get_with_zinc_query_params() {
    let base = start(false).await;
    let body = reqwest_get(&format!("{base}/read?filter=site")).await;
    assert!(body.starts_with("ver:"), "{body}");
    assert!(body.contains("HQ"));
}

async fn reqwest_get(url: &str) -> String {
    // minimal HTTP/1.1 GET without pulling reqwest into this crate's dev-deps
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let u = url.strip_prefix("http://").unwrap();
    let (host, path) = u.split_once('/').unwrap();
    let mut s = tokio::net::TcpStream::connect(host).await.unwrap();
    s.write_all(
        format!("GET /{path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\n\r\n").as_bytes(),
    )
    .await
    .unwrap();
    let mut out = String::new();
    s.read_to_string(&mut out).await.unwrap();
    out.split_once("\r\n\r\n")
        .map(|(_, b)| b.to_string())
        .unwrap_or_default()
}
