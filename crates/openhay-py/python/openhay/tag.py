"""openhay-tag: messy BAS point names to Haystack 4 tags.

    >>> import openhay as oh
    >>> oh.tag.tag_point(label="CHWSupTmp3")
    {'tags': {'chilled': True, 'water': True, 'leaving': True, 'temp': True, 'sensor': True},
     'rule': 'chilled-water-supply-temp', 'tokens': ['chw', 'sup', 'tmp'], 'signals': []}

It's deterministic and needs no model. The tokenizer splits camelCase and
digits (``CHWSupTmp3`` is ``chw sup tmp 3``). A BAS abbreviation lexicon
reads ``SAT``, ``ZN-T``, ``stpt`` and ``fdbk``. Units and the BACnet
object type count as evidence. The rule that explains most of the name
wins, and the result says which rule and which tokens, so a caller can
record provenance.

Per-site overrides are TOML in the shape of the bundled data
(``[words]``, ``[[rule]]``, ``[bacnet]``, ``[signal]``):

    >>> t = oh.tag.Tagger(overrides='[words]\\nlat = ["leaving", "air", "temp"]')
"""

from __future__ import annotations

from typing import Any

from . import _native

__all__ = ["Tagger", "tag_point", "haystack_unit", "tokenize", "rules"]


def _opt_str(v: Any) -> str | None:
    if v is None:
        return None
    s = str(v).strip()
    return s or None


class Tagger:
    """The bundled lexicon, units and rules, optionally with site overrides
    (a TOML string) layered on top."""

    def __init__(self, overrides: str | None = None):
        self._t = _native.NativeTagger(overrides)

    def tag_point(self, label: str = "", description: str = "", units: Any = None,
                  object_type: Any = None) -> dict:
        """Tags for one point.

        ``units`` is a BACnet engineering unit (``degreesFahrenheit`` or
        ``degrees-fahrenheit``) or a Haystack unit (``°F``). ``object_type``
        is a BACnet object type (``analog-input``, ``analogInput`` or
        ``AI``). Only the label is needed.

        Returns ``{"tags": {name: True}, "rule": id or None, "tokens": [...],
        "signals": [...]}``. ``tokens`` are the name tokens the rule
        matched, and ``signals`` are the other evidence it used
        (``units:°F``, ``object:analog-output``). When no rule matched,
        ``tags`` is empty and ``rule`` is None.
        """
        tags, rule, tokens, signals = self._t.tag_point(
            str(label or ""), str(description or ""), _opt_str(units), _opt_str(object_type))
        return {"tags": {t: True for t in tags}, "rule": rule, "tokens": tokens, "signals": signals}

    def haystack_unit(self, units: Any) -> str | None:
        """A BACnet engineering unit as its Haystack unit symbol
        (``degreesFahrenheit`` -> ``°F``), or None (``noUnits``, unknown)."""
        u = _opt_str(units)
        return self._t.haystack_unit(u) if u else None

    def rules(self) -> list[dict]:
        """Every rule, in match order: ``{"id", "match", "tags"}``."""
        return [{"id": i, "match": m, "tags": t} for i, m, t in self._t.rules()]


_default: Tagger | None = None


def _tagger() -> Tagger:
    global _default
    if _default is None:
        _default = Tagger()
    return _default


def tag_point(label: str = "", description: str = "", units: Any = None, object_type: Any = None) -> dict:
    """``Tagger().tag_point`` with the bundled data. See ``Tagger.tag_point``."""
    return _tagger().tag_point(label=label, description=description, units=units, object_type=object_type)


def haystack_unit(bacnet_units: Any) -> str | None:
    """A BACnet engineering unit as its Haystack unit symbol, or None."""
    return _tagger().haystack_unit(bacnet_units)


def tokenize(text: str) -> list[str]:
    """Split a point name: delimiters, camelCase and digits
    (``CHWSupTmp3`` -> ``["chw", "sup", "tmp", "3"]``)."""
    return _native.tokenize(str(text))


def rules() -> list[dict]:
    """The bundled rules: ``{"id", "match", "tags"}``."""
    return _tagger().rules()
