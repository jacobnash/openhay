//! A Haystack 3/4 HTTP server. Implement [`Backend`] for your storage, call
//! [`router`], and you serve the standard op surface at both
//! `/api/{op}` (Haxall layout) and `/api/{project}/{op}` (SkySpark layout),
//! with SCRAM auth, Zinc / JSON v3 / Hayson / CSV negotiation, Haystack error
//! grids, and server-managed watches.

pub mod backend;
pub mod mem;
pub mod watch;

pub use backend::{Backend, HisItem, HisRange, OpError, OpResult, PointWrite};
pub use mem::MemBackend;
pub use openhay_auth::ScramCredentials;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{Path, RawQuery, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::any;
use libhaystack::val::{Column, DateTime, Dict, Grid, HaystackDict, Ref, Value};
use openhay_auth::{ScramServerState, b64url, b64url_decode, parse_auth_header, random_token};
use openhay_codec::{Format, decode_grid, encode_grid};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Validates an externally issued bearer token (OAuth2/OIDC access token,
/// API key, JWT, ...) and returns the principal it identifies, or `None`.
/// Checked for both `Authorization: Bearer <token>` (OAuth style) and
/// `Authorization: BEARER authToken=<token>` (Haystack style).
pub type TokenValidator = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// Server configuration. Auth is on when either `users` (Haystack SCRAM
/// login) or `token_validator` (OAuth/API-key bearer tokens) is set; both
/// can be on at once.
pub struct Config {
    /// SCRAM users. `None` with no `token_validator` disables auth.
    pub users: Option<HashMap<String, ScramCredentials>>,
    /// External bearer-token check (OAuth2 / OIDC / API keys).
    pub token_validator: Option<TokenValidator>,
    /// Which JSON to send for a bare `Accept: application/json` when the
    /// request itself didn't reveal a flavor.
    pub json_flavor: Format,
    pub token_ttl: Duration,
    pub server_name: String,
    pub tz: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            users: None,
            token_validator: None,
            json_flavor: Format::Hayson,
            token_ttl: Duration::from_secs(12 * 3600),
            server_name: "openhay".into(),
            tz: "UTC".into(),
        }
    }
}

struct Handshake {
    username: String,
    creds: ScramCredentials,
    scram: Option<ScramServerState>,
    created: Instant,
}

pub struct Server<B: Backend> {
    pub backend: Arc<B>,
    pub config: Config,
    pub watches: watch::WatchRegistry,
    handshakes: Mutex<HashMap<String, Handshake>>,
    tokens: Mutex<HashMap<String, (String, Instant)>>,
    boot: DateTime,
}

impl<B: Backend> Server<B> {
    pub fn new(backend: B, config: Config) -> Self {
        Self::from_arc(Arc::new(backend), config)
    }

    pub fn from_arc(backend: Arc<B>, config: Config) -> Self {
        Self {
            backend,
            config,
            watches: watch::WatchRegistry::default(),
            handshakes: Mutex::new(HashMap::new()),
            tokens: Mutex::new(HashMap::new()),
            boot: DateTime::utc_now(),
        }
    }
}

/// Build the axum router.
pub fn router<B: Backend>(server: Arc<Server<B>>) -> Router {
    Router::new()
        .route("/api/{op}", any(handle_flat::<B>))
        .route("/api/{project}/{op}", any(handle_project::<B>))
        .with_state(server)
}

/// Serve on an address until the process exits.
pub async fn serve<B: Backend>(server: Arc<Server<B>>, addr: &str) -> std::io::Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, router(server)).await
}

async fn handle_flat<B: Backend>(
    State(s): State<Arc<Server<B>>>,
    Path(op): Path<String>,
    method: Method,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
    body: Bytes,
) -> Response {
    handle(s, None, op, method, headers, q, body).await
}

async fn handle_project<B: Backend>(
    State(s): State<Arc<Server<B>>>,
    Path((project, op)): Path<(String, String)>,
    method: Method,
    headers: HeaderMap,
    RawQuery(q): RawQuery,
    body: Bytes,
) -> Response {
    handle(s, Some(project), op, method, headers, q, body).await
}

