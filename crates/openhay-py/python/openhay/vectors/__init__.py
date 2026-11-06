"""Conformance vectors: Haystack test cases with the reference
implementation's expected results, usable from any client or server's tests.

    from openhay import vectors
    v = vectors.load("haxall/testHaystack-4.0.6")
    for case in v["filter_eval"][0]["cases"]: ...

Expected values are Hayson; turn them into openhay values with
`openhay.from_hayson`. See vectors/haxall/NOTICE.md for provenance and license.
"""

from __future__ import annotations

import json
from functools import lru_cache
from pathlib import Path

_HERE = Path(__file__).resolve().parent


def available() -> list[str]:
    return sorted(str(p.relative_to(_HERE).with_suffix("")) for p in _HERE.rglob("*.json"))


@lru_cache(maxsize=None)
def load(name: str) -> dict:
    return json.loads((_HERE / f"{name}.json").read_text(encoding="utf-8"))
