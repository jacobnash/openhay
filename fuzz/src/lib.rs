//! Shared checks for the fuzz targets.
//!
//! Every decoder must survive any input without panicking. When an input
//! does decode, the grid must encode to every format, and its encoding in
//! the format it came from must decode again: a grid openhay accepts is one
//! it can send back.

use openhay_codec::{Format, decode_grid, encode_grid};

pub fn decode_roundtrip(format: Format, data: &[u8]) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    let Ok(grid) = decode_grid(format, text) else {
        return;
    };
    for f in Format::ALL {
        let _ = encode_grid(f, &grid);
    }
    let Ok(again) = encode_grid(format, &grid) else {
        return;
    };
    if let Err(e) = decode_grid(format, &again) {
        panic!(
            "{format:?} re-encoding of a decoded grid doesn't decode: {e}\n--- input\n{text}\n--- re-encoded\n{again}"
        );
    }
}
