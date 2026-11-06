"""Extract test *inputs* from haxall's testHaystack suite into inputs.json.

Expected outputs are not transcribed here: gen.fan runs every input through
the reference implementation (haxall's haystack pod) and records what it says.
"""

import json
import re
import sys
from pathlib import Path

SRC = Path(sys.argv[1])  # .../haxall/src/test/testHaystack/fan
OUT = Path(sys.argv[2])


def dsl_strings(text: str):
    """Yield (line_no, value) for every Str<|...|> literal, dedented the way
    Fantom does: continuation lines lose leading whitespace up to the column
    where the content started."""
    for m in re.finditer(r"Str<\|(.*?)\|>", text, re.S):
        start = m.start(1)
        line_start = text.rfind("\n", 0, start) + 1
        col = start - line_start
        lines = m.group(1).split("\n")
        out = [lines[0]]
        for ln in lines[1:]:
            k = 0
            while k < col and k < len(ln) and ln[k] in " \t":
                k += 1
            out.append(ln[k:])
        yield text.count("\n", 0, m.start()) + 1, "\n".join(out)


def fan_str(lit: str) -> str:
    """Decode a Fantom "..." literal (no interpolation used in these tests)."""
    body = lit[1:-1]
    return json.loads('"' + body.replace("\\$", "$") + '"')


def calls_with_dsl(text: str, fn: str):
    """Str<|..|> literals inside a call to fn(...): the nearest `verifyXxx(`
    before the literal must be fn."""
    res = []
    for m in re.finditer(r"Str<\|(.*?)\|>", text, re.S):
        call = re.findall(r"(verify\w+)\(", text[: m.start()])
        if call and call[-1] == fn:
            line_no = text.count("\n", 0, m.start()) + 1
            res.append({"src": f"{fn}@{line_no}", "text": next(v for ln, v in dsl_strings(text) if ln == line_no)})
    return res


def read(name: str) -> str:
    """Source with /* ... */ blocks blanked out (same length, newlines kept),
    so commented-out tests (e.g. ZincTest's 'NO LONGER SUPPORTED' testVer1)
    aren't extracted and line numbers stay right."""
    text = (SRC / name).read_text()
    return re.sub(r"/\*.*?\*/", lambda m: re.sub(r"[^\n]", " ", m.group(0)), text, flags=re.S)


zinc = read("ZincTest.fan")
trio = read("TrioTest.fan")
filt = read("FilterTest.fan")

inputs = {
    "zinc_grids": calls_with_dsl(zinc, "verifyGrid"),
    "zinc_errs": calls_with_dsl(zinc, "verifyZincErr"),
    "zinc_tags": [
        {"src": f"verifyTags@{zinc.count(chr(10), 0, m.start()) + 1}", "text": fan_str(m.group(1))}
        for m in re.finditer(r'verifyTags\(("(?:[^"\\]|\\.)*")', zinc)
    ],
    "trio": calls_with_dsl(trio, "verifyTrio"),
}

# Filter parse cases: first arg of verifyParse when it's a plain literal (the
# few interpolated ones are added by hand with concrete values).
parse = []
for m in re.finditer(r'verifyParse\(("(?:[^"\\]|\\.)*")', filt):
    s = m.group(1)
    if "$" in s.replace("\\$", ""):
        continue
    parse.append({"src": f"verifyParse@{filt.count(chr(10), 0, m.start()) + 1}", "text": fan_str(s)})
parse += [
    {"src": "verifyParse@128 (dt literal)", "text": "foo < 2016-02-22T08:43:12.905-05:00 New_York"},
    {"src": "verifyParse@133 (Ref.gen)", "text": "author == @1f2e3d4c-5b6a7980"},
    {"src": "verifyParse@135", "text": "author == @foo:bar-baz"},
    {"src": "verifyParse@137", "text": "id==@1ec5d86e-8ea2ba5a"},
]
inputs["filter_parse"] = parse
inputs["filter_parse_errs"] = [
    {"src": f"verifyParseErr@{filt.count(chr(10), 0, m.start()) + 1}", "text": fan_str(m.group(1))}
    for m in re.finditer(r'verifyParseErr\(("(?:[^"\\]|\\.)*")\)', filt)
]

# Filter evaluation: record sets from testInclude* written as Zinc tags, the
# filters extracted from verifyInclude(recs, Str<|...|>, ...). Records carry
# an id equal to their dis so refs resolve the way the test's pather does
# (it looks records up by dis).
SETS = {
    "testInclude": [
        'id:@a dis:"a" num:10 date:2016-01-01 foo:"baz"',
        'id:@b dis:"b" num:20 date:2016-01-02 foo:12 ref:@a',
        'id:@c dis:"c" num:30 date:2016-01-03 foo:13 ref:@b thru:"c"',
        'id:@d dis:"d" num:30 date:2016-01-03 ref:@c',
        'id:@e dis:"e" num:40 date:2016-01-06 ref:{thru:"e"}',
    ],
    "testIncludeNumbers": [
        'id:@a dis:"a" num:75',
        'id:@b dis:"b" num:75°F',
        'id:@c dis:"c" num:2min',
    ],
    "testIncludeList": [
        'id:@a dis:"a" str:"q" ref:@a',
        'id:@b dis:"b" str:["q"] ref:[@a]',
        'id:@c dis:"c" str:["q","r"] ref:[@a,@b] air',
        'id:@d dis:"d" str:"q" ref:[@b,@c] water',
        'id:@e dis:"e" refx:@c',
        'id:@f dis:"f" refx:[@c]',
        'id:@g dis:"g" refx:[@d]',
        'id:@h dis:"h" refx:[@c,@d]',
    ],
    "testIncludeDateTime": [
        'id:@a dis:"a" ts:2020-09-01T01:00:00-04:00 New_York',
        'id:@b dis:"b" ts:2020-09-01T02:00:00-04:00 New_York',
        'id:@c dis:"c" ts:2020-09-02T03:00:00-04:00 New_York',
        'id:@d dis:"d" ts:2020-09-02T04:00:00-04:00 New_York',
        'id:@e dis:"e" ts:2020-09-03T02:00:00-04:00 New_York',
    ],
}
evals = []
for name, recs in SETS.items():
    body = filt[filt.index(f"Void {name}()"):]
    body = body[: body.index("\n  }\n")]
    filters = [s for _, s in dsl_strings(body)]
    evals.append({"src": f"FilterTest.{name}", "records": recs, "filters": filters})
inputs["filter_eval"] = evals

OUT.write_text(json.dumps(inputs, indent=1, ensure_ascii=False))
print({k: (len(v) if k != "filter_eval" else sum(len(e["filters"]) for e in v)) for k, v in inputs.items()})