// ---------------------------------------------------------------- auth

enum AuthOutcome {
    Ok(Option<HeaderValue>),
    Respond(Response),
}

fn unauthorized(www: Option<String>) -> Response {
    let mut r = (StatusCode::UNAUTHORIZED, "").into_response();
    if let Some(w) = www.and_then(|w| HeaderValue::from_str(&w).ok()) {
        r.headers_mut().insert(header::WWW_AUTHENTICATE, w);
    }
    r
}

impl<B: Backend> Server<B> {
    fn authenticate(&self, headers: &HeaderMap) -> AuthOutcome {
        if self.config.users.is_none() && self.config.token_validator.is_none() {
            return AuthOutcome::Ok(None);
        }
        let empty = HashMap::new();
        let users = self.config.users.as_ref().unwrap_or(&empty);
        let raw = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        let (scheme, params) = parse_auth_header(raw);
        match scheme.as_deref() {
            Some("BEARER") => {
                // Haystack style `BEARER authToken=x`, or OAuth style `Bearer x`
                // (the token itself may contain '=' padding, so take it raw).
                let tok = match params.get("authToken") {
                    Some(t) => t.clone(),
                    None => raw.trim()[6..].trim().to_string(),
                };
                {
                    let mut tokens = self.tokens.lock().unwrap();
                    let now = Instant::now();
                    tokens.retain(|_, (_, exp)| *exp > now);
                    if tokens.contains_key(&tok) {
                        return AuthOutcome::Ok(None);
                    }
                }
                match &self.config.token_validator {
                    Some(v) if !tok.is_empty() && v(&tok).is_some() => AuthOutcome::Ok(None),
                    _ => AuthOutcome::Respond(unauthorized(None)),
                }
            }
            Some("HELLO") if self.config.users.is_some() => {
                let user = params
                    .get("username")
                    .and_then(|u| b64url_decode(u).ok())
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_default();
                // Unknown users get throwaway credentials so the handshake
                // looks identical and fails at the proof (no user enumeration).
                let creds = users.get(&user).cloned().unwrap_or_else(|| {
                    // Random bytes, not a PBKDF2 run: spraying HELLOs for
                    // made-up usernames must not cost the server hashing.
                    let mut salt = [0u8; 16];
                    let mut salted = [0u8; 32];
                    rand_fill(&mut salt);
                    rand_fill(&mut salted);
                    ScramCredentials::from_salted_password(
                        &salted,
                        &salt,
                        ScramCredentials::DEFAULT_ITERATIONS,
                    )
                });
                let token = random_token(18);
                let mut hs = self.handshakes.lock().unwrap();
                hs.retain(|_, h| h.created.elapsed() < Duration::from_secs(120));
                hs.insert(
                    token.clone(),
                    Handshake {
                        username: user,
                        creds,
                        scram: None,
                        created: Instant::now(),
                    },
                );
                AuthOutcome::Respond(unauthorized(Some(format!(
                    "SCRAM handshakeToken={token}, hash=SHA-256"
                ))))
            }
            Some("SCRAM") => {
                let token = params.get("handshakeToken").cloned().unwrap_or_default();
                let data = params
                    .get("data")
                    .and_then(|d| b64url_decode(d).ok())
                    .and_then(|b| String::from_utf8(b).ok())
                    .unwrap_or_default();
                let mut hs = self.handshakes.lock().unwrap();
                let Some(h) = hs.get_mut(&token) else {
                    return AuthOutcome::Respond(unauthorized(None));
                };
                match &h.scram {
                    None => match ScramServerState::start(&data, &h.creds) {
                        Ok((state, server_first)) if state.username == h.username => {
                            h.scram = Some(state);
                            AuthOutcome::Respond(unauthorized(Some(format!(
                                "SCRAM handshakeToken={token}, hash=SHA-256, data={}",
                                b64url(server_first.as_bytes())
                            ))))
                        }
                        _ => {
                            hs.remove(&token);
                            AuthOutcome::Respond(unauthorized(None))
                        }
                    },
                    Some(state) => {
                        let result = state.finish(&data, &h.creds);
                        let user = h.username.clone();
                        hs.remove(&token);
                        match result {
                            Ok(server_final) => {
                                let auth_token = random_token(32);
                                self.tokens.lock().unwrap().insert(
                                    auth_token.clone(),
                                    (user, Instant::now() + self.config.token_ttl),
                                );
                                let info = format!(
                                    "authToken={auth_token}, data={}",
                                    b64url(server_final.as_bytes())
                                );
                                AuthOutcome::Ok(HeaderValue::from_str(&info).ok())
                            }
                            Err(_) => {
                                AuthOutcome::Respond((StatusCode::FORBIDDEN, "").into_response())
                            }
                        }
                    }
                }
            }
            _ => AuthOutcome::Respond(unauthorized(Some(if self.config.users.is_some() {
                "SCRAM hash=SHA-256".into()
            } else {
                "Bearer".into()
            }))),
        }
    }
}

