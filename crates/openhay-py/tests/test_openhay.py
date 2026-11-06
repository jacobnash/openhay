import base64
import hashlib
import math
import os
import shutil
import socket
import subprocess
import time
from datetime import date, datetime, timezone
from pathlib import Path

import pytest

import openhay as oh

ZINC = 'ver:"3.0" site\nid,dis,temp,ts,na\n@a "AHU-1",,55.2°F,2024-01-02T13:45:00-05:00 New_York,NA\n@b,"x",NaN,,\n'


def test_decode_zinc_values():
    g = oh.decode_grid(ZINC)
    assert g.meta == {"site": oh.MARKER}
    a, b = g.rows
    assert a["id"] == oh.Ref("a") and a["id"].dis == "AHU-1"
    assert a["temp"] == oh.Number(55.2, "°F")
    assert a["ts"].tzinfo.key == "America/New_York" and a["ts"].hour == 13
    assert a["na"] is oh.NA
    assert math.isnan(b["temp"].val)


@pytest.mark.parametrize("fmt", ["zinc", "json3", "hayson", "trio"])
def test_roundtrip_every_format(fmt):
    g = oh.decode_grid(ZINC)
    back = oh.decode_grid(oh.encode_grid(g, fmt), fmt)
    assert back.rows[0] == g.rows[0]
    assert math.isnan(back.rows[1]["temp"].val), fmt


def test_build_grid_from_python():
    g = oh.Grid.from_rows([
        {"id": oh.Ref("p1"), "point": oh.MARKER, "curVal": 72, "on": True,
         "day": date(2024, 1, 2), "geo": oh.Coord(37.5, -77.4), "mod": datetime(2024, 1, 2, 18, 0, tzinfo=timezone.utc)},
    ])
    z = oh.encode_grid(g)
    assert z.startswith('ver:"3.0"')
    assert "@p1" in z and "C(37.5,-77.4)" in z
    j3 = oh.encode_grid(g, "json3")
    assert '"m:"' in j3 and '"r:p1"' in j3


def test_sniff_and_negotiate():
    assert oh.sniff(ZINC) == "zinc"
    assert oh.sniff('{"_kind":"grid","meta":{"ver":"3.0"},"cols":[],"rows":[]}') == "hayson"
    assert oh.negotiate("application/json", "json3") == "json3"
    assert oh.negotiate(None) == "zinc"


def test_zinc_scalars():
    assert oh.to_zinc(oh.Ref("abc")) == "@abc"
    assert oh.from_zinc("12kW") == oh.Number(12, "kW")
    assert oh.from_zinc('"hi"') == "hi"


def test_haystack_tz():
    assert oh.haystack_tz("New_York").key == "America/New_York"
    assert oh.haystack_tz("UTC") is timezone.utc
    with pytest.raises(ValueError):
        oh.haystack_tz("Nowhere_Land")


def test_scram_server_with_python_client_math():
    """Server half from openhay, credentials derived from a stored PBKDF2 hash
    (Timberdoodle's salt$dk format), client half computed independently in
    pure Python per RFC 5802 - so the two sides can't share a bug."""
    salt = os.urandom(16)
    iters = 4096
    dk = hashlib.pbkdf2_hmac("sha256", b"pencil", salt, iters)
    creds = oh.ScramCredentials.from_salted_password(dk, salt, iters)
    assert creds == creds and creds.iterations == iters

    nonce = "clientnonce123"
    client_first_bare = f"n=admin,r={nonce}"
    assert oh.ScramServer.username("n,," + client_first_bare) == "admin"
    state, server_first = oh.ScramServer.start("n,," + client_first_bare, creds)
    attrs = dict(kv.split("=", 1) for kv in server_first.split(","))
    assert attrs["r"].startswith(nonce) and int(attrs["i"]) == iters

    import hmac
    salted = hashlib.pbkdf2_hmac("sha256", b"pencil", base64.b64decode(attrs["s"]), int(attrs["i"]))
    client_key = hmac.new(salted, b"Client Key", "sha256").digest()
    stored_key = hashlib.sha256(client_key).digest()
    without_proof = f"c=biws,r={attrs['r']}"
    auth_msg = f"{client_first_bare},{server_first},{without_proof}".encode()
    sig = hmac.new(stored_key, auth_msg, "sha256").digest()
    proof = base64.b64encode(bytes(a ^ b for a, b in zip(client_key, sig))).decode()
    server_final = state.finish(f"{without_proof},p={proof}", creds)
    server_key = hmac.new(salted, b"Server Key", "sha256").digest()
    assert server_final == "v=" + base64.b64encode(hmac.new(server_key, auth_msg, "sha256").digest()).decode()

    bad = base64.b64encode(b"\0" * 32).decode()
    with pytest.raises(oh.AuthError):
        state.finish(f"{without_proof},p={bad}", creds)


