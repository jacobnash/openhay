//! Haystack filters: parse once, evaluate against any record source.
//!
//! openhay's own engine, written from the spec grammar (`docHaystack/Filters`)
//! and checked against haxall's reference cases (`openhay.vectors`). The
//! semantics that matter:
//!
//! * A comparison is only true between values of the **same kind**, and never
//!   on a missing tag (`foo < 13` doesn't match a record without `foo`).
//! * Numbers compare only with the **same unit** (`75°F` is not `75`), and
//!   there's no unit conversion.
//! * Paths follow Refs through a [`Resolver`], fan out over **lists of Refs**
//!   (`refx->dis` checks every Ref), walk into nested Dicts, and read
//!   `date`/`time`/`tz` off a DateTime.
//! * `^a-b` matches records that are all of the conjunct parts, and Xeto spec
//!   names (`Ahu`, `ph::Ahu`) are delegated to the resolver.
//!
//! ```
//! use openhay_filter::{Filter, NoRefs};
//! use libhaystack::val::{Dict, Value};
//! let f = Filter::parse("point and temp > 70").unwrap();
//! let mut d = Dict::new();
//! d.insert("point".into(), Value::make_marker());
//! d.insert("temp".into(), Value::make_number(72.0));
//! assert!(f.matches(&d, &NoRefs));
//! ```

use libhaystack::val::{Dict, Ref, Value};
use std::cmp::Ordering;
use std::fmt;

#[derive(Debug, thiserror::Error, PartialEq)]
#[error("filter: {msg} (at {pos})")]
pub struct ParseError {
    pub msg: String,
    pub pos: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A parsed filter.
#[derive(Debug, Clone, PartialEq)]
pub enum Filter {
    Has(Vec<String>),
    Missing(Vec<String>),
    Cmp(Vec<String>, Op, Value),
    And(Box<Filter>, Box<Filter>),
    Or(Box<Filter>, Box<Filter>),
    /// `^equip`, `^hot-water`: def (conjunct) membership.
    IsSymbol(String),
    /// `Ahu`, `ph::Ahu`, `ph.points::ZoneAirTempSensor`: Xeto spec membership.
    IsSpec(String),
}

/// What a filter needs from the record store beyond the record itself.
pub trait Resolver {
    /// The record a Ref points at (for paths like `equipRef->dis`).
    fn deref(&self, r: &Ref) -> Option<Dict>;

    /// `^symbol` membership. Default: the record has every conjunct part as a
    /// tag (`^hot-water` = `hot` and `water`), the reference behavior without
    /// a defs namespace.
    fn is_symbol(&self, rec: &Dict, symbol: &str) -> bool {
        symbol
            .split('-')
            .all(|part| rec.get(part).is_some_and(|v| !v.is_null()))
    }

    /// Xeto spec membership. Default: nothing matches (no Xeto context).
    fn is_spec(&self, _rec: &Dict, _spec: &str) -> bool {
        false
    }
}

/// A resolver with no other records: paths through Refs find nothing.
pub struct NoRefs;

impl Resolver for NoRefs {
    fn deref(&self, _r: &Ref) -> Option<Dict> {
        None
    }
}

impl Filter {
    pub fn parse(text: &str) -> Result<Filter, ParseError> {
        let toks = lex(text)?;
        let mut p = Parser { toks, i: 0 };
        let f = p.cond_or()?;
        if p.cur() != &Tok::Eof {
            return Err(p.err(&format!("unexpected {}", p.cur())));
        }
        Ok(f)
    }

    pub fn matches(&self, rec: &Dict, cx: &dyn Resolver) -> bool {
        match self {
            Filter::And(a, b) => a.matches(rec, cx) && b.matches(rec, cx),
            Filter::Or(a, b) => a.matches(rec, cx) || b.matches(rec, cx),
            Filter::IsSymbol(s) => cx.is_symbol(rec, s),
            Filter::IsSpec(s) => cx.is_spec(rec, s),
            Filter::Has(p) | Filter::Missing(p) | Filter::Cmp(p, _, _) => {
                matches_at(self, p, rec.get(&p[0]).cloned(), 0, cx)
            }
        }
    }

    /// The first tag of every path the filter tests, in order (for indexing).
    pub fn tags(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.each_tag(&mut out);
        out
    }

