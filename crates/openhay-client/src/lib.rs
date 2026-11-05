//! Async Haystack client for SkySpark, Haxall, nhaystack, openhay, or any
//! other server speaking the Haystack HTTP API. Supports SCRAM-SHA-256 login,
//! Zinc or JSON (v3 or Hayson) on the wire, and every standard op.

use libhaystack::val::{Column, DateTime, Dict, Grid, HaystackDict, Ref, Value};
use openhay_auth::{ScramClient, b64url, b64url_decode, parse_auth_header};
use openhay_codec::{Format, decode_grid, encode_grid};

#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    #[error("http: {0}")]
    Http(#[from] reqwest::Error),
    #[error("auth: {0}")]
    Auth(String),
    #[error("codec: {0}")]
    Codec(#[from] openhay_codec::CodecError),
    #[error("HTTP {status}: {body}")]
    Status { status: u16, body: String },
    /// The server returned a Haystack error grid.
    #[error("server error: {0}")]
    Server(String),
}

pub type Result<T> = std::result::Result<T, ClientError>;

pub struct Client {
    http: reqwest::Client,
    /// Ends with `/`, e.g. `http://host/api/demo/`.
    base: String,
    token: Option<String>,
    /// Send `Authorization: Bearer <token>` (OAuth2 / API key) instead of
    /// Haystack's `BEARER authToken=<token>`.
    oauth_style: bool,
    /// Format used for request bodies and asked for in `Accept`.
    pub format: Format,
}

impl Client {
    /// `base` is the API root: `http://host:8080/api/demo` (SkySpark) or
    /// `http://host:8080/api` (Haxall). A trailing slash is optional.
    pub fn new(base: &str) -> Self {
        let mut base = base.to_string();
        if !base.ends_with('/') {
            base.push('/');
        }
        Self {
            http: reqwest::Client::new(),
            base,
            token: None,
            oauth_style: false,
            format: Format::Zinc,
        }
    }

    /// Use a Haystack `authToken` obtained elsewhere.
    pub fn with_token(mut self, token: &str) -> Self {
        self.token = Some(token.to_string());
        self.oauth_style = false;
        self
    }

    /// Use an OAuth2 / OIDC access token or API key, sent as
    /// `Authorization: Bearer <token>`.
    pub fn with_bearer(mut self, token: &str) -> Self {
        self.token = Some(token.to_string());
        self.oauth_style = true;
        self
    }

    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }

    fn url(&self, op: &str) -> String {
        format!("{}{op}", self.base)
    }

    /// Run the HELLO / SCRAM handshake and keep the bearer token.
    pub async fn login(&mut self, user: &str, password: &str) -> Result<()> {
        let mut scram = ScramClient::new(user);
        let url = self.url("about");
        let www = |r: &reqwest::Response| {
            r.headers()
                .get("www-authenticate")
                .and_then(|v| v.to_str().ok())
                .map(parse_auth_header)
                .map(|(_, p)| p)
                .unwrap_or_default()
        };
        // 1. HELLO
        let r = self
            .http
            .get(&url)
            .header("Authorization", scram.hello_header())
            .send()
            .await?;
        if r.status().as_u16() != 401 {
            return Err(ClientError::Auth(format!(
                "expected 401 to HELLO, got {}",
                r.status()
            )));
        }
        let p = www(&r);
        let hs = p
            .get("handshakeToken")
            .cloned()
            .ok_or_else(|| ClientError::Auth("no handshakeToken".into()))?;
        if let Some(h) = p.get("hash")
            && !h.eq_ignore_ascii_case("SHA-256")
        {
            return Err(ClientError::Auth(format!("unsupported SCRAM hash {h}")));
        }
        // 2. client-first
        let r = self
            .http
            .get(&url)
            .header(
                "Authorization",
                format!(
                    "SCRAM handshakeToken={hs}, data={}",
                    b64url(scram.client_first().as_bytes())
                ),
            )
            .send()
            .await?;
        if r.status().as_u16() != 401 {
            return Err(ClientError::Auth(format!(
                "expected 401 to client-first, got {}",
                r.status()
            )));
        }
        let p = www(&r);
        let hs = p.get("handshakeToken").cloned().unwrap_or(hs);
        let server_first = p
            .get("data")
            .and_then(|d| b64url_decode(d).ok())
            .and_then(|b| String::from_utf8(b).ok())
            .ok_or_else(|| ClientError::Auth("no server-first data".into()))?;
        let client_final = scram
            .client_final(&server_first, password)
            .map_err(|e| ClientError::Auth(e.to_string()))?;
        // 3. client-final
        let r = self
            .http
            .get(&url)
            .header(
                "Authorization",
                format!(
                    "SCRAM handshakeToken={hs}, data={}",
                    b64url(client_final.as_bytes())
                ),
            )
            .send()
            .await?;
        if !r.status().is_success() {
            return Err(ClientError::Auth(format!(
                "login rejected ({})",
                r.status()
            )));
        }
        let info = r
            .headers()
            .get("authentication-info")
            .and_then(|v| v.to_str().ok())
            .map(parse_auth_header)
            .map(|(_, p)| p)
            .unwrap_or_default();
        if let Some(d) = info
            .get("data")
            .and_then(|d| b64url_decode(d).ok())
            .and_then(|b| String::from_utf8(b).ok())
        {
            scram
                .verify_server_final(&d)
                .map_err(|e| ClientError::Auth(e.to_string()))?;
        }
        let token = info
            .get("authToken")
            .cloned()
            .ok_or_else(|| ClientError::Auth("no authToken".into()))?;
        self.token = Some(token);
        self.oauth_style = false;
        Ok(())
    }

    /// POST a request grid to an op and return the response grid. Error grids
    /// become [`ClientError::Server`].
    pub async fn call(&self, op: &str, req: &Grid) -> Result<Grid> {
        let body = encode_grid(self.format, req)?;
        let mut rb = self
            .http
            .post(self.url(op))
            .header(
                "Content-Type",
                format!("{}; charset=utf-8", self.format.mime()),
            )
            .header("Accept", self.format.mime())
            .body(body);
        if let Some(t) = &self.token {
            let value = if self.oauth_style {
                format!("Bearer {t}")
            } else {
                format!("BEARER authToken={t}")
            };
            rb = rb.header("Authorization", value);
        }
        let r = rb.send().await?;
        let status = r.status();
        let ct = r
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        let text = r.text().await?;
        if !status.is_success() {
            return Err(ClientError::Status {
                status: status.as_u16(),
                body: text,
            });
        }
        let fmt = if ct.starts_with("text/zinc") || ct.starts_with("text/plain") {
            Format::Zinc
        } else {
            Format::sniff(&text)
        };
        let g = decode_grid(fmt, &text)?;
        if let Some(meta) = &g.meta
            && meta.has("err")
        {
            let dis = match meta.get("dis") {
                Some(Value::Str(s)) => s.value().to_string(),
                _ => "unknown error".into(),
            };
            return Err(ClientError::Server(dis));
        }
        Ok(g)
    }

    // ------------------------------------------------------------ ops

    pub async fn about(&self) -> Result<Dict> {
        Ok(self
            .call("about", &Grid::make_empty())
            .await?
            .rows
            .into_iter()
            .next()
            .unwrap_or_default())
    }

    pub async fn ops(&self) -> Result<Grid> {
        self.call("ops", &Grid::make_empty()).await
    }

    pub async fn read(&self, filter: &str, limit: Option<usize>) -> Result<Vec<Dict>> {
        let mut d = Dict::new();
        d.insert("filter".into(), Value::make_str(filter));
        if let Some(l) = limit {
            d.insert("limit".into(), Value::make_number(l as f64));
        }
        Ok(self
            .call("read", &Grid::make_from_dicts(vec![d]))
            .await?
            .rows)
    }

    /// Read by id. Missing ids come back as `None`.
    pub async fn read_ids(&self, ids: &[Ref]) -> Result<Vec<Option<Dict>>> {
        let rows = ids
            .iter()
            .map(|r| one("id", Value::Ref(r.clone())))
            .collect();
        let g = self
            .call("read", &grid_with_cols(None, &["id"], rows))
            .await?;
        Ok(g.rows
            .into_iter()
            .map(|d| if d.has("id") { Some(d) } else { None })
            .collect())
    }

    pub async fn nav(&self, nav_id: Option<&str>) -> Result<Vec<Dict>> {
        let req = match nav_id {
            Some(n) => Grid::make_from_dicts(vec![one("navId", Value::make_str(n))]),
            None => Grid::make_empty(),
        };
        Ok(self.call("nav", &req).await?.rows)
    }

    /// `range` is Haystack range syntax: `today`, `yesterday`, `2024-01-01`,
    /// `2024-01-01,2024-01-31`, or two datetimes.
    pub async fn his_read(&self, id: &Ref, range: &str) -> Result<Grid> {
        let mut d = Dict::new();
        d.insert("id".into(), Value::Ref(id.clone()));
        d.insert("range".into(), Value::make_str(range));
        self.call("hisRead", &Grid::make_from_dicts(vec![d])).await
    }

    pub async fn his_write(&self, id: &Ref, items: &[(DateTime, Value)]) -> Result<()> {
        let mut meta = Dict::new();
        meta.insert("id".into(), Value::Ref(id.clone()));
        let rows = items
            .iter()
            .map(|(ts, v)| {
                let mut d = Dict::new();
                d.insert("ts".into(), Value::make_datetime(*ts));
                d.insert("val".into(), v.clone());
                d
            })
            .collect();
        self.call(
            "hisWrite",
            &grid_with_cols(Some(meta), &["ts", "val"], rows),
        )
        .await?;
        Ok(())
    }

    /// Write `val` at `level` (1-16); `None` releases the level.
    pub async fn point_write(
        &self,
        id: &Ref,
        level: u8,
        val: Option<Value>,
        who: &str,
    ) -> Result<()> {
        let mut d = Dict::new();
        d.insert("id".into(), Value::Ref(id.clone()));
        d.insert("level".into(), Value::make_number(level as f64));
        d.insert("who".into(), Value::make_str(who));
        d.insert("val".into(), val.unwrap_or(Value::Null));
        let req = grid_with_cols(None, &["id", "level", "val", "who"], vec![d]);
        self.call("pointWrite", &req).await?;
        Ok(())
    }

    pub async fn point_write_array(&self, id: &Ref) -> Result<Grid> {
        self.call(
            "pointWrite",
            &Grid::make_from_dicts(vec![one("id", Value::Ref(id.clone()))]),
        )
        .await
    }

    /// Open a watch. Returns the watch id and the initial records.
    pub async fn watch_sub(
        &self,
        dis: &str,
        ids: &[Ref],
        lease_secs: Option<f64>,
    ) -> Result<(String, Vec<Dict>)> {
        let mut meta = Dict::new();
        meta.insert("watchDis".into(), Value::make_str(dis));
        if let Some(l) = lease_secs {
            meta.insert(
                "lease".into(),
                Value::make_number_unit(l, libhaystack::units::get_unit("s").expect("s")),
            );
        }
        let rows = ids
            .iter()
            .map(|r| one("id", Value::Ref(r.clone())))
            .collect();
        let g = self
            .call("watchSub", &grid_with_cols(Some(meta), &["id"], rows))
            .await?;
        let wid = match g.meta.as_ref().and_then(|m| m.get("watchId")) {
            Some(Value::Str(s)) => s.value().to_string(),
            _ => {
                return Err(ClientError::Server(
                    "watchSub response has no watchId".into(),
                ));
            }
        };
        Ok((wid, g.rows))
    }

    pub async fn watch_poll(&self, watch_id: &str, refresh: bool) -> Result<Vec<Dict>> {
        let mut meta = Dict::new();
        meta.insert("watchId".into(), Value::make_str(watch_id));
        if refresh {
            meta.insert("refresh".into(), Value::make_marker());
        }
        Ok(self
            .call("watchPoll", &grid_with_cols(Some(meta), &["empty"], vec![]))
            .await?
            .rows)
    }

    pub async fn watch_close(&self, watch_id: &str) -> Result<()> {
        let mut meta = Dict::new();
        meta.insert("watchId".into(), Value::make_str(watch_id));
        meta.insert("close".into(), Value::make_marker());
        self.call(
            "watchUnsub",
            &grid_with_cols(Some(meta), &["empty"], vec![]),
        )
        .await?;
        Ok(())
    }

    pub async fn invoke_action(&self, id: &Ref, action: &str, args: Dict) -> Result<Grid> {
        let mut meta = Dict::new();
        meta.insert("id".into(), Value::Ref(id.clone()));
        meta.insert("action".into(), Value::make_str(action));
        let g = if args.is_empty() {
            grid_with_cols(Some(meta), &["empty"], vec![])
        } else {
            let mut g = Grid::make_from_dicts(vec![args]);
            g.meta = Some(meta);
            g
        };
        self.call("invokeAction", &g).await
    }
}

fn one(k: &str, v: Value) -> Dict {
    let mut d = Dict::new();
    d.insert(k.into(), v);
    d
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
