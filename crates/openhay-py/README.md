# openhay

Project Haystack 3 + 4 for Python, with a Rust core: no Fantom, no Axon, no license fee. BSD-3-Clause.

```bash
pip install openhay
```

- **Every wire format.** `decode_grid`/`encode_grid` handle Zinc, JSON v3 (SkySpark 3 / older nhaystack), Hayson (JSON v4) and Trio, and encode CSV. The input format is sniffed.
- **A blocking client** for SkySpark, Haxall, nhaystack or any other Haystack server. It logs in with SCRAM-SHA-256 or an OAuth/API-key bearer token. It covers `read` (and follows a paging server's `after`), `nav`, `hisRead`/`hisWrite`, `pointWrite`, watches and `invokeAction`.
- **The SCRAM server half**, for any Python web stack.
- **The Project Haystack 4.0.0 defs namespace**, bundled (`oh.defs("libs")`), and an in-memory filter (`oh.filter_rows`).
- **haxall's own test cases** as `openhay.vectors`. openhay passes all of them.

```python
import openhay as oh

g = oh.decode_grid(open("export.zinc").read())
print(oh.encode_grid(g, "json3"))

c = oh.Client("https://host/api/demo", user="admin", password="…")
for rec in c.read("point and his and equipRef->ahu"):
    his = c.his_read(rec["id"], "yesterday")
```

Wheels are abi3 (one per platform, any CPython 3.10+), so no Rust toolchain is needed to install.
Source, the Rust crates and the `hay` CLI live at <https://github.com/jacobnash/openhay>.