    fn each_tag(&self, out: &mut Vec<String>) {
        match self {
            Filter::Has(p) | Filter::Missing(p) | Filter::Cmp(p, _, _) => out.push(p[0].clone()),
            Filter::And(a, b) | Filter::Or(a, b) => {
                a.each_tag(out);
                b.each_tag(out);
            }
            Filter::IsSymbol(_) | Filter::IsSpec(_) => {}
        }
    }
}

fn matches_at(
    f: &Filter,
    path: &[String],
    val: Option<Value>,
    level: usize,
    cx: &dyn Resolver,
) -> bool {
    let val = val.filter(|v| !v.is_null());
    // A list of Refs: each Ref is tried in turn at this level.
    if let Some(Value::List(items)) = &val {
        for item in items.iter() {
            if matches!(item, Value::Ref(_)) && matches_at(f, path, Some(item.clone()), level, cx) {
                return true;
            }
        }
    }
    let mut val = val;
    if level + 1 < path.len() {
        let next = &path[level + 1];
        if let Some(Value::Ref(r)) = &val {
            val = cx.deref(r).map(Value::make_dict);
        }
        match &val {
            Some(Value::Dict(d)) => {
                return matches_at(f, path, d.get(next).cloned(), level + 1, cx);
            }
            Some(Value::DateTime(dt)) => {
                let sub = match next.as_str() {
                    "date" => Some(Value::make_date(dt.date_naive().into())),
                    "time" => Some(Value::make_time(dt.time().into())),
                    "tz" => Some(Value::make_str(&dt.timezone_short_name())),
                    _ => None,
                };
                return matches_at(f, path, sub, level + 1, cx);
            }
            Some(Value::List(_)) => return false,
            _ => val = None,
        }
    }
    match f {
        Filter::Has(_) => val.is_some(),
        Filter::Missing(_) => val.is_none(),
        Filter::Cmp(_, op, want) => match val {
            None => false,
            Some(v) => compare(&v, *op, want),
        },
        _ => false,
    }
}

fn same_unit(a: &libhaystack::val::Number, b: &libhaystack::val::Number) -> bool {
    a.unit.map(|u| u.symbol().to_string()) == b.unit.map(|u| u.symbol().to_string())
}

/// Equality with Haystack semantics: Refs by id, Numbers by value and unit.
pub fn values_equal(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => same_unit(x, y) && x.value == y.value,
        (Value::Ref(x), Value::Ref(y)) => x.value() == y.value(),
        _ => a == b,
    }
}

fn order(a: &Value, b: &Value) -> Option<Ordering> {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) if same_unit(x, y) => x.value.partial_cmp(&y.value),
        (Value::Str(x), Value::Str(y)) => Some(x.value().cmp(y.value())),
        (Value::Uri(x), Value::Uri(y)) => Some(x.value().cmp(y.value())),
        (Value::Ref(x), Value::Ref(y)) => Some(x.value().cmp(y.value())),
        (Value::Date(x), Value::Date(y)) => x.partial_cmp(y),
        (Value::Time(x), Value::Time(y)) => x.partial_cmp(y),
        (Value::DateTime(x), Value::DateTime(y)) => Some((**x).cmp(&**y)),
        (Value::Bool(x), Value::Bool(y)) => Some(x.value.cmp(&y.value)),
        _ => None,
    }
}

fn compare(v: &Value, op: Op, want: &Value) -> bool {
    match op {
        Op::Eq => values_equal(v, want),
        Op::Ne => !values_equal(v, want),
        _ => match order(v, want) {
            None => false, // different kinds or units never compare
            Some(o) => match op {
                Op::Lt => o == Ordering::Less,
                Op::Le => o != Ordering::Greater,
                Op::Gt => o == Ordering::Greater,
                Op::Ge => o != Ordering::Less,
                _ => unreachable!(),
            },
        },
    }
}

// ------------------------------------------------------------------ lexer

#[derive(Debug, Clone, PartialEq)]
enum Tok {
    Id(String),
    Symbol(String),
    Lit(Value),
    LParen,
    RParen,
    Arrow,
    Op(Op),
    Dot,
    Colon2,
    Eof,
}

