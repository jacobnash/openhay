//! `openhay._native`: the Rust half of the Python package. Kept deliberately
//! thin. Grids cross the boundary as Hayson text, and the pure-Python layer
//! (`python/openhay/__init__.py`) turns Hayson into Python values. One
//! conversion path, no per-type FFI glue to drift.

use libhaystack::val::{Grid, Value};
use openhay_auth as auth;
use openhay_codec::Format;
use pyo3::create_exception;
use pyo3::exceptions::{PyException, PyValueError};
use pyo3::prelude::*;
use pyo3::types::PyBytes;

create_exception!(
    _native,
    HaystackError,
    PyException,
    "A Haystack error grid or protocol failure."
);
create_exception!(_native, AuthError, HaystackError, "Authentication failed.");

fn fmt(name: &str) -> PyResult<Format> {
    Format::from_name(name).ok_or_else(|| PyValueError::new_err(format!("unknown format {name:?}")))
}

fn codec_err(e: openhay_codec::CodecError) -> PyErr {
    PyValueError::new_err(e.to_string())
}

fn grid_from_hayson(text: &str) -> PyResult<Grid> {
    openhay_codec::decode_grid(Format::Hayson, text).map_err(codec_err)
}

/// Convert grid text between formats. `src=None` sniffs the input.
#[pyfunction]
#[pyo3(signature = (text, to, src=None))]
fn convert(py: Python<'_>, text: &str, to: &str, src: Option<&str>) -> PyResult<String> {
    let to = fmt(to)?;
    let src = src.map(fmt).transpose()?;
    py.detach(|| {
        let grid = match src {
            Some(f) => openhay_codec::decode_grid(f, text),
            None => openhay_codec::decode_grid_auto(text),
        }
        .map_err(codec_err)?;
        openhay_codec::encode_grid(to, &grid).map_err(codec_err)
    })
}

/// Encode a single value given as Hayson JSON to Zinc (e.g. for GET params).
#[pyfunction]
fn hayson_value_to_zinc(hayson: &str) -> PyResult<String> {
    let v: Value =
        serde_json::from_str(hayson).map_err(|e| PyValueError::new_err(e.to_string()))?;
    openhay_codec::encode_zinc_scalar(&v).map_err(codec_err)
}

/// Decode a single Zinc scalar (e.g. a GET query parameter) to Hayson JSON.
#[pyfunction]
fn zinc_value_to_hayson(zinc: &str) -> PyResult<String> {
    let v = openhay_codec::decode_zinc_scalar(zinc).map_err(codec_err)?;
    serde_json::to_string(&v).map_err(|e| PyValueError::new_err(e.to_string()))
}

#[pyfunction]
fn sniff(text: &str) -> &'static str {
    Format::sniff(text).name()
}

/// Pick a response format from an `Accept` header.
#[pyfunction]
#[pyo3(signature = (accept, json_flavor="hayson"))]
fn negotiate(accept: Option<&str>, json_flavor: &str) -> PyResult<&'static str> {
    Ok(Format::negotiate(accept, fmt(json_flavor)?).name())
}

#[pyfunction]
fn mime(format: &str) -> PyResult<&'static str> {
    Ok(fmt(format)?.mime())
}

// ------------------------------------------------------------------ auth

#[pyfunction]
fn b64url(data: &[u8]) -> String {
    auth::b64url(data)
}

#[pyfunction]
fn b64url_decode<'py>(py: Python<'py>, data: &str) -> PyResult<Bound<'py, PyBytes>> {
    let b = auth::b64url_decode(data).map_err(|e| PyValueError::new_err(e.to_string()))?;
    Ok(PyBytes::new(py, &b))
}

#[pyfunction]
fn random_token(nbytes: usize) -> String {
    auth::random_token(nbytes)
}

/// `"SCRAM handshakeToken=x, data=y"` -> `("SCRAM", {"handshakeToken": "x", "data": "y"})`.
#[pyfunction]
fn parse_auth_header(value: &str) -> (Option<String>, std::collections::HashMap<String, String>) {
    auth::parse_auth_header(value)
}

/// What a server stores per user for SCRAM-SHA-256.
#[pyclass(frozen, from_py_object)]
#[derive(Clone)]
struct ScramCredentials {
    inner: auth::ScramCredentials,
}

#[pymethods]
impl ScramCredentials {
    #[new]
    fn new(salt: Vec<u8>, iterations: u32, stored_key: Vec<u8>, server_key: Vec<u8>) -> Self {
        Self {
            inner: auth::ScramCredentials {
                salt,
                iterations,
                stored_key,
                server_key,
            },
        }
    }

