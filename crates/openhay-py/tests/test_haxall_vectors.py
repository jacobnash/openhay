"""openhay against haxall's own test cases (openhay.vectors).

Every case's expected value came from running the input through the
reference runtime, so a failure here is a real difference from Haxall.
Differences we know about and haven't fixed yet are listed in KNOWN_GAPS
with the reason. They must still fail (strict xfail), so fixing one
without removing it from the list breaks the build, and the list never goes
stale.
"""

from __future__ import annotations

import json
import math

import pytest

import openhay as oh
from openhay import vectors

V = vectors.load("haxall/testHaystack-4.0.6")

# case id -> why openhay differs from the reference today
KNOWN_GAPS: dict[str, str] = {}


def _canon(v):
    """Hayson-shaped JSON, for comparing values regardless of how each side
    spelled them (number formatting, ordering)."""
    j = oh.to_hayson(v)
    return json.loads(json.dumps(j, sort_keys=True, default=str))


def _nan_safe(a):
    if isinstance(a, float) and math.isnan(a):
        return "NaN"
    if isinstance(a, dict):
        return {k: _nan_safe(x) for k, x in a.items()}
    if isinstance(a, list):
        return [_nan_safe(x) for x in a]
    return a


def _grid_view(g: oh.Grid):
    meta = {k: v for k, v in g.meta.items() if k != "ver"}
    return {
        "meta": _nan_safe(_canon(meta)),
        "cols": [(c.name, _nan_safe(_canon(dict(c.meta)))) for c in g.cols],
        "rows": [_nan_safe(_canon(r)) for r in g.rows],
    }


def _cases(section):
    return [pytest.param(c, id=f"{section}:{c['src']}") for c in V[section]]


def _maybe_known(request):
    why = KNOWN_GAPS.get(request.node.callspec.id)
    if why:
        request.applymarker(pytest.mark.xfail(reason=why, strict=True))


@pytest.mark.parametrize("case", _cases("zinc_grids"))
def test_zinc_grid(case, request):
    _maybe_known(request)
    got = oh.decode_grid(case["zinc"], "zinc")
    want = oh.from_hayson(case["expected"])
    assert _grid_view(got) == _grid_view(want)
    # and it survives our own Zinc round trip
    again = oh.decode_grid(oh.encode_grid(got, "zinc"), "zinc")
    assert _grid_view(again) == _grid_view(want)


@pytest.mark.parametrize("case", _cases("zinc_errs"))
def test_zinc_error(case, request):
    _maybe_known(request)
    assert "error" in case
    with pytest.raises(Exception):
        oh.decode_grid(case["zinc"], "zinc")


@pytest.mark.parametrize("case", _cases("zinc_tags"))
def test_zinc_tags(case, request):
    _maybe_known(request)
    got = oh.from_zinc("{" + case["zinc"] + "}")
    assert _nan_safe(_canon(got)) == _nan_safe(_canon(oh.from_hayson(case["expected"])))


@pytest.mark.parametrize("case", _cases("trio"))
def test_trio(case, request):
    _maybe_known(request)
    want = [_nan_safe(_canon(oh.from_hayson(d))) for d in case["expected"]]
    if not case["trio"].strip():
        assert want == []
        return
    got = oh.decode_grid(case["trio"], "trio")
    assert [_nan_safe(_canon(r)) for r in got.rows] == want


PROBE = [{"id": oh.Ref("probe"), "x": oh.MARKER}]


@pytest.mark.parametrize("case", _cases("filter_parse"))
def test_filter_parses(case, request):
    _maybe_known(request)
    oh.filter_rows(case["filter"], PROBE)  # must not raise


@pytest.mark.parametrize("case", _cases("filter_parse_errs"))
def test_filter_parse_error(case, request):
    _maybe_known(request)
    with pytest.raises(ValueError):
        oh.filter_rows(case["filter"], PROBE)


def _eval_cases():
    for s in V["filter_eval"]:
        recs = [oh.from_hayson(r) for r in s["records"]]
        for c in s["cases"]:
            yield pytest.param(recs, c, id=f"filter_eval:{s['src']}:{c['filter']}")


@pytest.mark.parametrize("recs, case", list(_eval_cases()))
def test_filter_eval(recs, case, request):
    _maybe_known(request)
    got = [r["dis"] for r in oh.filter_rows(case["filter"], recs)]
    assert got == case["expected"]


def test_known_gaps_name_real_cases():
    ids = set()
    for section in ("zinc_grids", "zinc_errs", "zinc_tags", "trio", "filter_parse", "filter_parse_errs"):
        ids |= {f"{section}:{c['src']}" for c in V[section]}
    for s in V["filter_eval"]:
        ids |= {f"filter_eval:{s['src']}:{c['filter']}" for c in s["cases"]}
    assert set(KNOWN_GAPS) <= ids, set(KNOWN_GAPS) - ids
