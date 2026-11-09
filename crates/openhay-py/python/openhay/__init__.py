"""openhay: Project Haystack 3 + 4 for Python, backed by a Rust core.

    >>> import openhay as oh
    >>> g = oh.decode_grid('ver:"3.0"\\nid,dis,temp\\n@a,"AHU-1 DAT",55.2°F\\n')
    >>> g.rows[0]["temp"]
    Number(val=55.2, unit='°F')
    >>> print(oh.encode_grid(g, "json3"))           # Haystack 3 JSON for older servers

Formats: ``zinc``, ``json3`` (Haystack 3 JSON), ``hayson`` (Haystack 4 JSON),
``trio``, ``csv`` (encode only). Decoding without a format sniffs it.

Values map to Python as: Marker/Remove/NA singletons (``MARKER``, ``REMOVE``,
``NA``), ``Ref``, ``Number``, ``Symbol``, ``Uri``, ``XStr``, ``Coord``,
``datetime`` (timezone-aware), ``date``, ``time``, ``str``, ``bool``,
``None``, ``list``, ``dict`` and ``Grid``.
"""

from __future__ import annotations

import json
import math
from dataclasses import dataclass, field
from datetime import date, datetime, time, timezone
from functools import lru_cache
from typing import Any, Iterable
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from . import _native
from ._native import AuthError, HaystackError, ScramCredentials, ScramServer

__version__ = _native.__version__

__all__ = [
    "MARKER", "REMOVE", "NA", "Ref", "Number", "Symbol", "Uri", "XStr", "Coord", "Col", "Grid",
    "decode_grid", "encode_grid", "to_zinc", "from_zinc", "sniff", "negotiate", "mime",
    "from_hayson", "to_hayson", "haystack_tz", "Client", "HaystackError", "AuthError",
    "ScramCredentials", "ScramServer", "b64url", "b64url_decode", "parse_auth_header", "random_token",
    "validate_tags", "protos_matching", "nearest_protos", "protos",
]

sniff = _native.sniff
negotiate = _native.negotiate
mime = _native.mime
b64url = _native.b64url
b64url_decode = _native.b64url_decode
parse_auth_header = _native.parse_auth_header
random_token = _native.random_token


# --------------------------------------------------------------------- values


class _Singleton:
    __slots__ = ("_name",)

    def __init__(self, name: str):
        self._name = name

    def __repr__(self) -> str:
        return self._name

    def __reduce__(self):
        return self._name


MARKER = _Singleton("MARKER")
REMOVE = _Singleton("REMOVE")
NA = _Singleton("NA")


@dataclass(frozen=True)
class Ref:
    id: str
    dis: str | None = None

    def __str__(self) -> str:
        return "@" + self.id

    def __eq__(self, other) -> bool:  # dis is display-only
        return isinstance(other, Ref) and other.id == self.id

    def __hash__(self) -> int:
        return hash(self.id)


@dataclass(frozen=True)
class Number:
    val: float
    unit: str | None = None

    def __float__(self) -> float:
        return float(self.val)


@dataclass(frozen=True)
class Symbol:
    val: str


@dataclass(frozen=True)
class Uri:
    val: str


@dataclass(frozen=True)
class XStr:
    type: str
    val: str


@dataclass(frozen=True)
class Coord:
    lat: float
    lng: float


@dataclass
class Col:
    name: str
    meta: dict = field(default_factory=dict)


@dataclass
class Grid:
    meta: dict = field(default_factory=dict)
    cols: list[Col] = field(default_factory=list)
    rows: list[dict] = field(default_factory=list)

    @classmethod
    def from_rows(cls, rows: Iterable[dict], meta: dict | None = None, cols: Iterable[str] | None = None) -> "Grid":
        rows = [dict(r) for r in rows]
        if cols is None:
            seen: dict[str, None] = {}
            for r in rows:
                for k in r:
                    seen.setdefault(k)
            cols = list(seen)
        return cls(meta=dict(meta or {}), cols=[Col(c) for c in cols], rows=rows)

    @property
    def col_names(self) -> list[str]:
        return [c.name for c in self.cols]

    @property
    def is_error(self) -> bool:
        return self.meta.get("err") is MARKER

    def __len__(self) -> int:
        return len(self.rows)

    def __iter__(self):
        return iter(self.rows)

    def __getitem__(self, i):
        return self.rows[i]