fn rand_fill(buf: &mut [u8]) {
    let hex = random_token(buf.len());
    for (i, b) in buf.iter_mut().enumerate() {
        *b = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).expect("hex");
    }
}

// ---------------------------------------------------------------- request decoding

/// Decode GET query params (Zinc-encoded values) into a one-row grid.
fn query_to_grid(q: &str) -> Grid {
    let mut d = Dict::new();
    for (k, v) in parse_query(q) {
        let val = openhay_codec::decode_zinc_scalar(&v).unwrap_or_else(|_| Value::make_str(&v));
        d.insert(k, val);
    }
    if d.is_empty() {
        Grid::make_empty()
    } else {
        Grid::make_from_dicts(vec![d])
    }
}

fn parse_query(q: &str) -> Vec<(String, String)> {
    q.split('&')
        .filter(|kv| !kv.is_empty())
        .map(|kv| {
            let (k, v) = kv.split_once('=').unwrap_or((kv, ""));
            (pct_decode(k), pct_decode(v))
        })
        .collect()
}

fn pct_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let hex = |x: u8| (x as char).to_digit(16);
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => match (hex(b[i + 1]), hex(b[i + 2])) {
                (Some(h), Some(l)) => {
                    out.push((h * 16 + l) as u8);
                    i += 2;
                }
                _ => out.push(b'%'),
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn request_format(headers: &HeaderMap, body: &str) -> Format {
    let ct = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ct.starts_with("application/json") {
        if body.contains("\"_kind\"") {
            Format::Hayson
        } else {
            Format::Json3
        }
    } else if ct.starts_with("text/trio") {
        Format::Trio
    } else if ct.starts_with("text/zinc") || ct.starts_with("text/plain") {
        Format::Zinc
    } else {
        Format::sniff(body)
    }
}

// ---------------------------------------------------------------- responses

pub fn err_grid(msg: &str) -> Grid {
    let mut meta = Dict::new();
    meta.insert("err".into(), Value::make_marker());
    meta.insert("dis".into(), Value::make_str(msg));
    meta.insert("errTrace".into(), Value::make_str(msg));
    Grid {
        meta: Some(meta),
        columns: vec![Column {
            name: "empty".into(),
            meta: None,
        }],
        rows: vec![],
        ver: "3.0".into(),
    }
}

fn grid_response(grid: &Grid, fmt: Format, auth_info: Option<HeaderValue>) -> Response {
    let (status, body) = match encode_grid(fmt, grid) {
        Ok(b) => (StatusCode::OK, b),
        Err(e) => (StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };
    let mut r = (status, body).into_response();
    let ct = format!("{}; charset=utf-8", fmt.mime());
    r.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&ct).expect("ascii"),
    );
    if let Some(info) = auth_info {
        r.headers_mut().insert("Authentication-Info", info);
    }
    r
}

fn grid_with_cols(meta: Option<Dict>, cols: &[&str], rows: Vec<Dict>) -> Grid {
    Grid {
        meta,
        columns: cols
            .iter()
            .map(|c| Column {
                name: (*c).into(),
                meta: None,
            })
            .collect(),
        rows,
        ver: "3.0".into(),
    }
}

fn rows_grid(rows: Vec<Dict>) -> Grid {
    if rows.is_empty() {
        Grid::make_empty()
    } else {
        Grid::make_from_dicts(rows)
    }
}

