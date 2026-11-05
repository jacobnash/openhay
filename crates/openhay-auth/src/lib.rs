//! Haystack authentication (<https://project-haystack.org/doc/docHaystack/Auth>).
//!
//! The flow every Haystack client speaks, SkySpark and Haxall included:
//!
//! 1. `Authorization: HELLO username=<b64url(user)>` → 401 with
//!    `WWW-Authenticate: SCRAM hash=SHA-256, handshakeToken=…`
//! 2. `Authorization: SCRAM handshakeToken=…, data=<b64url(client-first)>` → 401
//!    with the server-first message
//! 3. `Authorization: SCRAM handshakeToken=…, data=<b64url(client-final)>` → 200
//!    with `Authentication-Info: authToken=…, data=<b64url(v=server-sig)>`
//! 4. Every later request: `Authorization: BEARER authToken=…`
//!
//! SCRAM is RFC 5802 / 7677 with SHA-256 and no channel binding (`c=biws`).
//! This crate is transport-agnostic: no HTTP, just messages and header parsing.

use base64::Engine;
use base64::engine::general_purpose::{STANDARD, URL_SAFE_NO_PAD};
use hmac::{Hmac, Mac};
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::collections::HashMap;

type HmacSha256 = Hmac<Sha256>;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum AuthError {
    #[error("malformed auth message: {0}")]
    Malformed(String),
    #[error("nonce mismatch")]
    Nonce,
    #[error("invalid credentials")]
    BadProof,
    #[error("server signature did not verify")]
    BadServerSignature,
}

pub type Result<T> = std::result::Result<T, AuthError>;