    #[staticmethod]
    fn derive(py: Python<'_>, password: &str, salt: Vec<u8>, iterations: u32) -> Self {
        let password = password.to_string();
        py.detach(move || Self {
            inner: auth::ScramCredentials::derive(&password, &salt, iterations),
        })
    }

    /// From an already-computed PBKDF2-HMAC-SHA256 salted password (32 bytes).
    #[staticmethod]
    fn from_salted_password(salted: &[u8], salt: &[u8], iterations: u32) -> Self {
        Self {
            inner: auth::ScramCredentials::from_salted_password(salted, salt, iterations),
        }
    }

    #[getter]
    fn salt<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.salt)
    }
    #[getter]
    fn iterations(&self) -> u32 {
        self.inner.iterations
    }
    #[getter]
    fn stored_key<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.stored_key)
    }
    #[getter]
    fn server_key<'py>(&self, py: Python<'py>) -> Bound<'py, PyBytes> {
        PyBytes::new(py, &self.inner.server_key)
    }
}

/// Server state between SCRAM client-first and client-final.
#[pyclass(frozen)]
struct ScramServer {
    inner: auth::ScramServerState,
}

#[pymethods]
impl ScramServer {
    /// Parse a decoded client-first message and return the username it names
    /// (so the caller can look up credentials before calling `start`).
    #[staticmethod]
    fn username(client_first: &str) -> PyResult<String> {
        auth::ScramServerState::parse_client_first(client_first)
            .map(|(u, _, _)| u)
            .map_err(|e| AuthError::new_err(e.to_string()))
    }

    /// Returns `(state, server_first_message)`.
    #[staticmethod]
    fn start(client_first: &str, creds: &ScramCredentials) -> PyResult<(Self, String)> {
        let (inner, server_first) = auth::ScramServerState::start(client_first, &creds.inner)
            .map_err(|e| AuthError::new_err(e.to_string()))?;
        Ok((Self { inner }, server_first))
    }

    /// Verify the client proof; returns the server-final message (`v=...`).
    fn finish(&self, client_final: &str, creds: &ScramCredentials) -> PyResult<String> {
        self.inner
            .finish(client_final, &creds.inner)
            .map_err(|e| AuthError::new_err(e.to_string()))
    }

    #[getter]
    fn user(&self) -> String {
        self.inner.username.clone()
    }
}

// ------------------------------------------------------------------ client

/// Blocking Haystack client. Grids go in and out as Hayson text; use the
/// `openhay.Client` wrapper rather than this class directly.
#[pyclass]
struct NativeClient {
    inner: openhay_client::Client,
    rt: tokio::runtime::Runtime,
}

fn client_err(e: openhay_client::ClientError) -> PyErr {
    match e {
        openhay_client::ClientError::Auth(m) => AuthError::new_err(m),
        openhay_client::ClientError::Status {
            status: 401 | 403,
            body,
        } => AuthError::new_err(body),
        other => HaystackError::new_err(other.to_string()),
    }
}

#[pymethods]
impl NativeClient {
    #[new]
    #[pyo3(signature = (base, format="zinc"))]
    fn new(base: &str, format: &str) -> PyResult<Self> {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| HaystackError::new_err(e.to_string()))?;
        let mut inner = openhay_client::Client::new(base);
        inner.format = fmt(format)?;
        Ok(Self { inner, rt })
    }

    fn login(&mut self, py: Python<'_>, user: &str, password: &str) -> PyResult<()> {
        let Self { inner, rt } = self;
        py.detach(|| rt.block_on(inner.login(user, password)))
            .map_err(client_err)
    }

    /// Use an existing token: `oauth=True` sends `Bearer <token>`, otherwise
    /// Haystack's `BEARER authToken=<token>`.
    #[pyo3(signature = (token, oauth=true))]
    fn set_token(&mut self, token: &str, oauth: bool) {
        let c = std::mem::replace(
            &mut self.inner,
            openhay_client::Client::new("http://unused/"),
        );
        let fmt = c.format;
        self.inner = if oauth {
            c.with_bearer(token)
        } else {
            c.with_token(token)
        };
        self.inner.format = fmt;
    }

    #[getter]
    fn token(&self) -> Option<String> {
        self.inner.token().map(str::to_string)
    }

    /// POST a Hayson request grid to `op`; returns the Hayson response grid.
    fn call(&self, py: Python<'_>, op: &str, request_hayson: &str) -> PyResult<String> {
        let req = grid_from_hayson(request_hayson)?;
        let Self { inner, rt } = self;
        let grid = py
            .detach(|| rt.block_on(inner.call(op, &req)))
            .map_err(client_err)?;
        openhay_codec::encode_grid(Format::Hayson, &grid).map_err(codec_err)
    }
}