// ---------------------------------------------------------------- dispatch

async fn handle<B: Backend>(
    s: Arc<Server<B>>,
    _project: Option<String>,
    op: String,
    method: Method,
    headers: HeaderMap,
    query: Option<String>,
    body: Bytes,
) -> Response {
    let auth_info = match s.authenticate(&headers) {
        AuthOutcome::Ok(info) => info,
        AuthOutcome::Respond(r) => return r,
    };
    let body = String::from_utf8_lossy(&body).into_owned();
    let (req, req_fmt) = if method == Method::POST && !body.trim().is_empty() {
        let f = request_format(&headers, &body);
        match decode_grid(f, &body) {
            Ok(g) => (g, Some(f)),
            Err(e) => {
                return grid_response(
                    &err_grid(&format!("cannot decode request: {e}")),
                    Format::Zinc,
                    auth_info,
                );
            }
        }
    } else {
        (query_to_grid(query.as_deref().unwrap_or("")), None)
    };
    let json_flavor = match req_fmt {
        Some(f @ (Format::Json3 | Format::Hayson)) => f,
        _ => s.config.json_flavor,
    };
    let accept = headers.get(header::ACCEPT).and_then(|v| v.to_str().ok());
    let fmt = Format::negotiate(accept, json_flavor);
    if !OPS.iter().any(|(name, _)| *name == op) {
        return (StatusCode::NOT_FOUND, format!("unknown op: {op}")).into_response();
    }
    let grid = match dispatch(&s, &op, req).await {
        Ok(g) => g,
        Err(e) => err_grid(&e.to_string()),
    };
    grid_response(&grid, fmt, auth_info)
}

const OPS: &[(&str, &str)] = &[
    ("about", "Summary information for server"),
    ("ops", "Operations supported by this server"),
    ("formats", "Grid data formats supported by this server"),
    ("read", "Read records by id or filter"),
    ("nav", "Navigate record tree"),
    ("hisRead", "Read time series history data"),
    ("hisWrite", "Write time series history data"),
    ("pointWrite", "Read/write writable point priority array"),
    ("watchSub", "Subscribe to watch"),
    ("watchUnsub", "Unsubscribe from watch"),
    ("watchPoll", "Poll watch for changes"),
    ("invokeAction", "Invoke action on a record"),
    ("close", "Close the current session"),
];

fn first_row(g: &Grid) -> Dict {
    g.rows.first().cloned().unwrap_or_default()
}

fn get_ref(d: &Dict, tag: &str) -> OpResult<Ref> {
    match d.get(tag) {
        Some(Value::Ref(r)) => Ok(r.clone()),
        Some(Value::Str(s)) => Ok(Ref::make(s.value().trim_start_matches('@'), None)),
        _ => Err(OpError::BadRequest(format!("missing {tag} ref"))),
    }
}

fn get_str(d: &Dict, tag: &str) -> Option<String> {
    match d.get(tag) {
        Some(Value::Str(s)) => Some(s.value().to_string()),
        Some(Value::Uri(u)) => Some(u.value().to_string()),
        Some(Value::Ref(r)) => Some(r.value().to_string()),
        _ => None,
    }
}

fn get_num(d: &Dict, tag: &str) -> Option<f64> {
    match d.get(tag) {
        Some(Value::Number(n)) => Some(n.value),
        _ => None,
    }
}

fn meta_of(g: &Grid) -> Dict {
    g.meta.clone().unwrap_or_default()
}

fn lease_of(meta: &Dict) -> Option<Duration> {
    match meta.get("lease") {
        Some(Value::Number(n)) => {
            let secs = match n.unit.map(|u| u.to_string()) {
                Some(u) if u == "min" => n.value * 60.0,
                Some(u) if u == "h" || u == "hr" => n.value * 3600.0,
                Some(u) if u == "ms" => n.value / 1000.0,
                _ => n.value,
            };
            Some(Duration::from_secs_f64(secs.max(1.0)))
        }
        _ => None,
    }
}

