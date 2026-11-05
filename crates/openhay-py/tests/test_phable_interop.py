"""Interop with phable, an independent Haystack client (not ours), against
`hay serve`: SCRAM login, read, read_by_id, hisWrite/hisRead, in both Hayson
and Zinc. Catches wire-format assumptions our own client wouldn't (it found
four: base64 alphabet in, base64 alphabet out, token charset, UTC tz)."""

import datetime as dt

import pytest

phable = pytest.importorskip("phable")

from test_openhay import server  # noqa: E402,F401  (reuses the hay-serve fixture)


@pytest.mark.parametrize("content_type", ["json", "zinc"])
def test_phable_round_trip(server, content_type):  # noqa: F811
    with phable.open_haystack_client(server, "admin", "pencil", content_type=content_type) as c:
        assert c.about()["productName"] == "openhay"
        rows = c.read_all("point and equipRef->ahu").rows
        assert {str(r["id"].val) for r in rows} == {"p1", "p2"}
        p1 = phable.Ref("p1")
        assert c.read_by_id(p1)["id"].val == "p1"
        ts = dt.datetime(2024, 3, 1, 12, tzinfo=dt.timezone.utc)
        c.his_write_by_id(p1, [{"ts": ts, "val": phable.Number(21.5)}])
        his = c.his_read_by_id(p1, dt.date(2024, 3, 1))
        assert [r["val"].val for r in his.rows] == [21.5]
