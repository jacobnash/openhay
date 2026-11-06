//
// Run every extracted input through the reference implementation (haxall's
// haystack pod) and record what it produces: Hayson for successful decodes,
// the error for failures, the matching record names for filters.
//
//   fan gen.fan inputs.json vectors.json
//

using util
using xeto
using haystack

class Gen
{
  static Str text(Str:Obj? c) { c["text"] }

  static Obj? hay(Obj? v)
  {
    buf := StrBuf()
    JsonWriter(buf.out).writeVal(v)
    return JsonInStream(buf.toStr.in).readJson
  }

  static Str:Obj? row(Str:Obj? c, Str key)
  {
    r := Str:Obj?[:] { ordered = true }
    r["src"] = c["src"]
    r[key] = c["text"]
    return r
  }

  static Obj?[] zincGrids(Obj?[] cases)
  {
    cases.map |Obj? o->Obj?|
    {
      c := (Str:Obj?)o
      r := row(c, "zinc")
      try r["expected"] = hay(ZincReader(text(c).in).readGrid)
      catch (Err e) r["error"] = e.toStr
      return r
    }
  }

  static Obj?[] zincTags(Obj?[] cases)
  {
    cases.map |Obj? o->Obj?|
    {
      c := (Str:Obj?)o
      r := row(c, "zinc")
      try r["expected"] = hay(ZincReader(text(c).in).readTags)
      catch (Err e) r["error"] = e.toStr
      return r
    }
  }

  static Obj?[] trio(Obj?[] cases)
  {
    cases.map |Obj? o->Obj?|
    {
      c := (Str:Obj?)o
      r := row(c, "trio")
      try r["expected"] = hay(TrioReader(text(c).in).readAllDicts)
      catch (Err e) r["error"] = e.toStr
      return r
    }
  }

  static Obj?[] filterParse(Obj?[] cases)
  {
    cases.map |Obj? o->Obj?|
    {
      c := (Str:Obj?)o
      r := row(c, "filter")
      f := Filter.fromStr(text(c), false)
      r["ok"] = f != null
      return r
    }
  }

  static Obj?[] filterEval(Obj?[] sets)
  {
    sets.map |Obj? o->Obj?|
    {
      s := (Str:Obj?)o
      List rawRecs := s["records"]
      recs := Dict[,]
      rawRecs.each |Obj? z| { Str zs := z; recs.add(ZincReader(zs.in).readTags) }
      |Ref->Dict?| lookup := |Ref id->Dict?| { return recs.find |Dict d->Bool| { return d["id"] == id } }
      pather := PatherContext(lookup)
      out := Str:Obj?[:] { ordered = true }
      out["src"] = s["src"]
      out["records"] = recs.map |Dict d->Obj?| { return hay(d) }
      List filters := s["filters"]
      out["cases"] = filters.map |Obj? fo->Obj?|
      {
        r := Str:Obj?[:] { ordered = true }
        r["filter"] = fo
        try
        {
          Str fs := fo
          f := Filter(fs)
          matched := recs.findAll |Dict d->Bool| { return f.matches(d, pather) }
          r["expected"] = matched.map |Dict d->Obj?| { return d["dis"] }
        }
        catch (Err e) r["error"] = e.toStr
        return r
      }
      return out
    }
  }

  static Void main(Str[] args)
  {
    inp := (Str:Obj?)JsonInStream(File.os(args[0]).in).readJson
    out := Str:Obj?[:] { ordered = true }
    out["zinc_grids"]        = zincGrids(inp["zinc_grids"])
    out["zinc_errs"]         = zincGrids(inp["zinc_errs"])
    out["zinc_tags"]         = zincTags(inp["zinc_tags"])
    out["trio"]              = trio(inp["trio"])
    out["filter_parse"]      = filterParse(inp["filter_parse"])
    out["filter_parse_errs"] = filterParse(inp["filter_parse_errs"])
    out["filter_eval"]       = filterEval(inp["filter_eval"])
    f := File.os(args[1]).out
    JsonOutStream(f).writeJson(out)
    f.close
    echo("wrote " + args[1])
  }
}