# ------------------------------------------------------------------ timezones

_TZ_PREFIXES = ("America", "Europe", "Asia", "Africa", "Australia", "Pacific", "Atlantic", "Indian", "Antarctica", "Etc")


@lru_cache(maxsize=512)
def haystack_tz(name: str | None) -> ZoneInfo | timezone:
    """Haystack timezone name (``New_York``, ``UTC``, ``GMT+5``) -> tzinfo."""
    if not name or name in ("UTC", "Etc/UTC", "Z"):
        return timezone.utc
    for candidate in (name, *(f"{p}/{name}" for p in _TZ_PREFIXES)):
        try:
            return ZoneInfo(candidate)
        except (ZoneInfoNotFoundError, ValueError):
            continue
    raise ValueError(f"unknown Haystack timezone {name!r}")


def _tz_name(dt: datetime) -> str:
    tz = dt.tzinfo
    if tz is None or tz is timezone.utc:
        return "UTC"
    key = getattr(tz, "key", None)
    if key:
        return "UTC" if key in ("UTC", "Etc/UTC") else key.split("/")[-1]
    return "UTC"


# --------------------------------------------------------------------- hayson


def _num_from(v) -> float:
    if isinstance(v, str):
        return {"NaN": math.nan, "INF": math.inf, "-INF": -math.inf}[v]
    return float(v)


def _num_to(v: float):
    if math.isnan(v):
        return "NaN"
    if math.isinf(v):
        return "INF" if v > 0 else "-INF"
    return int(v) if float(v).is_integer() and abs(v) < 2**53 else v


def from_hayson(obj: Any) -> Any:
    """Hayson (Haystack 4 JSON) object -> Python value."""
    if isinstance(obj, bool) or obj is None or isinstance(obj, str):
        return obj
    if isinstance(obj, (int, float)):
        return Number(float(obj))
    if isinstance(obj, list):
        return [from_hayson(x) for x in obj]
    if not isinstance(obj, dict):
        raise TypeError(f"not Hayson: {obj!r}")
    kind = obj.get("_kind")
    if kind is None:
        return {k: from_hayson(v) for k, v in obj.items()}
    if kind == "marker":
        return MARKER
    if kind == "remove":
        return REMOVE
    if kind == "na":
        return NA
    if kind == "number":
        return Number(_num_from(obj["val"]), obj.get("unit"))
    if kind == "ref":
        return Ref(obj["val"], obj.get("dis"))
    if kind == "symbol":
        return Symbol(obj["val"])
    if kind == "uri":
        return Uri(obj["val"])
    if kind == "xstr":
        return XStr(obj["type"], obj["val"])
    if kind == "coord":
        return Coord(float(obj["lat"]), float(obj["lng"]))
    if kind == "date":
        return date.fromisoformat(obj["val"])
    if kind == "time":
        return time.fromisoformat(obj["val"])
    if kind == "dateTime":
        dt = datetime.fromisoformat(obj["val"].replace("Z", "+00:00"))
        return dt.astimezone(haystack_tz(obj.get("tz")))
    if kind == "grid":
        return _grid_from_hayson(obj)
    if kind == "dict":
        return {k: from_hayson(v) for k, v in obj.items() if k != "_kind"}
    raise ValueError(f"unknown Hayson kind {kind!r}")