impl fmt::Display for Tok {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Tok::Id(s) => write!(f, "'{s}'"),
            Tok::Symbol(s) => write!(f, "^{s}"),
            Tok::Lit(_) => write!(f, "a value"),
            Tok::LParen => write!(f, "'('"),
            Tok::RParen => write!(f, "')'"),
            Tok::Arrow => write!(f, "'->'"),
            Tok::Op(_) => write!(f, "an operator"),
            Tok::Dot => write!(f, "'.'"),
            Tok::Colon2 => write!(f, "'::'"),
            Tok::Eof => write!(f, "end of filter"),
        }
    }
}

fn is_id_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_id(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Characters that end a literal value in a filter.
fn ends_literal(c: char) -> bool {
    c.is_whitespace() || matches!(c, '(' | ')' | ',' | '=' | '!' | '<' | '>')
}

fn lex(text: &str) -> Result<Vec<(Tok, usize)>, ParseError> {
    let s: Vec<char> = text.chars().collect();
    let mut i = 0;
    let mut out = Vec::new();
    let err = |msg: String, pos: usize| ParseError { msg, pos };
    while i < s.len() {
        let c = s[i];
        let at = i;
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        if c == '/' && s.get(i + 1) == Some(&'/') {
            while i < s.len() && s[i] != '\n' {
                i += 1;
            }
            continue;
        }
        let two: String = s[i..(i + 2).min(s.len())].iter().collect();
        let tok = match two.as_str() {
            "->" => Some((Tok::Arrow, 2)),
            "==" => Some((Tok::Op(Op::Eq), 2)),
            "!=" => Some((Tok::Op(Op::Ne), 2)),
            "<=" => Some((Tok::Op(Op::Le), 2)),
            ">=" => Some((Tok::Op(Op::Ge), 2)),
            "::" => Some((Tok::Colon2, 2)),
            _ => match c {
                '(' => Some((Tok::LParen, 1)),
                ')' => Some((Tok::RParen, 1)),
                '<' => Some((Tok::Op(Op::Lt), 1)),
                '>' => Some((Tok::Op(Op::Gt), 1)),
                '.' => Some((Tok::Dot, 1)),
                _ => None,
            },
        };
        if let Some((t, n)) = tok {
            out.push((t, at));
            i += n;
            continue;
        }
        if is_id_start(c) {
            let mut id = String::new();
            while i < s.len() && is_id(s[i]) {
                id.push(s[i]);
                i += 1;
            }
            out.push((Tok::Id(id), at));
            continue;
        }
        if c == '^' {
            i += 1;
            let mut sym = String::new();
            while i < s.len() && (is_id(s[i]) || matches!(s[i], '-' | ':' | '.')) {
                sym.push(s[i]);
                i += 1;
            }
            if sym.is_empty() {
                return Err(err("empty symbol".into(), at));
            }
            out.push((Tok::Symbol(sym), at));
            continue;
        }
        // literal values: quoted, ref, or numeric/date/time
        let start = i;
        match c {
            '"' | '`' => {
                i += 1;
                while i < s.len() && s[i] != c {
                    if s[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                if i >= s.len() {
                    return Err(err("unterminated literal".into(), at));
                }
                i += 1;
            }
            '@' => {
                i += 1;
                while i < s.len() && (is_id(s[i]) || matches!(s[i], ':' | '-' | '.' | '~')) {
                    i += 1;
                }
            }
            c if c.is_ascii_digit()
                || (c == '-' && s.get(i + 1).is_some_and(char::is_ascii_digit)) =>
            {
                i += 1;
                while i < s.len() && !ends_literal(s[i]) {
                    i += 1;
                }
                // a DateTime may carry a timezone name after one space
                let lit: String = s[start..i].iter().collect();
                if lit.len() >= 20
                    && lit.as_bytes().get(10) == Some(&b'T')
                    && s.get(i) == Some(&' ')
                    && s.get(i + 1).is_some_and(char::is_ascii_uppercase)
                {
                    i += 1;
                    while i < s.len() && !ends_literal(s[i]) {
                        i += 1;
                    }
                }
            }
            _ => return Err(err(format!("unexpected '{c}'"), at)),
        }
        let lit: String = s[start..i].iter().collect();
        out.push((Tok::Lit(literal(&lit).map_err(|m| err(m, at))?), at));
    }
    out.push((Tok::Eof, s.len()));
    Ok(out)
}

/// A literal's text -> value. Zinc syntax, plus the filter extras: hex
/// numbers (`0xabcd_0123`) and times without a leading zero (`8:30`).
fn literal(t: &str) -> Result<Value, String> {
    if let Some(hex) = t.strip_prefix("0x") {
        let digits: String = hex.chars().filter(|c| *c != '_').collect();
        return i64::from_str_radix(&digits, 16)
            .map(|n| Value::make_number(n as f64))
            .map_err(|_| format!("invalid hex number {t}"));
    }
    let b = t.as_bytes();
    let is_time = (b.len() >= 4 && b[1] == b':' && b[0].is_ascii_digit())
        || (b.len() >= 5 && b[2] == b':' && b[0].is_ascii_digit() && b[1].is_ascii_digit());
    let fixed;
    let t = if is_time && b[1] == b':' {
        fixed = format!("0{t}");
        fixed.as_str()
    } else {
        t
    };
    openhay_codec::decode_zinc_scalar(t).map_err(|e| format!("invalid value {t}: {e}"))
}

// ------------------------------------------------------------------ parser

struct Parser {
    toks: Vec<(Tok, usize)>,
    i: usize,
}

impl Parser {
    fn cur(&self) -> &Tok {
        &self.toks[self.i].0
    }

    fn peek(&self) -> &Tok {
        self.toks.get(self.i + 1).map(|t| &t.0).unwrap_or(&Tok::Eof)
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.i].0.clone();
        if self.i + 1 < self.toks.len() {
            self.i += 1;
        }
        t
    }

    fn err(&self, msg: &str) -> ParseError {
        ParseError {
            msg: msg.into(),
            pos: self.toks[self.i].1,
        }
    }

    fn is_keyword(&self, kw: &str) -> bool {
        matches!(self.cur(), Tok::Id(s) if s == kw)
    }

    fn cond_or(&mut self) -> Result<Filter, ParseError> {
        let lhs = self.cond_and()?;
        if !self.is_keyword("or") {
            return Ok(lhs);
        }
        self.bump();
        Ok(Filter::Or(Box::new(lhs), Box::new(self.cond_or()?)))
    }

    fn cond_and(&mut self) -> Result<Filter, ParseError> {
        let lhs = self.term()?;
        if !self.is_keyword("and") {
            return Ok(lhs);
        }
        self.bump();
        Ok(Filter::And(Box::new(lhs), Box::new(self.cond_and()?)))
    }

    fn term(&mut self) -> Result<Filter, ParseError> {
        match self.cur().clone() {
            Tok::LParen => {
                self.bump();
                let f = self.cond_or()?;
                if self.cur() != &Tok::RParen {
                    return Err(self.err("expected ')'"));
                }
                self.bump();
                Ok(f)
            }
            Tok::Id(s) if s == "not" && matches!(self.peek(), Tok::Id(_)) => {
                self.bump();
                Ok(Filter::Missing(self.path()?))
            }
            Tok::Symbol(s) => {
                self.bump();
                Ok(Filter::IsSymbol(s))
            }
            Tok::Id(s) if s.starts_with(|c: char| c.is_ascii_uppercase()) => {
                self.bump();
                Ok(Filter::IsSpec(s))
            }
            Tok::Id(_) if matches!(self.peek(), Tok::Colon2 | Tok::Dot) => {
                Ok(Filter::IsSpec(self.spec_qname()?))
            }
            Tok::Id(_) => {
                let p = self.path()?;
                if let Tok::Op(op) = self.cur().clone() {
                    self.bump();
                    return Ok(Filter::Cmp(p, op, self.val()?));
                }
                Ok(Filter::Has(p))
            }
            other => Err(self.err(&format!("expected a tag name, found {other}"))),
        }
    }

    fn path(&mut self) -> Result<Vec<String>, ParseError> {
        let mut p = vec![self.name()?];
        while self.cur() == &Tok::Arrow {
            self.bump();
            p.push(self.name()?);
        }
        Ok(p)
    }

    fn name(&mut self) -> Result<String, ParseError> {
        match self.cur().clone() {
            Tok::Id(s) => {
                self.bump();
                Ok(s)
            }
            other => Err(self.err(&format!("expected a tag name, found {other}"))),
        }
    }

    fn val(&mut self) -> Result<Value, ParseError> {
        match self.cur().clone() {
            Tok::Lit(v) => {
                self.bump();
                Ok(v)
            }
            Tok::Id(s) if s == "true" || s == "false" => {
                self.bump();
                Ok(Value::make_bool(s == "true"))
            }
            Tok::Symbol(s) => {
                self.bump();
                Ok(Value::make_symbol(&s))
            }
            other => Err(self.err(&format!("expected a value, found {other}"))),
        }
    }

    fn spec_qname(&mut self) -> Result<String, ParseError> {
        let mut q = self.name()?;
        while self.cur() == &Tok::Dot {
            self.bump();
            q.push('.');
            q.push_str(&self.name()?);
        }
        if self.cur() != &Tok::Colon2 {
            return Err(self.err("expected '::' in spec name"));
        }
        self.bump();
        let spec = self.name()?;
        if !spec.starts_with(|c: char| c.is_ascii_uppercase()) {
            return Err(self.err("spec name must be capitalized"));
        }
        Ok(format!("{q}::{spec}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn rec(pairs: &[(&str, Value)]) -> Dict {
        let mut d = Dict::new();
        for (k, v) in pairs {
            d.insert((*k).into(), v.clone());
        }
        d
    }

    struct Rows(HashMap<String, Dict>);
    impl Resolver for Rows {
        fn deref(&self, r: &Ref) -> Option<Dict> {
            self.0.get(r.value()).cloned()
        }
    }

    #[test]
    fn missing_tags_and_other_kinds_never_compare() {
        let f = Filter::parse("foo < 13").unwrap();
        assert!(!f.matches(&rec(&[]), &NoRefs));
        assert!(!f.matches(&rec(&[("foo", Value::make_str("baz"))]), &NoRefs));
        assert!(f.matches(&rec(&[("foo", Value::make_number(12.0))]), &NoRefs));
        assert!(
            !Filter::parse("foo != 13")
                .unwrap()
                .matches(&rec(&[]), &NoRefs)
        );
    }

    #[test]
    fn units_must_match() {
        let f75 = libhaystack::units::get_unit("°F").unwrap();
        let r = rec(&[("num", Value::make_number_unit(75.0, f75))]);
        assert!(!Filter::parse("num==75").unwrap().matches(&r, &NoRefs));
        assert!(Filter::parse("num==75°F").unwrap().matches(&r, &NoRefs));
        assert!(!Filter::parse("num > 70°C").unwrap().matches(&r, &NoRefs));
    }

    #[test]
    fn ref_lists_fan_out() {
        let mut rows = HashMap::new();
        rows.insert(
            "c".to_string(),
            rec(&[("dis", Value::make_str("c")), ("air", Value::make_marker())]),
        );
        let cx = Rows(rows);
        let r = rec(&[("refx", Value::make_list(vec![Value::make_ref("c")]))]);
        assert!(Filter::parse("refx->dis==\"c\"").unwrap().matches(&r, &cx));
        assert!(Filter::parse("refx==@c").unwrap().matches(&r, &cx));
        assert!(!Filter::parse("not refx->air").unwrap().matches(&r, &cx));
        let s = rec(&[("str", Value::make_list(vec![Value::make_str("q")]))]);
        assert!(!Filter::parse("str==\"q\"").unwrap().matches(&s, &cx));
    }

    #[test]
    fn grammar_edges() {
        assert_eq!(
            Filter::parse("not").unwrap(),
            Filter::Has(vec!["not".into()])
        );
        assert_eq!(
            Filter::parse("not not").unwrap(),
            Filter::Missing(vec!["not".into()])
        );
        assert!(matches!(
            Filter::parse("num > 0xabcd").unwrap(),
            Filter::Cmp(_, Op::Gt, _)
        ));
        assert!(matches!(
            Filter::parse("foo < 8:30").unwrap(),
            Filter::Cmp(_, Op::Lt, Value::Time(_))
        ));
        assert!(matches!(
            Filter::parse("lib==^lib:phIoT").unwrap(),
            Filter::Cmp(_, Op::Eq, Value::Symbol(_))
        ));
        assert_eq!(
            Filter::parse("ph.points::AirTempSensor").unwrap(),
            Filter::IsSpec("ph.points::AirTempSensor".into())
        );
        assert_eq!(
            Filter::parse("a // and foo==5").unwrap(),
            Filter::Has(vec!["a".into()])
        );
        for bad in [
            "",
            "n:",
            "n-foo",
            "n->",
            "(x",
            "(x and",
            "n==",
            "n==\"..",
            "n==`..",
            "foo==Foo(\"xxx\")",
            "n=3x5",
        ] {
            assert!(Filter::parse(bad).is_err(), "{bad}");
        }
    }
}
