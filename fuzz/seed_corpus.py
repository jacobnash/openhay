"""Seed the fuzz corpora from haxall's own test cases (openhay.vectors).

    python fuzz/seed_corpus.py      # writes fuzz/corpus/<target>/

Stdlib only, so CI can run it before building anything. The corpus dir is
git-ignored: libFuzzer grows it as it runs.
"""

import hashlib
import json
from pathlib import Path

HERE = Path(__file__).resolve().parent
VECTORS = HERE.parent / "crates/openhay-py/python/openhay/vectors/haxall/testHaystack-4.0.6.json"

JSON3 = [
    '{"meta":{"ver":"3.0"},"cols":[{"name":"id"},{"name":"v"}],"rows":[{"id":"r:a Dis","v":"n:72.5 °F"}]}',
    '{"meta":{"ver":"3.0","site":"m:"},"cols":[{"name":"t"}],"rows":[{"t":"t:2024-01-02T13:45:00-05:00 New_York"}]}',
    '{"meta":{"ver":"3.0"},"cols":[{"name":"a"},{"name":"b"},{"name":"c"}],'
    '"rows":[{"a":"s:x:y","b":"c:37.5,-77.4","c":["d:2024-01-02","h:13:45:00","u:http://x","y:elec","x:Span:today","z:","-:","n:INF"]}]}',
    '{"meta":{"ver":"3.0"},"cols":[{"name":"g"}],"rows":[{"g":{"meta":{"ver":"3.0"},"cols":[{"name":"x"}],"rows":[{"x":"b:text/plain"}]}}]}',
]


def write(target: str, items) -> int:
    d = HERE / "corpus" / target
    d.mkdir(parents=True, exist_ok=True)
    n = 0
    for text in items:
        data = text.encode()
        (d / hashlib.sha1(data).hexdigest()).write_bytes(data)
        n += 1
    return n


def main() -> None:
    v = json.loads(VECTORS.read_text())
    zinc = [c["zinc"] for c in v["zinc_grids"] + v["zinc_errs"]]
    counts = {
        "zinc": write("zinc", zinc),
        "hayson": write("hayson", [json.dumps(c["expected"]) for c in v["zinc_grids"]]),
        "trio": write("trio", [c["trio"] for c in v["trio"]]),
        "json3": write("json3", JSON3),
        "zinc_scalar": write("zinc_scalar", [c["zinc"] for c in v["zinc_tags"]]
                             + ["@a", '"x"', "55.2°F", "2024-01-02T13:45:00-05:00 New_York", "C(1,2)", "[1,2]", "{a}"]),
        "filter": write("filter", [c["filter"] for c in v["filter_parse"] + v["filter_parse_errs"]]),
    }
    for k, n in counts.items():
        print(f"{k}: {n} seeds")


if __name__ == "__main__":
    main()