pub fn b64url(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

pub fn b64url_decode(s: &str) -> Result<Vec<u8>> {
    URL_SAFE_NO_PAD
        .decode(s.trim_end_matches('='))
        .map_err(|e| AuthError::Malformed(format!("base64url: {e}")))
}

pub fn random_token(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::rng().fill_bytes(&mut buf);
    b64url(&buf)
}

/// Parse an `Authorization` / `WWW-Authenticate` / `Authentication-Info` value
/// such as `SCRAM handshakeToken=abc, hash=SHA-256` into
/// (`Some("SCRAM")`, {handshakeToken: abc, hash: SHA-256}). The scheme is
/// returned upper-cased; `Authentication-Info` has no scheme.
pub fn parse_auth_header(value: &str) -> (Option<String>, HashMap<String, String>) {
    let value = value.trim();
    let (scheme, rest) = match value.split_once(char::is_whitespace) {
        Some((first, rest)) if !first.contains('=') => (Some(first.to_ascii_uppercase()), rest),
        _ if !value.contains('=') && !value.is_empty() => (Some(value.to_ascii_uppercase()), ""),
        _ => (None, value),
    };
    let params = rest
        .split(',')
        .filter_map(|kv| {
            let (k, v) = kv.trim().split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    (scheme, params)
}

fn parse_scram_attrs(msg: &str) -> HashMap<char, String> {
    msg.split(',')
        .filter_map(|kv| {
            let mut chars = kv.chars();
            let k = chars.next()?;
            if chars.next()? != '=' {
                return None;
            }
            Some((k, kv[2..].to_string()))
        })
        .collect()
}

fn hmac(key: &[u8], msg: &[u8]) -> Vec<u8> {
    let mut m = HmacSha256::new_from_slice(key).expect("hmac accepts any key length");
    m.update(msg);
    m.finalize().into_bytes().to_vec()
}

fn xor(a: &[u8], b: &[u8]) -> Vec<u8> {
    a.iter().zip(b).map(|(x, y)| x ^ y).collect()
}

fn salted_password(password: &str, salt: &[u8], iterations: u32) -> [u8; 32] {
    let mut out = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<Sha256>(password.as_bytes(), salt, iterations, &mut out);
    out
}

/// What a server stores per user. Never the password itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScramCredentials {
    pub salt: Vec<u8>,
    pub iterations: u32,
    pub stored_key: Vec<u8>,
    pub server_key: Vec<u8>,
}

impl ScramCredentials {
    pub const DEFAULT_ITERATIONS: u32 = 10_000;

    pub fn derive(password: &str, salt: &[u8], iterations: u32) -> Self {
        Self::from_salted_password(
            &salted_password(password, salt, iterations),
            salt,
            iterations,
        )
    }

    /// Build credentials from an already-computed SCRAM SaltedPassword,
    /// i.e. `PBKDF2-HMAC-SHA256(password, salt, iterations)` with a 32-byte
    /// output. Any system that already stores passwords that way (Django's
    /// `pbkdf2_sha256`, Timberdoodle's `salt$dk`, ...) can serve SCRAM logins
    /// for existing users without a password reset.
    pub fn from_salted_password(salted: &[u8], salt: &[u8], iterations: u32) -> Self {
        let client_key = hmac(salted, b"Client Key");
        Self {
            salt: salt.to_vec(),
            iterations,
            stored_key: Sha256::digest(&client_key).to_vec(),
            server_key: hmac(salted, b"Server Key"),
        }
    }

    pub fn new_random(password: &str) -> Self {
        let mut salt = [0u8; 16];
        rand::rng().fill_bytes(&mut salt);
        Self::derive(password, &salt, Self::DEFAULT_ITERATIONS)
    }
}

// ---------------- client half ----------------

/// Client side of one SCRAM exchange.
pub struct ScramClient {
    username: String,
    nonce: String,
    client_first_bare: String,
    expected_server_sig: Option<Vec<u8>>,
}

impl ScramClient {
    pub fn new(username: &str) -> Self {
        Self::with_nonce(username, &random_token(24))
    }

    pub fn with_nonce(username: &str, nonce: &str) -> Self {
        let escaped = username.replace('=', "=3D").replace(',', "=2C");
        Self {
            username: username.to_string(),
            nonce: nonce.to_string(),
            client_first_bare: format!("n={escaped},r={nonce}"),
            expected_server_sig: None,
        }
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    /// `HELLO` header value.
    pub fn hello_header(&self) -> String {
        format!("HELLO username={}", b64url(self.username.as_bytes()))
    }

    /// Full client-first message (with gs2 header), unencoded.
    pub fn client_first(&self) -> String {
        format!("n,,{}", self.client_first_bare)
    }

    /// Given the (decoded) server-first message, produce the (unencoded)
    /// client-final message.
    pub fn client_final(&mut self, server_first: &str, password: &str) -> Result<String> {
        let attrs = parse_scram_attrs(server_first);
        let r = attrs
            .get(&'r')
            .ok_or_else(|| AuthError::Malformed("server-first missing r".into()))?;
        if !r.starts_with(&self.nonce) {
            return Err(AuthError::Nonce);
        }
        let salt = STANDARD
            .decode(
                attrs
                    .get(&'s')
                    .ok_or_else(|| AuthError::Malformed("missing s".into()))?,
            )
            .map_err(|e| AuthError::Malformed(format!("salt: {e}")))?;
        let iterations: u32 = attrs
            .get(&'i')
            .and_then(|i| i.parse().ok())
            .ok_or_else(|| AuthError::Malformed("missing i".into()))?;
        let salted = salted_password(password, &salt, iterations);
        let client_key = hmac(&salted, b"Client Key");
        let stored_key = Sha256::digest(&client_key);
        let without_proof = format!("c=biws,r={r}");
        let auth_message = format!(
            "{},{},{}",
            self.client_first_bare, server_first, without_proof
        );
        let client_sig = hmac(&stored_key, auth_message.as_bytes());
        let proof = xor(&client_key, &client_sig);
        let server_key = hmac(&salted, b"Server Key");
        self.expected_server_sig = Some(hmac(&server_key, auth_message.as_bytes()));
        Ok(format!("{without_proof},p={}", STANDARD.encode(proof)))
    }

    /// Verify the server's `v=` signature from the final `Authentication-Info`.
    pub fn verify_server_final(&self, server_final: &str) -> Result<()> {
        let attrs = parse_scram_attrs(server_final);
        let v = attrs
            .get(&'v')
            .ok_or_else(|| AuthError::Malformed("missing v".into()))?;
        let got = STANDARD
            .decode(v)
            .map_err(|e| AuthError::Malformed(format!("v: {e}")))?;
        match &self.expected_server_sig {
            Some(exp) if *exp == got => Ok(()),
            _ => Err(AuthError::BadServerSignature),
        }
    }
}

// ---------------- server half ----------------

/// Server state between the client-first and client-final messages.
#[derive(Debug, Clone)]
pub struct ScramServerState {
    pub username: String,
    client_first_bare: String,
    server_first: String,
    combined_nonce: String,
}

impl ScramServerState {
    /// Parse the (decoded) client-first message. Returns the username so the
    /// caller can look up credentials, and the state to continue with.
    pub fn parse_client_first(client_first: &str) -> Result<(String, String, String)> {
        let bare = client_first
            .strip_prefix("n,,")
            .or_else(|| client_first.strip_prefix("y,,"))
            .ok_or_else(|| AuthError::Malformed("unsupported gs2 header".into()))?;
        let attrs = parse_scram_attrs(bare);
        let user = attrs
            .get(&'n')
            .ok_or_else(|| AuthError::Malformed("client-first missing n".into()))?
            .replace("=2C", ",")
            .replace("=3D", "=");
        let nonce = attrs
            .get(&'r')
            .ok_or_else(|| AuthError::Malformed("client-first missing r".into()))?;
        Ok((user, nonce.clone(), bare.to_string()))
    }

    /// Start the server side once credentials are known. Returns the state
    /// and the (unencoded) server-first message.
    pub fn start(client_first: &str, creds: &ScramCredentials) -> Result<(Self, String)> {
        let (username, client_nonce, client_first_bare) = Self::parse_client_first(client_first)?;
        let combined_nonce = format!("{client_nonce}{}", random_token(18));
        let server_first = format!(
            "r={combined_nonce},s={},i={}",
            STANDARD.encode(&creds.salt),
            creds.iterations
        );
        Ok((
            Self {
                username,
                client_first_bare,
                server_first: server_first.clone(),
                combined_nonce,
            },
            server_first,
        ))
    }

    /// Verify the client-final proof. Returns the (unencoded) server-final
    /// message (`v=…`) on success.
    pub fn finish(&self, client_final: &str, creds: &ScramCredentials) -> Result<String> {
        let (without_proof, proof_b64) = client_final
            .rsplit_once(",p=")
            .ok_or_else(|| AuthError::Malformed("client-final missing p".into()))?;
        let attrs = parse_scram_attrs(without_proof);
        if attrs.get(&'r') != Some(&self.combined_nonce) {
            return Err(AuthError::Nonce);
        }
        let proof = STANDARD
            .decode(proof_b64)
            .map_err(|_| AuthError::BadProof)?;
        let auth_message = format!(
            "{},{},{}",
            self.client_first_bare, self.server_first, without_proof
        );
        let client_sig = hmac(&creds.stored_key, auth_message.as_bytes());
        let client_key = xor(&proof, &client_sig);
        let ok = proof.len() == client_sig.len()
            && constant_time_eq(&Sha256::digest(&client_key), &creds.stored_key);
        if !ok {
            return Err(AuthError::BadProof);
        }
        let server_sig = hmac(&creds.server_key, auth_message.as_bytes());
        Ok(format!("v={}", STANDARD.encode(server_sig)))
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rfc7677_vector() {
        // RFC 7677 §3 test vector (user "user", password "pencil").
        let salt = STANDARD.decode("W22ZaJ0SNY7soEsUEjb6gQ==").unwrap();
        let mut c = ScramClient::with_nonce("user", "rOprNGfwEbeRWgbNEkqO");
        let server_first = "r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,s=W22ZaJ0SNY7soEsUEjb6gQ==,i=4096";
        let fin = c.client_final(server_first, "pencil").unwrap();
        assert_eq!(
            fin,
            "c=biws,r=rOprNGfwEbeRWgbNEkqO%hvYDpWUa2RaTCAfuxFIlj)hNlF$k0,p=dHzbZapWIk4jUhN+Ute9ytag9zjfMHgsqmmiz7AndVQ="
        );
        c.verify_server_final("v=6rriTRBi23WpRR/wtup+mMhUZUn/dB5nLTJRsjl95G4=")
            .unwrap();
        let creds = ScramCredentials::derive("pencil", &salt, 4096);
        assert_eq!(creds.salt, salt);
    }

    #[test]
    fn client_server_roundtrip() {
        let creds = ScramCredentials::new_random("s3cret");
        let mut c = ScramClient::new("alice,admin");
        let (state, sf) = ScramServerState::start(&c.client_first(), &creds).unwrap();
        assert_eq!(state.username, "alice,admin");
        let cf = c.client_final(&sf, "s3cret").unwrap();
        let sfin = state.finish(&cf, &creds).unwrap();
        c.verify_server_final(&sfin).unwrap();
    }

    #[test]
    fn from_salted_matches_derive() {
        let salt = b"0123456789abcdef";
        let salted = salted_password("pw", salt, 1000);
        assert_eq!(
            ScramCredentials::from_salted_password(&salted, salt, 1000),
            ScramCredentials::derive("pw", salt, 1000)
        );
    }

    #[test]
    fn wrong_password_rejected() {
        let creds = ScramCredentials::new_random("right");
        let mut c = ScramClient::new("bob");
        let (state, sf) = ScramServerState::start(&c.client_first(), &creds).unwrap();
        let cf = c.client_final(&sf, "wrong").unwrap();
        assert_eq!(state.finish(&cf, &creds), Err(AuthError::BadProof));
    }

    #[test]
    fn header_parsing() {
        let (s, p) = parse_auth_header("SCRAM handshakeToken=abc, hash=SHA-256");
        assert_eq!(s.as_deref(), Some("SCRAM"));
        assert_eq!(p["handshakeToken"], "abc");
        assert_eq!(p["hash"], "SHA-256");
        let (s, p) = parse_auth_header("authToken=xyz, data=dj1hYmM");
        assert_eq!(s, None);
        assert_eq!(p["authToken"], "xyz");
        let (s, p) = parse_auth_header("bearer authToken=t");
        assert_eq!(s.as_deref(), Some("BEARER"));
        assert_eq!(p["authToken"], "t");
    }
}