def _grid_from_hayson(obj: dict) -> Grid:
    meta = {k: from_hayson(v) for k, v in (obj.get("meta") or {}).items() if k != "ver"}
    cols = [Col(c["name"], {k: from_hayson(v) for k, v in (c.get("meta") or {}).items()}) for c in obj.get("cols", [])]
    rows = [{k: from_hayson(v) for k, v in r.items() if v is not None} for r in obj.get("rows", [])]
    return Grid(meta, cols, rows)


def to_hayson(v: Any) -> Any:
    """Python value -> Hayson (Haystack 4 JSON) object."""
    if v is None or isinstance(v, (bool, str)):
        return v
    if v is MARKER:
        return {"_kind": "marker"}
    if v is REMOVE:
        return {"_kind": "remove"}
    if v is NA:
        return {"_kind": "na"}
    if isinstance(v, (int, float)):
        v = Number(float(v))
    if isinstance(v, Number):
        val = _num_to(float(v.val))
        if v.unit or isinstance(val, str):
            out = {"_kind": "number", "val": val}
            if v.unit:
                out["unit"] = v.unit
            return out
        return val
    if isinstance(v, Ref):
        out = {"_kind": "ref", "val": v.id}
        if v.dis is not None:
            out["dis"] = v.dis
        return out
    if isinstance(v, Symbol):
        return {"_kind": "symbol", "val": v.val}
    if isinstance(v, Uri):
        return {"_kind": "uri", "val": v.val}
    if isinstance(v, XStr):
        return {"_kind": "xstr", "type": v.type, "val": v.val}
    if isinstance(v, Coord):
        return {"_kind": "coord", "lat": v.lat, "lng": v.lng}
    if isinstance(v, datetime):
        if v.tzinfo is None:
            raise ValueError("Haystack DateTime must be timezone-aware")
        name = _tz_name(v)
        if name == "UTC":
            # tz is optional for UTC in Hayson, but always sent: some clients
            # (e.g. phable) read it unconditionally.
            return {"_kind": "dateTime", "val": v.astimezone(timezone.utc).isoformat().replace("+00:00", "Z"), "tz": "UTC"}
        return {"_kind": "dateTime", "val": v.isoformat(), "tz": name}
    if isinstance(v, date):
        return {"_kind": "date", "val": v.isoformat()}
    if isinstance(v, time):
        return {"_kind": "time", "val": v.isoformat()}
    if isinstance(v, Grid):
        meta = {"ver": "3.0", **{k: to_hayson(x) for k, x in v.meta.items()}}
        cols = []
        for c in v.cols:
            col = {"name": c.name}
            if c.meta:
                col["meta"] = {k: to_hayson(x) for k, x in c.meta.items()}
            cols.append(col)
        return {"_kind": "grid", "meta": meta, "cols": cols,
                "rows": [{k: to_hayson(x) for k, x in r.items() if x is not None} for r in v.rows]}
    if isinstance(v, dict):
        return {k: to_hayson(x) for k, x in v.items()}
    if isinstance(v, (list, tuple)):
        return [to_hayson(x) for x in v]
    raise TypeError(f"cannot encode {type(v).__name__} as a Haystack value")


# --------------------------------------------------------------------- codecs


def decode_grid(text: str, format: str | None = None) -> Grid:
    """Decode grid text. ``format=None`` sniffs Zinc / JSON v3 / Hayson / Trio."""
    hayson = _native.convert(text, "hayson", format)
    return _grid_from_hayson(json.loads(hayson))


def encode_grid(grid: Grid, format: str = "zinc") -> str:
    return _native.convert(json.dumps(to_hayson(grid)), format, "hayson")


def defs(op: str = "defs", filter: str | None = None, limit: int | None = None) -> Grid:
    """The bundled Project Haystack 4 defs namespace: ``op`` is ``defs``
    (every def), ``libs`` or ``filetypes``, optionally filtered
    (``lib==^lib:phIoT``) and limited. What a server's defs/libs/filetypes
    ops return."""
    return _grid_from_hayson(json.loads(_native.defs_op(op, filter, limit)))


