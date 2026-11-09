#![no_main]

use libfuzzer_sys::fuzz_target;
use openhay_codec::Format;

fuzz_target!(|data: &[u8]| openhay_fuzz::decode_roundtrip(Format::Zinc, data));