/// The `defs`/`libs`/`filetypes` op over the bundled Haystack 4 namespace,
/// as a Hayson grid.
#[pyfunction]
#[pyo3(signature = (op, filter=None, limit=None))]
fn defs_op(op: &str, filter: Option<&str>, limit: Option<usize>) -> PyResult<String> {
    let g =
        openhay_defs::op(op, filter, limit).map_err(|e| PyValueError::new_err(e.to_string()))?;
    openhay_codec::encode_grid(Format::Hayson, &g).map_err(codec_err)
}

/// Indices of a Hayson grid's rows that match a Haystack filter (evaluated
/// with the defs namespace, so `^symbol` comparisons work).
#[pyfunction]
fn filter_indices(filter: &str, grid_hayson: &str) -> PyResult<Vec<usize>> {
    let g = grid_from_hayson(grid_hayson)?;
    openhay_defs::filter_indices(filter, &g.rows).map_err(|e| PyValueError::new_err(e.to_string()))
}

fn hayson_dict(text: &str) -> PyResult<libhaystack::val::Dict> {
    match serde_json::from_str::<Value>(text).map_err(|e| PyValueError::new_err(e.to_string()))? {
        Value::Dict(d) => Ok(d),
        _ => Err(PyValueError::new_err("tags must be a dict")),
    }
}

/// Problems with a Hayson tag dict as a Haystack 4 entity:
/// `(level, kind, tag, options, message)`, errors first.
#[pyfunction]
#[allow(clippy::type_complexity)]
fn validate_tags(
    tags_hayson: &str,
) -> PyResult<
    Vec<(
        &'static str,
        &'static str,
        Option<String>,
        Vec<String>,
        String,
    )>,
> {
    let d = hayson_dict(tags_hayson)?;
    Ok(openhay_defs::validate::validate(&d)
        .into_iter()
        .map(|p| (p.level.as_str(), p.kind, p.tag, p.options, p.message))
        .collect())
}

/// Protos (sorted marker lists) the tags fit, most specific first.
#[pyfunction]
fn protos_matching(tags_hayson: &str) -> PyResult<Vec<Vec<String>>> {
    let d = hayson_dict(tags_hayson)?;
    Ok(openhay_defs::validate::protos_matching(&d)
        .into_iter()
        .cloned()
        .collect())
}

/// The `n` protos nearest the tags.
#[pyfunction]
fn nearest_protos(tags_hayson: &str, n: usize) -> PyResult<Vec<Vec<String>>> {
    let d = hayson_dict(tags_hayson)?;
    Ok(openhay_defs::validate::nearest_protos(&d, n)
        .into_iter()
        .cloned()
        .collect())
}

/// Every Haystack 4 proto, as sorted marker lists.
#[pyfunction]
fn protos() -> Vec<Vec<String>> {
    openhay_defs::protos::all().to_vec()
}

#[pymodule]
fn _native(m: &Bound<'_, PyModule>) -> PyResult<()> {
    m.add("__version__", env!("CARGO_PKG_VERSION"))?;
    m.add("HaystackError", m.py().get_type::<HaystackError>())?;
    m.add("AuthError", m.py().get_type::<AuthError>())?;
    m.add_function(wrap_pyfunction!(convert, m)?)?;
    m.add_function(wrap_pyfunction!(hayson_value_to_zinc, m)?)?;
    m.add_function(wrap_pyfunction!(zinc_value_to_hayson, m)?)?;
    m.add_function(wrap_pyfunction!(sniff, m)?)?;
    m.add_function(wrap_pyfunction!(negotiate, m)?)?;
    m.add_function(wrap_pyfunction!(mime, m)?)?;
    m.add_function(wrap_pyfunction!(b64url, m)?)?;
    m.add_function(wrap_pyfunction!(b64url_decode, m)?)?;
    m.add_function(wrap_pyfunction!(random_token, m)?)?;
    m.add_function(wrap_pyfunction!(parse_auth_header, m)?)?;
    m.add_function(wrap_pyfunction!(defs_op, m)?)?;
    m.add_function(wrap_pyfunction!(filter_indices, m)?)?;
    m.add_function(wrap_pyfunction!(validate_tags, m)?)?;
    m.add_function(wrap_pyfunction!(protos_matching, m)?)?;
    m.add_function(wrap_pyfunction!(nearest_protos, m)?)?;
    m.add_function(wrap_pyfunction!(protos, m)?)?;
    m.add_class::<ScramCredentials>()?;
    m.add_class::<ScramServer>()?;
    m.add_class::<NativeClient>()?;
    Ok(())
}
