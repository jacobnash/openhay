"""Client.read follows a paging server's `after`; read_page surfaces it.

The fake server pages the way Baler does (jacobnash/Baler#244): a short
page's grid meta carries `incomplete`, `limit` and `after` (the last id it
covered), and a request that sends `after` back reads on. `paging=False`
stands in for Haxall/SkySpark, which never set `after`.
"""

import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import pytest

import openhay as oh

N = 2500
PAGE = 1000
RECS = [{"id": oh.Ref(f"p{i:05}"), "point": oh.MARKER, "n": oh.Number(i)} for i in range(N)]


class FakeServer:
    def __init__(self, paging: bool):
        self.paging = paging
        self.requests: list[dict] = []
        fake = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *a):
                pass

            def do_POST(self):
                body = self.rfile.read(int(self.headers["Content-Length"])).decode()
                req = oh.decode_grid(body)
                row = req.rows[0] if req.rows else {}
                fake.requests.append(row)
                out = fake.answer(row)
                data = oh.encode_grid(out, "zinc").encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/zinc; charset=utf-8")
                self.send_header("Content-Length", str(len(data)))
                self.end_headers()
                self.wfile.write(data)

        self.httpd = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        threading.Thread(target=self.httpd.serve_forever, daemon=True).start()
        self.base = f"http://127.0.0.1:{self.httpd.server_address[1]}/api"

    def answer(self, row: dict) -> oh.Grid:
        limit = int(row["limit"].val) if "limit" in row else None
        if not self.paging:
            return oh.Grid.from_rows(RECS[:limit], cols=["id", "point", "n"])
        n = limit or PAGE
        after = row.get("after")
        rest = [r for r in RECS if after is None or r["id"].id > after.id]
        page = rest[:n]
        meta = {}
        if len(rest) > n:
            meta = {"incomplete": f"limit of {n} reached; pass after to read on", "limit": oh.Number(n),
                    "after": page[-1]["id"]}
        return oh.Grid.from_rows(page, meta=meta, cols=["id", "point", "n"])


@pytest.fixture
def paging():
    s = FakeServer(paging=True)
    yield s
    s.httpd.shutdown()


@pytest.fixture
def plain():
    s = FakeServer(paging=False)
    yield s
    s.httpd.shutdown()


@pytest.mark.parametrize("fmt", ["zinc", "json3", "hayson"])
def test_read_returns_all_2500(paging, fmt):
    c = oh.Client(paging.base, format=fmt)
    recs = c.read("point")
    assert [r["id"] for r in recs] == [r["id"] for r in RECS]
    assert len(paging.requests) == 3
    assert "after" not in paging.requests[0]
    assert paging.requests[1]["after"] == oh.Ref("p00999")


def test_read_page_has_after_incomplete_and_limit(paging):
    c = oh.Client(paging.base)
    page = c.read_page("point")
    assert isinstance(page, oh.Grid) and len(page) == PAGE
    assert page.meta["after"] == oh.Ref("p00999")
    assert page.meta["limit"] == oh.Number(PAGE)
    assert "limit of 1000" in page.meta["incomplete"]

    # Each accepted form of the cursor reads on from the same place.
    for after in (page.meta["after"], "p00999", "@p00999"):
        nxt = c.read_page("point", after=after)
        assert nxt.rows[0]["id"] == oh.Ref("p01000")
    last = c.read_page("point", after=nxt.meta["after"])
    assert len(last) == 500 and "after" not in last.meta


def test_read_with_limit(paging):
    c = oh.Client(paging.base)
    assert len(c.read("point", limit=10)) == 10
    assert len(paging.requests) == 1


def test_server_that_doesnt_page_is_unchanged(plain):
    c = oh.Client(plain.base)
    assert len(c.read("point")) == N
    assert len(c.read("point", limit=5)) == 5
    page = c.read_page("point")
    assert len(page) == N and page.meta == {}
    assert len(plain.requests) == 3
    assert all("after" not in r for r in plain.requests)