def test_parse_auth_header():
    scheme, params = oh.parse_auth_header("SCRAM handshakeToken=abc, hash=SHA-256")
    assert scheme == "SCRAM" and params == {"handshakeToken": "abc", "hash": "SHA-256"}


def test_bundled_defs():
    libs = sorted(r["def"].val for r in oh.defs("libs"))
    assert libs == ["lib:ph", "lib:phIct", "lib:phIoT", "lib:phScience"]
    site = oh.defs("defs", "def==^site").rows
    assert len(site) == 1 and oh.Symbol("entity") in site[0]["is"]
    assert len(oh.defs("defs", limit=5)) == 5
    with pytest.raises(ValueError, match="filter"):
        oh.defs("defs", "def ==")


def test_filter_rows():
    rows = [{"id": oh.Ref("a"), "ahu": oh.MARKER, "dis": "A"}, {"id": oh.Ref("b"), "dis": "B", "n": oh.Number(3)}]
    assert oh.filter_rows("ahu", rows) == rows[:1]
    assert oh.filter_rows("n > 2", rows) == rows[1:]
    assert oh.filter_rows('dis == "B" or ahu', rows) == rows
    assert oh.filter_rows("x", []) == []


# ------------------------------------------------------------- live client


def _hay_bin() -> str | None:
    root = Path(__file__).resolve().parents[3]
    for p in (root / "target" / "debug" / "hay", root / "target" / "release" / "hay"):
        if p.exists():
            return str(p)
    return shutil.which("hay")


@pytest.fixture(scope="module")
def server(tmp_path_factory):
    hay = _hay_bin()
    if not hay:
        pytest.skip("hay binary not built (cargo build -p hay-cli)")
    trio = tmp_path_factory.mktemp("site") / "site.trio"
    trio.write_text(
        "id: @s1\nsite\ndis: \"HQ\"\ntz: \"New_York\"\n---\n"
        "id: @e1\nequip\nahu\nsiteRef: @s1\n---\n"
        "id: @p1\npoint\nhis\ntemp\nequipRef: @e1\ntz: \"New_York\"\n---\n"
        "id: @p2\npoint\nwritable\nequipRef: @e1\n"
    )
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        port = s.getsockname()[1]
    proc = subprocess.Popen([hay, "serve", str(trio), "--addr", f"127.0.0.1:{port}", "--auth", "admin:pencil"],
                            stderr=subprocess.PIPE)
    for _ in range(100):
        try:
            socket.create_connection(("127.0.0.1", port), timeout=0.1).close()
            break
        except OSError:
            time.sleep(0.05)
    yield f"http://127.0.0.1:{port}/api/demo"
    proc.terminate()
    proc.wait()


def test_client_requires_login(server):
    with pytest.raises(oh.AuthError):
        oh.Client(server).about()
    with pytest.raises(oh.AuthError):
        oh.Client(server, user="admin", password="wrong")


@pytest.mark.parametrize("fmt", ["zinc", "json3", "hayson"])
def test_client_ops(server, fmt):
    c = oh.Client(server, user="admin", password="pencil", format=fmt)
    assert c.token
    assert c.about()["productName"] == "openhay"
    assert [r["id"] for r in c.read("point and equipRef->ahu")] == [oh.Ref("p1"), oh.Ref("p2")]
    assert c.read_ids(["@p1", "nope"])[1] is None

    tz = oh.haystack_tz("New_York")
    c.his_write("p1", [(datetime(2024, 1, 2, 10, tzinfo=tz), 70.0), (datetime(2024, 1, 2, 11, tzinfo=tz), 71)])
    his = c.his_read("p1", "2024-01-02")
    assert his.col_names == ["ts", "val"] and [r["val"].val for r in his] == [70.0, 71.0]

    wid, recs = c.watch_sub("py", ["p2"])
    assert len(recs) == 1 and c.watch_poll(wid) == []
    c.point_write("p2", 8, 72.0, who="pytest")
    changed = c.watch_poll(wid)
    assert changed[0]["curVal"] == oh.Number(72.0)
    c.point_write("p2", 8, None)
    c.watch_close(wid)
    assert len(c.defs("libs")) == 4 and len(c.defs("defs", "def==^ahu")) == 1

    with pytest.raises(oh.HaystackError):
        c.read("point and (")
