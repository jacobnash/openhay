//! Haystack wire formats for both Haystack 3 and Haystack 4 servers.
//!
//! Values come from `libhaystack` (BSD-3, J2 Innovations). The Zinc and Trio
//! readers are openhay's own, checked against haxall's reference test cases
//! (`openhay.vectors`). This crate also adds what is missing for real-world
//! interop:
//!
//! * **JSON v3** — the prefixed-string encoding (`"n:72 °F"`, `"r:abc Dis"`)
//!   that SkySpark 3, Haxall in v3 mode and older nhaystack builds emit.
//! * **CSV** — encode only, as in the Haystack spec (CSV is lossy).
//! * One [`Format`] enum with MIME negotiation and content sniffing, so callers
//!   never branch on wire format themselves.

pub mod csv;
pub mod hayson;
pub mod json3;
pub mod trio_read;
pub mod zinc;
pub mod zinc_read;

use libhaystack::encoding::trio::encode::TrioWriter;
use libhaystack::encoding::zinc::encode::ToZinc;
pub use libhaystack::val::{Dict, Grid, Value};

#[derive(Debug, thiserror::Error)]
pub enum CodecError {
    #[error("zinc: {0}")]
    Zinc(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("trio: {0}")]
    Trio(String),
    #[error("{0}")]
    Invalid(String),
    #[error("format {0:?} cannot be decoded")]
    EncodeOnly(Format),
}

pub type Result<T> = std::result::Result<T, CodecError>;

/// A Haystack wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Format {
    Zinc,
    /// Haystack 3 JSON (prefixed strings).
    Json3,
    /// Haystack 4 JSON (Hayson, `_kind` objects).
    Hayson,
    Trio,
    Csv,
}

impl Format {
    pub const ALL: [Format; 5] = [
        Format::Zinc,
        Format::Json3,
        Format::Hayson,
        Format::Trio,
        Format::Csv,
    ];

    pub fn mime(self) -> &'static str {
        match self {
            Format::Zinc => "text/zinc",
            Format::Json3 | Format::Hayson => "application/json",
            Format::Trio => "text/trio",
            Format::Csv => "text/csv",
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Format::Zinc => "zinc",
            Format::Json3 => "json3",
            Format::Hayson => "hayson",
            Format::Trio => "trio",
            Format::Csv => "csv",
        }
    }

    pub fn from_name(name: &str) -> Option<Format> {
        match name.to_ascii_lowercase().as_str() {
            "zinc" => Some(Format::Zinc),
            "json3" | "json-v3" | "jsonv3" => Some(Format::Json3),
            "hayson" | "json" | "json4" | "json-v4" => Some(Format::Hayson),
            "trio" => Some(Format::Trio),
            "csv" => Some(Format::Csv),
            _ => None,
        }
    }

    /// Pick the response format for an HTTP `Accept` header. `application/json`
    /// maps to `json_flavor` because the MIME type alone does not say which JSON
    /// (the Haystack spec reuses it for both versions). Defaults to Zinc.
    pub fn negotiate(accept: Option<&str>, json_flavor: Format) -> Format {
        let Some(accept) = accept else {
            return Format::Zinc;
        };
        for part in accept.split(',') {
            let mime = part
                .split(';')
                .next()
                .unwrap_or("")
                .trim()
                .to_ascii_lowercase();
            match mime.as_str() {
                "text/zinc" | "text/plain" | "*/*" => return Format::Zinc,
                "application/json" => return json_flavor,
                "text/trio" => return Format::Trio,
                "text/csv" => return Format::Csv,
                _ => {}
            }
        }
        Format::Zinc
    }

    /// Guess the format of a payload from its first non-blank characters.
    pub fn sniff(text: &str) -> Format {
        let t = text.trim_start();
        if t.starts_with('{') || t.starts_with('[') {
            if t.contains("\"_kind\"") {
                Format::Hayson
            } else {
                Format::Json3
            }
        } else if t.starts_with("ver:") {
            Format::Zinc
        } else {
            Format::Trio
        }
    }
}

/// Decode a grid. For [`Format::Trio`] every record becomes a row.
pub fn decode_grid(format: Format, text: &str) -> Result<Grid> {
    match format {
        Format::Zinc => zinc_read::read_grid(text).map_err(CodecError::Zinc),
        Format::Json3 => json3::decode_grid(text),
        Format::Hayson => hayson::decode_grid(text),
        Format::Trio => trio_read::read_grid(text).map_err(CodecError::Trio),
        Format::Csv => Err(CodecError::EncodeOnly(Format::Csv)),
    }
}

/// Decode, sniffing the format first.
pub fn decode_grid_auto(text: &str) -> Result<Grid> {
    decode_grid(Format::sniff(text), text)
}

/// Encode a grid.
pub fn encode_grid(format: Format, grid: &Grid) -> Result<String> {
    match format {
        Format::Zinc => zinc::encode_grid(grid),
        Format::Json3 => Ok(json3::encode_grid(grid).to_string()),
        Format::Hayson => Ok(hayson::encode_grid(grid).to_string()),
        Format::Trio => {
            let mut w = TrioWriter::new();
            for row in &grid.rows {
                w.add_dict(row.clone());
            }
            Ok(w.to_trio_string())
        }
        Format::Csv => Ok(csv::encode_grid(grid)),
    }
}

/// Decode a single scalar in Zinc syntax (e.g. `72°F`, `@abc`, `2024-01-01`).
pub fn decode_zinc_scalar(text: &str) -> Result<Value> {
    zinc_read::read_value(text).map_err(CodecError::Zinc)
}

/// Encode a single value in Zinc syntax.
pub fn encode_zinc_scalar(v: &Value) -> Result<String> {
    v.to_zinc_string()
        .map_err(|e| CodecError::Zinc(e.to_string()))
}
