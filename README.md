# openhay

Project Haystack 3 + 4 in Rust: wire formats, client, server and a CLI. No Fantom, no Axon, no license fee.
BSD-3-Clause, so anyone can embed it, including commercial vendors.

Values, filters, units, Hayson, Trio and Zinc scalars come from
[`libhaystack`](https://github.com/j2inn/libhaystack) (BSD-3, J2 Innovations). openhay adds what you
need to talk to real servers and to *be* one.

| crate | what it gives you |
|---|---|
| `openhay-codec` | One `Format` enum covering **Zinc, JSON v3** (SkySpark 3 / older nhaystack prefixed strings), **Hayson** (JSON v4), **Trio** and CSV. Also MIME negotiation and format sniffing. |
| `openhay-auth` | Haystack auth: `HELLO` → SCRAM-SHA-256 → `BEARER authToken`. Client and server halves, checked against the RFC 7677 test vector. |
| `openhay-client` | Async client for SkySpark, Haxall, nhaystack or anything else: login, `about`, `read` (filter or ids), `nav`, `hisRead`, `hisWrite`, `pointWrite` (+ priority array), `watchSub`/`Poll`/`Unsub`, `invokeAction`. |
| `openhay-server` | Implement one `Backend` trait over your storage and get the whole op surface. It serves both `/api/{op}` (Haxall layout) and `/api/{project}/{op}` (SkySpark layout), with SCRAM auth, content negotiation, Haystack error grids, and **server-managed watches with bounded memory**. `MemBackend` is a complete in-memory implementation. |
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

The four crates above work, with end-to-end tests that run the client against the server over real HTTP in
Zinc, JSON v3 and Hayson.

Roadmap:

- [ ] Python bindings (PyO3 / maturin, `pip install openhay`)
- [ ] TypeScript / WASM bindings
- [ ] `openhay-ws`: an open WebSocket transport with server-push watches
- [ ] Haystack 4 defs namespace exposed through `defs`/`libs` ops
- [ ] Xeto: parser and `xeto → anything` converter (Trio defs, JSON Schema, SHACL, TS/Python types)
- [ ] Conformance suite against Haxall (reference server *and* client) in Docker

## Known limitations

- Numbers whose unit isn't in the Haystack unit database lose the unit on decode. `libhaystack::Number`
  only holds database units.
- `MemBackend.nav` is a fixed site → equip → point tree.

## Related

- `jacobnash/timberdoodle`: a building data platform that will serve its records through `openhay-server`
- `jacobnash/fbf`: BACnet/Modbus → MQTT
