# openhay

Project Haystack 3 + 4 in Rust: wire formats, client, server and a CLI. No Fantom, no Axon, no license fee.
BSD-3-Clause, so anyone can embed it, including commercial vendors.

Values, filters, units, Hayson, Trio and Zinc scalars come from
[`libhaystack`](https://github.com/j2inn/libhaystack) (BSD-3, J2 Innovations). openhay adds what you
need to talk to real servers and to *be* one.

| crate | what it gives you |
|---|---|
| `openhay-codec` | One `Format` enum covering **Zinc, JSON v3** (SkySpark 3 / older nhaystack prefixed strings), **Hayson** (JSON v4), **Trio** and CSV. Also MIME negotiation and format sniffing. |
| `openhay-auth` | Haystack auth: `HELLO` → SCRAM-SHA-256 → `BEARER authToken`. Client and server halves, checked against the RFC 7677 test vector. `ScramCredentials::from_salted_password` serves SCRAM straight from existing PBKDF2-SHA256 password hashes, so no password reset is needed. |
| `openhay-client` | Async client for SkySpark, Haxall, nhaystack or anything else: login, `about`, `read` (filter or ids; follows a paging server's `after` to the full set, `read_page` for one page and its meta), `nav`, `hisRead`, `hisWrite`, `pointWrite` (+ priority array), `watchSub`/`Poll`/`Unsub`, `invokeAction`. |
| `openhay-filter` | Haystack filters with the reference semantics: typed comparisons (no matches on missing tags, other kinds or other units), paths through Refs, Ref lists, nested Dicts and DateTime `date`/`time`/`tz`, `^symbol` terms, Xeto spec names, hex/`h:mm` literals and `//` comments. Pluggable `Resolver` for whatever store holds the records. |
| `openhay-defs` | The official Project Haystack 4.0.0 defs namespace (`ph`, `phScience`, `phIoT`, `phIct`; 719 defs), bundled, plus the `defs`/`libs`/`filetypes` ops and a filter evaluator that knows the namespace (`lib==^lib:phIoT`). |
| `openhay-server` | Implement one `Backend` trait over your storage and get the whole op surface. It serves both `/api/{op}` (Haxall layout) and `/api/{project}/{op}` (SkySpark layout), with SCRAM auth, content negotiation, Haystack error grids, and **server-managed watches with bounded memory**. `MemBackend` is a complete in-memory implementation. |
| `openhay-py` | Python package `openhay` (PyO3, abi3 wheel for Python 3.10+). It includes value types, `decode_grid`/`encode_grid` for every format, a blocking `Client`, the SCRAM server half for any Python web stack, the bundled defs (`oh.defs("libs")`) and an in-memory filter (`oh.filter_rows`). |
| `hay-cli` | The `hay` binary: `convert`, `read`, `his`, `call`, and `serve` (turn a Trio/Zinc file into a live Haystack server). |

## Quick start

```bash
cargo install --path crates/hay-cli

hay convert site.zinc --to json3                # Zinc -> Haystack 3 JSON
hay convert export.json --to trio               # sniffs JSON v3 vs Hayson

hay serve site.trio --addr 127.0.0.1:8080 --auth admin:pencil
hay read http://127.0.0.1:8080/api/demo "point and equipRef->ahu" --user admin --password pencil
hay his  http://127.0.0.1:8080/api/demo @p1 yesterday --to csv
```

### Auth: Haystack SCRAM *and* OAuth / API keys

SkySpark-style clients log in with HELLO/SCRAM. Modern clients send a plain
`Authorization: Bearer <token>`, for example an OAuth2/OIDC access token, an API key or a JWT. A server can
accept both at once: set `Config::users` for SCRAM and `Config::token_validator` for bearer tokens.
On the client, use `Client::login(user, pw)` for SCRAM or `Client::with_bearer(token)` for OAuth.

### Python

```bash
pip install maturin && (cd crates/openhay-py && maturin develop)
```

```python
import openhay as oh

g = oh.decode_grid(open("export.zinc").read())          # or json3 / hayson / trio, sniffed
print(oh.encode_grid(g, "json3"))

c = oh.Client("https://host/api/demo", user="admin", password="…")   # SCRAM
c = oh.Client("https://host/api/demo", bearer=access_token)          # OAuth / API key
for rec in c.read("point and his and equipRef->ahu"):
    his = c.his_read(rec["id"], "yesterday")
```

`read` returns every match. Haystack has no standard paging, but some servers (e.g. Baler) cut a
filter read into pages and mark a short page's grid meta with `incomplete`, `limit` and `after`.
`read` follows `after` until the set is complete, or until `limit` records are in. A server that
doesn't page (Haxall, SkySpark) gets the same single request as before. To take one page at a
time and see the meta:

```python
page = c.read_page("point")                        # oh.Grid
page.meta.get("after")                              # cursor, or None on the last page
page = c.read_page("point", after=page.meta["after"])
```

Embedding the server:

```rust
use openhay_server::{Backend, OpResult, Server, Config, router};

struct MyStore { /* postgres, sqlite, whatever */ }

impl Backend for MyStore {
    async fn read_filter(&self, filter: &str, limit: Option<usize>) -> OpResult<Vec<Dict>> { /* ... */ }
    async fn read_ids(&self, ids: &[Ref]) -> OpResult<Vec<Option<Dict>>> { /* ... */ }
    // his_read / his_write / point_write / nav / invoke_action are optional
}
```

## Status

The crates above work, with end-to-end tests that run the client against the server over real HTTP in
Zinc, JSON v3 and Hayson.

Roadmap:

- [x] Python bindings (PyO3 / maturin)
- [ ] Publish wheels to PyPI (CI with maturin-action)
- [ ] TypeScript / WASM bindings
- [ ] `openhay-ws`: an open WebSocket transport with server-push watches
- [x] Haystack 4 defs namespace exposed through `defs`/`libs`/`filetypes` ops
- [ ] Xeto: parser and `xeto → anything` converter (Trio defs, JSON Schema, SHACL, TS/Python types)
- [x] Conformance suite against Haxall in Docker (runs in [Baler](https://github.com/jacobnash/Baler)'s CI: every op, every format, openhay and phable as clients)

## Conformance

Every codec and the filter engine are tested against **haxall's own test suite**: 219 cases
(Zinc, Trio, filter parsing and evaluation) whose expected results were produced by running
haxall 4.0.6 itself. They ship in the Python package as `openhay.vectors`, so any other Haystack
implementation can run them too. openhay matches the reference on all of them.

The Zinc and Trio readers and the filter engine are openhay's own. libhaystack's versions
clamped large exponents to `i64::MAX`, dropped rows after nested grids, truncated unquoted Trio
values (`13 main st` became `13`) and compared missing tags and mixed kinds. libhaystack still
provides the value types, units and the Hayson/Zinc scalar writers we haven't replaced.

## Testing

CI (`.github/workflows/ci.yml`) runs on every PR and on `main`:

- **rust**: `cargo fmt --check`, `cargo clippy -D warnings` and `cargo test`. The tests include property tests (`crates/openhay-codec/tests/prop.rs`): random grids of every value kind must round-trip through Zinc, Hayson, JSON v3 and Trio. Run a longer soak with `PROPTEST_CASES=5000 cargo test -p openhay-codec --test prop`.
- **python**: builds the abi3 wheel with maturin and runs `pytest crates/openhay-py/tests` against `hay serve`, plus the phable interop tests.
- **fuzz**: [cargo-fuzz](https://github.com/rust-fuzz/cargo-fuzz) targets for the Zinc, Trio, JSON v3 and Hayson decoders, Zinc scalars and the filter parser. Each runs for 40 s, seeded from haxall's test cases. A decoded grid must never panic and must re-encode to something that decodes again. Locally:

```bash
cargo install cargo-fuzz && rustup toolchain install nightly
python3 fuzz/seed_corpus.py
cargo +nightly fuzz run zinc fuzz/corpus/zinc -- -max_total_time=600
```

## Known limitations

- Numbers whose unit isn't in the Haystack unit database lose the unit on decode. `libhaystack::Number`
  only holds database units.
- `MemBackend.nav` is a fixed site → equip → point tree.

## Related

- [`jacobnash/baler`](https://github.com/jacobnash/baler): the building data platform (formerly Timberdoodle); serves the Haystack API with openhay
- [`jacobnash/windrow`](https://github.com/jacobnash/windrow): BACnet/Modbus → MQTT (formerly FBF)
