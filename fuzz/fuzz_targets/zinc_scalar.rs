#![no_main]

//! One Zinc scalar, as in a GET query parameter (`?filter=...&id=@x`).

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    if let Ok(v) = openhay_codec::decode_zinc_scalar(text) {
        let _ = openhay_codec::encode_zinc_scalar(&v);
    }
});
