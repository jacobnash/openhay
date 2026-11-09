"""openhay.tag: the binding Windrow#21 (jacobnash/Windrow PR #39) calls.

    openhay.tag.tag_point(label=, description=, units=, object_type=)
      -> {"tags": {name: True}, "rule": id or None, "tokens": [...], ...}
    openhay.tag.haystack_unit(bacnet_units) -> "°F" or None
"""

import pytest

import openhay
import openhay as oh


def test_reachable_as_an_attribute_after_import_openhay():
    # Windrow checks callable(getattr(getattr(openhay, "tag", None), "tag_point", None)).
    assert callable(getattr(getattr(openhay, "tag", None), "tag_point", None))
    assert callable(getattr(openhay.tag, "haystack_unit", None))


def test_windrow_call_shape():
    r = oh.tag.tag_point(label="CHWSupTmp3", description="", units="degreesFahrenheit", object_type="analog-input")
    assert r["tags"] == {"chilled": True, "water": True, "leaving": True, "temp": True, "sensor": True}
    assert r["rule"] == "chilled-water-supply-temp"
    assert r["tokens"] == ["chw", "sup", "tmp"]
    assert isinstance(r["signals"], list)


def test_no_match_is_empty_tags_and_no_rule():
    r = oh.tag.tag_point(label="mystery", description="", units=None, object_type=None)
    assert r == {"tags": {}, "rule": None, "tokens": [], "signals": []}


@pytest.mark.parametrize("label,rule", [
    ("SUP-AIR-TEM-12", "supply-air-temp"),
    ("SupplyAirTemp", "supply-air-temp"),
    ("AHU1-SAT", "supply-air-temp"),
    ("ZN-T-3", "zone-air-temp"),
    ("ZoneTempOccSp", "zone-air-temp-occ-sp"),
    ("DMPR-FDBK", "damper-feedback"),
    ("SUP-FAN-STA-4", "supply-fan-status"),
    ("STA-PRE-9", "static-pressure"),
])
def test_messy_names(label, rule):
    assert oh.tag.tag_point(label=label)["rule"] == rule


def test_units_and_object_type_as_evidence():
    r = oh.tag.tag_point(label="AI-17", units="degrees-fahrenheit")
    assert r["tags"] == {"temp": True, "sensor": True} and r["signals"] == ["units:°F"]
    assert oh.tag.tag_point(label="DMPR-3", object_type="analog-output")["tags"] == {"damper": True, "cmd": True}
    # Non-string inputs (bacpypes enums) are stringified, blanks ignored.
    class Units:
        def __str__(self):
            return "degreesFahrenheit"
    assert oh.tag.tag_point(label="AI-17", units=Units())["rule"] == "temp"
    assert oh.tag.tag_point(label="ZN-T", units="", object_type="")["rule"] == "zone-air-temp"


def test_haystack_unit():
    assert oh.tag.haystack_unit("degreesFahrenheit") == "°F"
    assert oh.tag.haystack_unit("percent-relative-humidity") == "%RH"
    assert oh.tag.haystack_unit("noUnits") is None
    assert oh.tag.haystack_unit(None) is None
    assert oh.tag.haystack_unit("") is None


def test_tokenize_and_rules():
    assert oh.tag.tokenize("CHWSupTmp3") == ["chw", "sup", "tmp", "3"]
    rules = oh.tag.rules()
    assert {"id", "match", "tags"} <= set(rules[0])
    assert any(r["id"] == "zone-air-temp" for r in rules)


def test_site_overrides():
    t = oh.tag.Tagger(overrides='[words]\nlat = ["leaving", "air", "temp"]\n'
                                '[[rule]]\nid = "lat"\nmatch = ["leaving", "air", "temp"]\n'
                                'tags = ["discharge", "air", "temp", "sensor"]\n')
    assert t.tag_point(label="AHU1-LAT")["rule"] == "lat"
    assert oh.tag.tag_point(label="AHU1-LAT")["rule"] is None  # the default tagger is unchanged
    with pytest.raises(ValueError):
        oh.tag.Tagger(overrides="not toml [")
