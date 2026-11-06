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
KNOWN_GAPS: dict[str, str] = {
    'zinc_grids:verifyGrid@41': 'Zinc writer: a lone null cell is written as a blank line, which decodes as no row',
    'zinc_grids:verifyGrid@63': 'Zinc 2.0 Bin(...) syntax (unquoted mime) not supported',
    'zinc_grids:verifyGrid@76': 'large exponent numbers (2.4e20) clamp to i64::MAX',
    'zinc_grids:verifyGrid@102': 'names starting with _ rejected',
    'zinc_grids:verifyGrid@132': 'XStr C("") parsed as a Coord',
    'zinc_grids:verifyGrid@146': 'names starting with _ rejected',
    'zinc_grids:verifyGrid@166': 'GMT+/-N datetimes change local time when re-encoded',
    'zinc_grids:verifyGrid@176': 'a row is dropped; legacy tz alias PST8PDT not normalized to Los_Angeles',
    'zinc_grids:verifyGrid@212': 'the row after a nested grid is dropped',
    'zinc_grids:verifyGrid@390': 'Zinc 2.0 Bin(...) syntax (unquoted mime) not supported',
    'trio:verifyTrio@58': 'multiline string loses its trailing newline',
    'trio:verifyTrio@70': 'unquoted string starting with a number decodes as just the number',
    'trio:verifyTrio@95': 'unquoted string starting with a number decodes as just the number',
    'trio:verifyTrio@110': 'names starting with _ rejected',
    'trio:verifyTrio@165': 'true/false not decoded as Bool',
    'trio:verifyTrio@190': 'unquoted string starting with a date decodes as just the date',
    'trio:verifyTrio@207': 'inline Zinc list/dict values fail to decode',
    'trio:verifyTrio@240': 'nested Zinc grid values fail to decode',
    'filter_parse:verifyParse@78': 'filter parser: unsupported syntax (keyword tag names, hex numbers, h:mm times, Xeto spec names, // comments)',
    'filter_parse:verifyParse@96': 'filter parser: unsupported syntax (keyword tag names, hex numbers, h:mm times, Xeto spec names, // comments)',
    'filter_parse:verifyParse@106': 'filter parser: unsupported syntax (keyword tag names, hex numbers, h:mm times, Xeto spec names, // comments)',
    'filter_parse:verifyParse@107': 'filter parser: unsupported syntax (keyword tag names, hex numbers, h:mm times, Xeto spec names, // comments)',
    'filter_parse:verifyParse@125': 'filter parser: unsupported syntax (keyword tag names, hex numbers, h:mm times, Xeto spec names, // comments)',
    'filter_parse:verifyParse@156': 'filter parser: unsupported syntax (keyword tag names, hex numbers, h:mm times, Xeto spec names, // comments)',
    'filter_parse:verifyParse@157': 'filter parser: unsupported syntax (keyword tag names, hex numbers, h:mm times, Xeto spec names, // comments)',
    'filter_parse:verifyParse@158': 'filter parser: unsupported syntax (keyword tag names, hex numbers, h:mm times, Xeto spec names, // comments)',
    'filter_parse:verifyParse@159': 'filter parser: unsupported syntax (keyword tag names, hex numbers, h:mm times, Xeto spec names, // comments)',
    'filter_parse:verifyParse@160': 'filter parser: unsupported syntax (keyword tag names, hex numbers, h:mm times, Xeto spec names, // comments)',
    'filter_parse:verifyParse@161': 'filter parser: unsupported syntax (keyword tag names, hex numbers, h:mm times, Xeto spec names, // comments)',
    'filter_parse:verifyParse@175': 'filter parser: unsupported syntax (keyword tag names, hex numbers, h:mm times, Xeto spec names, // comments)',
    'filter_eval:FilterTest.testInclude:foo != 13': 'filter: comparisons must be false on a missing tag or a different kind (libhaystack compares them)',
    'filter_eval:FilterTest.testInclude:foo < 13': 'filter: comparisons must be false on a missing tag or a different kind (libhaystack compares them)',
    'filter_eval:FilterTest.testInclude:foo < "c"': 'filter: comparisons must be false on a missing tag or a different kind (libhaystack compares them)',
    'filter_eval:FilterTest.testInclude:foo <= 13': 'filter: comparisons must be false on a missing tag or a different kind (libhaystack compares them)',
    'filter_eval:FilterTest.testInclude:foo <= "baz"': 'filter: comparisons must be false on a missing tag or a different kind (libhaystack compares them)',
    'filter_eval:FilterTest.testInclude:foo > 12': 'filter: comparisons must be false on a missing tag or a different kind (libhaystack compares them)',
    'filter_eval:FilterTest.testInclude:foo >= 12': 'filter: comparisons must be false on a missing tag or a different kind (libhaystack compares them)',
    'filter_eval:FilterTest.testInclude:ref->num <= 20': 'filter: comparisons must be false on a missing tag or a different kind (libhaystack compares them)',
    'filter_eval:FilterTest.testInclude:^foo': 'filter: ^symbol (def conjunct) terms not supported',
    'filter_eval:FilterTest.testInclude:^foo-thru': 'filter: ^symbol (def conjunct) terms not supported',
    'filter_eval:FilterTest.testInclude:None': 'filter: Xeto spec names not supported',
    'filter_eval:FilterTest.testInclude:Dict': 'filter: Xeto spec names not supported',
    'filter_eval:FilterTest.testIncludeNumbers:dis >  75': 'filter: comparisons must be false on a missing tag or a different kind (libhaystack compares them)',
    'filter_eval:FilterTest.testIncludeList:str=="q"': 'filter: Ref lists in paths and List vs scalar equality',
    'filter_eval:FilterTest.testIncludeList:ref->dis=="a"': 'filter: Ref lists in paths and List vs scalar equality',
    'filter_eval:FilterTest.testIncludeList:refx->dis=="c"': 'filter: Ref lists in paths and List vs scalar equality',
    'filter_eval:FilterTest.testIncludeList:refx->ref': 'filter: Ref lists in paths and List vs scalar equality',
    'filter_eval:FilterTest.testIncludeList:refx->ref==@a': 'filter: Ref lists in paths and List vs scalar equality',
    'filter_eval:FilterTest.testIncludeList:refx->air': 'filter: Ref lists in paths and List vs scalar equality',
    'filter_eval:FilterTest.testIncludeList:refx->water': 'filter: Ref lists in paths and List vs scalar equality',
    'filter_eval:FilterTest.testIncludeList:not refx->air': 'filter: Ref lists in paths and List vs scalar equality',
    'filter_eval:FilterTest.testIncludeDateTime:ts->date==2020-09-02': 'filter: DateTime virtual paths (ts->date/time/tz) not supported',
    'filter_eval:FilterTest.testIncludeDateTime:ts->date <= 2020-09-02': 'filter: DateTime virtual paths (ts->date/time/tz) not supported',
    'filter_eval:FilterTest.testIncludeDateTime:ts->date >= 2020-09-02': 'filter: DateTime virtual paths (ts->date/time/tz) not supported',
    'filter_eval:FilterTest.testIncludeDateTime:ts->time == 02:00': 'filter: DateTime virtual paths (ts->date/time/tz) not supported',
    'filter_eval:FilterTest.testIncludeDateTime:ts->time <= 03:00': 'filter: DateTime virtual paths (ts->date/time/tz) not supported',
    'filter_eval:FilterTest.testIncludeDateTime:ts->time > 03:00': 'filter: DateTime virtual paths (ts->date/time/tz) not supported',
    'filter_eval:FilterTest.testIncludeDateTime:ts->tz == "New_York"': 'filter: DateTime virtual paths (ts->date/time/tz) not supported',
}


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