def filter_rows(filter: str, rows: Iterable[dict]) -> list[dict]:
    """The rows (plain dicts of openhay values) that match a Haystack
    filter. Refs aren't followed; ``a->b`` walks nested dicts only."""
    rows = list(rows)
    if not rows:
        return []
    idx = _native.filter_indices(filter, json.dumps(to_hayson(Grid.from_rows(rows))))
    return [rows[i] for i in idx]


def _tags_hayson(tags: Any) -> str:
    """A tag dict, or an iterable of marker names, as a Hayson dict. A value
    of ``True`` counts as a marker, as in ``openhay.tag``'s ``{name: True}``."""
    if isinstance(tags, dict):
        d = {k: (MARKER if v is True else v) for k, v in tags.items()}
    else:
        d = {str(k): MARKER for k in tags}
    return json.dumps({k: to_hayson(v) for k, v in d.items()})


def validate_tags(tags: Any) -> list[dict]:
    """Check a tag set against the Haystack 4 defs. ``tags`` is a dict
    (``{"zone": MARKER, ...}``; ``True`` counts as a marker) or marker names.

    Returns problems, errors first, each as ``{"level": "error" | "warning",
    "kind", "tag", "options", "message"}``. The kinds are:

    - ``unknown_marker`` (error) or ``unknown_tag`` (warning, for value tags)
    - ``no_entity`` and ``multiple_entities`` (errors: not exactly one of
      point, equip, site, space, ...)
    - ``choice_conflict`` (error: e.g. both ``sensor`` and ``cmd``)
    - ``choice_missing`` (warning: e.g. no pointFunction)
    - ``no_proto`` (warning: legal, but not a shape the standard defines;
      ``options`` holds the nearest protos)

    An empty list means the markers are exactly a Haystack 4 proto.
    """
    return [{"level": lv, "kind": k, "tag": t, "options": o, "message": m}
            for lv, k, t, o, m in _native.validate_tags(_tags_hayson(tags))]


def protos_matching(tags: Any) -> list[list[str]]:
    """The Haystack 4 protos the tags fit (every proto tag present), most
    specific first, each as sorted marker names."""
    return _native.protos_matching(_tags_hayson(tags))


def nearest_protos(tags: Any, n: int = 5) -> list[list[str]]:
    """The ``n`` protos closest to the tags (fewest markers to add or
    remove), for suggesting a fix."""
    return _native.nearest_protos(_tags_hayson(tags), n)


def protos() -> list[list[str]]:
    """Every proto in the bundled Haystack 4 defs, as sorted marker names."""
    return _native.protos()


def to_zinc(value: Any) -> str:
    """One value in Zinc syntax, e.g. for a GET query parameter."""
    return _native.hayson_value_to_zinc(json.dumps(to_hayson(value)))


def from_zinc(text: str) -> Any:
    return from_hayson(json.loads(_native.zinc_value_to_hayson(text)))


# --------------------------------------------------------------------- client