async fn dispatch<B: Backend>(s: &Server<B>, op: &str, req: Grid) -> OpResult<Grid> {
    let b = &s.backend;
    match op {
        "about" => {
            let mut d = Dict::new();
            d.insert("haystackVersion".into(), Value::make_str("4.0"));
            d.insert("serverName".into(), Value::make_str(&s.config.server_name));
            d.insert(
                "serverTime".into(),
                Value::make_datetime(DateTime::utc_now()),
            );
            d.insert("serverBootTime".into(), Value::make_datetime(s.boot));
            d.insert("tz".into(), Value::make_str(&s.config.tz));
            d.insert("productName".into(), Value::make_str("openhay"));
            d.insert(
                "productVersion".into(),
                Value::make_str(env!("CARGO_PKG_VERSION")),
            );
            d.insert("vendorName".into(), Value::make_str("openhay"));
            d.insert(
                "vendorUri".into(),
                Value::make_uri("https://github.com/jacobnash/openhay"),
            );
            for (k, v) in b.about().iter() {
                d.insert(k.to_string(), v.clone());
            }
            Ok(Grid::make_from_dicts(vec![d]))
        }
        "ops" => Ok(Grid::make_from_dicts(
            OPS.iter()
                .map(|(n, summary)| {
                    let mut d = Dict::new();
                    d.insert("name".into(), Value::make_str(n));
                    d.insert("summary".into(), Value::make_str(summary));
                    d
                })
                .collect(),
        )),
        "formats" => Ok(Grid::make_from_dicts(
            Format::ALL
                .iter()
                .filter(|f| **f != Format::Json3)
                .map(|f| {
                    let mut d = Dict::new();
                    d.insert("mime".into(), Value::make_str(f.mime()));
                    if *f != Format::Csv {
                        d.insert("receive".into(), Value::make_marker());
                    }
                    d.insert("send".into(), Value::make_marker());
                    d
                })
                .collect(),
        )),
        "read" => {
            let row = first_row(&req);
            if row.has("filter") {
                let filter = get_str(&row, "filter").unwrap_or_default();
                let limit = get_num(&row, "limit").map(|n| n.max(0.0) as usize);
                Ok(rows_grid(b.read_filter(&filter, limit).await?))
            } else {
                let ids: Vec<Ref> = req
                    .rows
                    .iter()
                    .filter_map(|r| get_ref(r, "id").ok())
                    .collect();
                if ids.is_empty() {
                    return Err(OpError::BadRequest("read requires filter or id".into()));
                }
                let recs = b.read_ids(&ids).await?;
                // Missing ids come back as rows of nulls, per spec.
                let rows: Vec<Dict> = recs.into_iter().map(Option::unwrap_or_default).collect();
                if rows.iter().all(Dict::is_empty) {
                    return Ok(grid_with_cols(None, &["id"], rows));
                }
                Ok(Grid::make_from_dicts(rows))
            }
        }
        "nav" => {
            let row = first_row(&req);
            b.nav(get_str(&row, "navId").as_deref()).await
        }
        "hisRead" => {
            let row = first_row(&req);
            let id = get_ref(&row, "id")?;
            let range = get_str(&row, "range")
                .or_else(|| {
                    row.get("range")
                        .and_then(|v| openhay_codec::encode_zinc_scalar(v).ok())
                })
                .ok_or_else(|| OpError::BadRequest("missing range".into()))?;
            let rec = b
                .read_ids(std::slice::from_ref(&id))
                .await?
                .pop()
                .flatten()
                .ok_or_else(|| OpError::UnknownRec(id.value().into()))?;
            let tz = get_str(&rec, "tz").unwrap_or_else(|| s.config.tz.clone());
            let (start, end) = HisRange::parse(&range)?.resolve(&tz)?;
            let items = b.his_read(&rec, &start, &end).await?;
            let mut meta = Dict::new();
            meta.insert("id".into(), Value::Ref(id));
            meta.insert("hisStart".into(), Value::make_datetime(start));
            meta.insert("hisEnd".into(), Value::make_datetime(end));
            let rows = items
                .into_iter()
                .map(|i| {
                    let mut d = Dict::new();
                    d.insert("ts".into(), Value::make_datetime(i.ts));
                    d.insert("val".into(), i.val);
                    d
                })
                .collect();
            Ok(grid_with_cols(Some(meta), &["ts", "val"], rows))
        }
        "hisWrite" => {
            let id = get_ref(&meta_of(&req), "id")?;
            let rec = b
                .read_ids(std::slice::from_ref(&id))
                .await?
                .pop()
                .flatten()
                .ok_or_else(|| OpError::UnknownRec(id.value().into()))?;
            let mut items = Vec::with_capacity(req.rows.len());
            for r in &req.rows {
                let Some(Value::DateTime(ts)) = r.get("ts") else {
                    return Err(OpError::BadRequest("hisWrite row missing ts".into()));
                };
                items.push(HisItem {
                    ts: *ts,
                    val: r.get("val").cloned().unwrap_or(Value::Null),
                });
            }
            b.his_write(&rec, items).await?;
            Ok(Grid::make_empty())
        }
        "pointWrite" => {
            let row = first_row(&req);
            let id = get_ref(&row, "id")?;
            match get_num(&row, "level") {
                None => b.point_write_array(&id).await,
                Some(level) => {
                    let val = row.get("val").cloned().filter(|v| !v.is_null());
                    b.point_write(PointWrite {
                        id: id.clone(),
                        level: level as u8,
                        val,
                        who: get_str(&row, "who").unwrap_or_else(|| "openhay".into()),
                        duration: row.get("duration").cloned(),
                    })
                    .await?;
                    Ok(Grid::make_empty())
                }
            }
        }
        "watchSub" => {
            let meta = meta_of(&req);
            let ids: Vec<Ref> = req
                .rows
                .iter()
                .filter_map(|r| get_ref(r, "id").ok())
                .collect();
            let (wid, lease) = s
                .watches
                .subscribe(
                    get_str(&meta, "watchId").as_deref(),
                    get_str(&meta, "watchDis").as_deref(),
                    lease_of(&meta),
                    &ids,
                )
                .map_err(OpError::BadRequest)?;
            let recs: Vec<Dict> = b.read_ids(&ids).await?.into_iter().flatten().collect();
            let rows = s.watches.diff(&wid, recs, true);
            let mut m = Dict::new();
            m.insert("watchId".into(), Value::make_str(&wid));
            m.insert(
                "lease".into(),
                Value::make_number_unit(
                    lease.as_secs_f64(),
                    libhaystack::units::get_unit("s").expect("sec unit"),
                ),
            );
            let mut g = rows_grid(rows);
            g.meta = Some(m);
            Ok(g)
        }
        "watchUnsub" => {
            let meta = meta_of(&req);
            let wid = get_str(&meta, "watchId")
                .ok_or_else(|| OpError::BadRequest("missing watchId".into()))?;
            let ids: Vec<Ref> = req
                .rows
                .iter()
                .filter_map(|r| get_ref(r, "id").ok())
                .collect();
            s.watches.unsubscribe(&wid, &ids, meta.has("close"));
            Ok(Grid::make_empty())
        }
        "watchPoll" => {
            let meta = meta_of(&req);
            let wid = get_str(&meta, "watchId")
                .ok_or_else(|| OpError::BadRequest("missing watchId".into()))?;
            let ids = s.watches.ids_for_poll(&wid).map_err(OpError::BadRequest)?;
            let recs: Vec<Dict> = b.read_ids(&ids).await?.into_iter().flatten().collect();
            let rows = s.watches.diff(&wid, recs, meta.has("refresh"));
            let mut g = rows_grid(rows);
            let mut m = Dict::new();
            m.insert("watchId".into(), Value::make_str(&wid));
            g.meta = Some(m);
            Ok(g)
        }
        "invokeAction" => {
            let meta = meta_of(&req);
            let id = get_ref(&meta, "id")?;
            let action = get_str(&meta, "action")
                .ok_or_else(|| OpError::BadRequest("missing action".into()))?;
            b.invoke_action(&id, &action, first_row(&req)).await
        }
        "close" => Ok(Grid::make_empty()),
        _ => Err(OpError::BadRequest(format!("unknown op {op}"))),
    }
}
