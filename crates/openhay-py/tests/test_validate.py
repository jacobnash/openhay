"""openhay#8: validate a tag set against the Haystack 4 defs and protos."""

import openhay as oh


def test_a_proto_is_clean():
    assert oh.validate_tags(["discharge", "air", "temp", "sensor", "point"]) == []
    assert oh.validate_tags({"chilled": oh.MARKER, "water": oh.MARKER, "leaving": oh.MARKER,
                             "temp": oh.MARKER, "sensor": oh.MARKER, "point": oh.MARKER}) == []


def test_openhay_tag_shape_counts_true_as_marker():
    assert oh.validate_tags({"zone": True, "air": True, "temp": True, "sensor": True, "point": True}) == []


def test_problem_kinds():
    p = oh.validate_tags(["zone", "air", "temp", "sensor", "cmd", "point"])
    assert p[0] == {"level": "error", "kind": "choice_conflict", "tag": "pointFunction",
                    "options": ["cmd", "sensor"],
                    "message": "`pointFunction` allows one of its options, found cmd, sensor"}
    kinds = {x["kind"] for x in oh.validate_tags(["zone", "air", "tmep", "point"])}
    assert {"unknown_marker", "choice_missing"} <= kinds
    assert oh.validate_tags(["air", "temp", "sensor"])[0]["kind"] == "no_entity"
    warn = oh.validate_tags({"zone": oh.MARKER, "air": oh.MARKER, "temp": oh.MARKER, "sensor": oh.MARKER,
                             "point": oh.MARKER, "dis": "Zone temp", "windrowTagRule": "zone-air-temp"})
    assert [(w["level"], w["kind"], w["tag"]) for w in warn] == [("warning", "unknown_tag", "windrowTagRule")]


def test_not_a_proto_suggests_the_nearest():
    p = oh.validate_tags(["zone", "co2", "sensor", "point"])
    assert [x["kind"] for x in p] == ["no_proto"]
    assert any("concentration" in o for o in p[0]["options"])
    near = oh.nearest_protos(["zone", "co2", "sensor", "point"], n=3)
    assert len(near) == 3 and all(isinstance(x, list) for x in near)


def test_protos_matching_and_protos():
    m = oh.protos_matching(["discharge", "air", "temp", "sensor", "point", "his"])
    assert m[0] == ["air", "discharge", "point", "sensor", "temp"]
    allp = oh.protos()
    assert len(allp) > 900 and ["point", "run", "sensor"] in allp
