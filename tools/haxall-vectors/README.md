# Regenerating the haxall conformance vectors

Inputs come from haxall's test suite; expected values come from the reference
runtime itself. This keeps openhay's tests honest about what Haxall actually does.

1. Get the haxall release (the runtime) and the test sources **at the same tag**:
   ```bash
   curl -LO https://github.com/haxall/haxall/releases/download/v4.0.6/haxall-4.0.6.zip && unzip haxall-4.0.6.zip
   git clone --depth 1 --branch v4.0.6 https://github.com/haxall/haxall haxall-src
   ```
2. Extract the inputs, then run them through the reference runtime (needs Java):
   ```bash
   python3 extract.py haxall-src/src/test/testHaystack/fan inputs.json
   haxall-4.0.6/bin/fan gen.fan inputs.json vectors.json
   ```
3. Wrap `vectors.json` with a `source` block (tag, commit, license) and save it as
   `crates/openhay-py/python/openhay/vectors/haxall/testHaystack-<version>.json`.

`extract.py` blanks out `/* ... */` comment blocks first, so commented-out tests
such as ZincTest's "NO LONGER SUPPORTED" Haystack 1.0 cases aren't picked up.
`gen.fan` is our own code (BSD-3-Clause) that calls haxall's public
ZincReader, TrioReader, JsonWriter and Filter APIs.