class Client:
    """Blocking Haystack client for SkySpark, Haxall, nhaystack, openhay, ...

    ``base`` is the API root: ``http://host/api/demo`` (SkySpark layout) or
    ``http://host/api`` (Haxall). Log in with SCRAM via ``login()``, or pass an
    OAuth2/OIDC access token or API key as ``bearer=``.
    """

    def __init__(self, base: str, *, user: str | None = None, password: str | None = None,
                 bearer: str | None = None, format: str = "zinc"):
        self._c = _native.NativeClient(base, format)
        if bearer:
            self._c.set_token(bearer, True)
        if user is not None and password is not None:
            self.login(user, password)

    def login(self, user: str, password: str) -> None:
        self._c.login(user, password)

    @property
    def token(self) -> str | None:
        return self._c.token

    def call(self, op: str, req: Grid | None = None) -> Grid:
        req = req if req is not None else Grid()
        return _grid_from_hayson(json.loads(self._c.call(op, json.dumps(to_hayson(req)))))

    def about(self) -> dict:
        g = self.call("about")
        return g.rows[0] if g.rows else {}

    def read(self, filter: str, limit: int | None = None) -> list[dict]:
        row = {"filter": filter}
        if limit is not None:
            row["limit"] = Number(limit)
        return self.call("read", Grid.from_rows([row])).rows

    def defs(self, op: str = "defs", filter: str | None = None, limit: int | None = None) -> list[dict]:
        """A server's Haystack 4 namespace: ``op`` is defs, libs or filetypes."""
        row: dict = {}
        if filter is not None:
            row["filter"] = filter
        if limit is not None:
            row["limit"] = Number(limit)
        return self.call(op, Grid.from_rows([row]) if row else None).rows

    def read_ids(self, ids: Iterable[Ref | str]) -> list[dict | None]:
        refs = [i if isinstance(i, Ref) else Ref(str(i).lstrip("@")) for i in ids]
        g = self.call("read", Grid.from_rows([{"id": r} for r in refs], cols=["id"]))
        return [r if "id" in r else None for r in g.rows]

    def nav(self, nav_id: str | None = None) -> list[dict]:
        req = Grid.from_rows([{"navId": nav_id}]) if nav_id else Grid()
        return self.call("nav", req).rows

    def his_read(self, id: Ref | str, range: str = "today") -> Grid:
        ref = id if isinstance(id, Ref) else Ref(str(id).lstrip("@"))
        return self.call("hisRead", Grid.from_rows([{"id": ref, "range": range}]))

    def his_write(self, id: Ref | str, items: Iterable[tuple[datetime, Any]]) -> None:
        ref = id if isinstance(id, Ref) else Ref(str(id).lstrip("@"))
        rows = [{"ts": ts, "val": val} for ts, val in items]
        self.call("hisWrite", Grid.from_rows(rows, meta={"id": ref}, cols=["ts", "val"]))

    def point_write(self, id: Ref | str, level: int, val: Any, who: str = "openhay") -> None:
        ref = id if isinstance(id, Ref) else Ref(str(id).lstrip("@"))
        row = {"id": ref, "level": Number(level), "who": who, "val": val}
        self.call("pointWrite", Grid.from_rows([row], cols=["id", "level", "val", "who"]))

    def point_write_array(self, id: Ref | str) -> Grid:
        ref = id if isinstance(id, Ref) else Ref(str(id).lstrip("@"))
        return self.call("pointWrite", Grid.from_rows([{"id": ref}]))

    def watch_sub(self, dis: str, ids: Iterable[Ref | str], lease_secs: float | None = None) -> tuple[str, list[dict]]:
        refs = [i if isinstance(i, Ref) else Ref(str(i).lstrip("@")) for i in ids]
        meta = {"watchDis": dis}
        if lease_secs is not None:
            meta["lease"] = Number(lease_secs, "s")
        g = self.call("watchSub", Grid.from_rows([{"id": r} for r in refs], meta=meta, cols=["id"]))
        return str(g.meta["watchId"]), g.rows

    def watch_poll(self, watch_id: str, refresh: bool = False) -> list[dict]:
        meta = {"watchId": watch_id}
        if refresh:
            meta["refresh"] = MARKER
        return self.call("watchPoll", Grid(meta=meta, cols=[Col("empty")])).rows

    def watch_close(self, watch_id: str) -> None:
        self.call("watchUnsub", Grid(meta={"watchId": watch_id, "close": MARKER}, cols=[Col("empty")]))

    def invoke_action(self, id: Ref | str, action: str, **args) -> Grid:
        ref = id if isinstance(id, Ref) else Ref(str(id).lstrip("@"))
        meta = {"id": ref, "action": action}
        g = Grid.from_rows([args], meta=meta) if args else Grid(meta=meta, cols=[Col("empty")])
        return self.call("invokeAction", g)
