//! Zinc reader: grids, single values and tag lists.
//!
//! openhay's own recursive-descent parser for the Zinc grammar
//! (project-haystack.org `docHaystack/Zinc`), checked against haxall's
//! reference test cases (`openhay.vectors`). It replaces libhaystack's
//! decoder, which clamped large exponents to `i64::MAX`, dropped rows after
//! nested grids, rejected `_`-prefixed names and read `C("")` as a Coord.
//!
//! Values are libhaystack `Value`s, so everything downstream is unchanged.

use libhaystack::units::get_unit;
use libhaystack::val::{Column, Coord, Date, DateTime, Dict, Grid, List, Ref, Time, Value, XStr};

pub type Result<T> = std::result::Result<T, String>;

/// Haystack names for POSIX-style legacy zones some servers still emit.
const TZ_ALIASES: &[(&str, &str)] = &[
    ("PST8PDT", "Los_Angeles"),
    ("MST7MDT", "Denver"),
    ("CST6CDT", "Chicago"),
    ("EST5EDT", "New_York"),
];

/// Read a whole grid (top level, starting at `ver:`).
pub fn read_grid(text: &str) -> Result<Grid> {
    let mut p = Parser::new(text);
    p.skip_blank_lines();
    let g = p.grid(false)?;
    p.skip_ws_nl();
    if !p.eof() {
        return Err(p.err("unexpected content after grid"));
    }
    Ok(g)
}

/// Read one value (`72°F`, `@abc "Dis"`, `[1,2]`, `{a:1}`, `<<...>>`).
pub fn read_value(text: &str) -> Result<Value> {
    let mut p = Parser::new(text);
    p.skip_ws_nl();
    let v = p.val()?;
    p.skip_ws_nl();
    if !p.eof() {
        return Err(p.err("unexpected content after value"));
    }
    Ok(v)
}

/// Read a tag list (`a:1 b c:"x"`), commas between tags allowed.
pub fn read_tags(text: &str) -> Result<Dict> {
    let mut p = Parser::new(text);
    let d = p.tags(&['\0'])?;
    p.skip_ws_nl();
    if !p.eof() {
        return Err(p.err("unexpected content after tags"));
    }
    Ok(d)
}

struct Parser {
    s: Vec<char>,
    i: usize,
    line: usize,
}

fn is_id_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '_'
}

fn is_id(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_unit_char(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '%' | '_' | '/' | '$') || (c as u32) > 127
}

impl Parser {
    fn new(text: &str) -> Self {
        Self {
            s: text.chars().collect(),
            i: 0,
            line: 1,
        }
    }

    fn eof(&self) -> bool {
        self.i >= self.s.len()
    }

    fn cur(&self) -> char {
        self.s.get(self.i).copied().unwrap_or('\0')
    }

    fn peek(&self, n: usize) -> char {
        self.s.get(self.i + n).copied().unwrap_or('\0')
    }

    fn bump(&mut self) -> char {
        let c = self.cur();
        if c == '\n' {
            self.line += 1;
        }
        self.i += 1;
        c
    }

    fn err(&self, msg: &str) -> String {
        format!("{msg} [line {}]", self.line)
    }

    fn expect(&mut self, c: char) -> Result<()> {
        if self.cur() != c {
            return Err(self.err(&format!("expected '{c}', found '{}'", self.cur())));
        }
        self.bump();
        Ok(())
    }

    fn skip_ws(&mut self) {
        while matches!(self.cur(), ' ' | '\t' | '\u{a0}') {
            self.bump();
        }
    }

    fn skip_ws_nl(&mut self) {
        while matches!(self.cur(), ' ' | '\t' | '\r' | '\n' | '\u{a0}') {
            self.bump();
        }
    }

    fn skip_blank_lines(&mut self) {
        loop {
            let start = self.i;
            self.skip_ws();
            if matches!(self.cur(), '\r' | '\n') {
                self.end_line();
            } else {
                self.i = start;
                return;
            }
        }
    }

    /// At end of a line: consume `\r?\n`. True if one was consumed.
    fn end_line(&mut self) -> bool {
        if self.cur() == '\r' {
            self.bump();
        }
        if self.cur() == '\n' {
            self.bump();
            return true;
        }
        false
    }

    fn at_line_end(&self) -> bool {
        matches!(self.cur(), '\r' | '\n' | '\0')
    }

    fn at_nested_end(&self) -> bool {
        self.cur() == '>' && self.peek(1) == '>'
    }

    fn id(&mut self) -> Result<String> {
        if !is_id_start(self.cur()) {
            return Err(self.err(&format!("expected a name, found '{}'", self.cur())));
        }
        let mut out = String::new();
        while is_id(self.cur()) {
            out.push(self.bump());
        }
        Ok(out)
    }

    // ------------------------------------------------------------- grids

    fn grid(&mut self, nested: bool) -> Result<Grid> {
        // ver:"x.y"
        let ver_name = self.id()?;
        if ver_name != "ver" {
            return Err(self.err("grid must start with ver:"));
        }
        self.expect(':')?;
        let ver = match self.val()? {
            Value::Str(s) => s.value().to_string(),
            _ => return Err(self.err("ver must be a string")),
        };
        if ver != "3.0" && ver != "2.0" {
            return Err(self.err(&format!("unsupported version \"{ver}\"")));
        }
        let meta = self.tags(&['\n', '\r'])?;
        if !self.end_line() {
            return Err(self.err("expected newline after grid meta"));
        }

        // columns
        self.skip_ws();
        let mut columns = Vec::new();
        loop {
            self.skip_ws();
            let name = self.id()?;
            let cmeta = self.tags(&[',', '\n', '\r'])?;
            columns.push(Column {
                name,
                meta: (!cmeta.is_empty()).then_some(cmeta),
            });
            self.skip_ws();
            if self.cur() == ',' {
                self.bump();
                continue;
            }
            break;
        }
        if !self.end_line() && !self.eof() {
            return Err(self.err("expected newline after columns"));
        }
        if columns.len() == 1 && columns[0].name == "empty" && columns[0].meta.is_none() {
            columns.clear();
        }

        // rows: until EOF, or `>>` for a nested grid; blank lines are skipped
        let mut rows = Vec::new();
        loop {
            self.skip_blank_lines();
            self.skip_ws();
            if self.eof() || (nested && self.at_nested_end()) {
                break;
            }
            let mut row = Dict::new();
            for (ci, col) in columns.iter().enumerate() {
                if ci > 0 {
                    self.skip_ws();
                    if self.cur() != ',' {
                        break; // short row: remaining cells are null
                    }
                    self.bump();
                }
                self.skip_ws();
                if matches!(self.cur(), ',' | '\r' | '\n' | '\0')
                    || (nested && self.at_nested_end())
                {
                    continue; // empty cell = null
                }
                let v = self.val()?;
                if !matches!(v, Value::Null) {
                    row.insert(col.name.clone(), v);
                }
            }
            self.skip_ws();
            if columns.is_empty() && !self.at_line_end() {
                return Err(self.err("row in a grid with no columns"));
            }
            if !self.at_line_end() && !(nested && self.at_nested_end()) {
                return Err(self.err(&format!("unexpected '{}' in row", self.cur())));
            }
            self.end_line();
            rows.push(row);
        }

        let mut meta = meta;
        meta.remove("ver");
        Ok(Grid {
            meta: (!meta.is_empty()).then_some(meta),
            columns,
            rows,
            ver: "3.0".into(),
        })
    }

    /// `name[:val]` pairs separated by spaces (and optional commas inside
    /// `{}`), until one of `stop` or end of input.
    fn tags(&mut self, stop: &[char]) -> Result<Dict> {
        let mut d = Dict::new();
        loop {
            self.skip_ws();
            let c = self.cur();
            if c == '\0' || stop.contains(&c) || (c == ',' && stop.contains(&',')) {
                return Ok(d);
            }
            if c == ',' {
                self.bump();
                continue;
            }
            if !is_id_start(c) {
                return Ok(d);
            }
            let name = self.id()?;
            self.skip_ws();
            let v = if self.cur() == ':' {
                self.bump();
                self.skip_ws();
                self.val()?
            } else {
                Value::make_marker()
            };
            d.insert(name, v);
        }
    }

    // ------------------------------------------------------------- values

    fn val(&mut self) -> Result<Value> {
        let c = self.cur();
        match c {
            '<' if self.peek(1) == '<' => {
                self.bump();
                self.bump();
                self.skip_ws_nl();
                let g = self.grid(true)?;
                self.skip_ws_nl();
                self.expect('>')?;
                self.expect('>')?;
                Ok(Value::make_grid(g))
            }
            '[' => self.list(),
            '{' => {
                self.bump();
                self.skip_ws_nl();
                let d = self.dict_body()?;
                Ok(Value::make_dict(d))
            }
            '"' => Ok(Value::make_str(&self.str_lit()?)),
            '`' => Ok(Value::make_uri(&self.uri_lit()?)),
            '@' => self.ref_lit(),
            '^' => {
                self.bump();
                let mut out = String::new();
                while is_id(self.cur()) || matches!(self.cur(), ':' | '-' | '.' | '~') {
                    out.push(self.bump());
                }
                if out.is_empty() {
                    return Err(self.err("invalid empty Symbol"));
                }
                Ok(Value::make_symbol(&out))
            }
            '-' if self.peek(1) == 'I' => {
                self.bump();
                let w = self.id()?;
                if w != "INF" {
                    return Err(self.err(&format!("unexpected -{w}")));
                }
                Ok(Value::make_number(f64::NEG_INFINITY))
            }
            c if c.is_ascii_digit() || (c == '-' && self.peek(1).is_ascii_digit()) => {
                self.numeric()
            }
            c if c.is_ascii_alphabetic() => self.keyword_or_xstr(),
            _ => Err(self.err(&format!("unexpected '{c}'"))),
        }
    }

    fn list(&mut self) -> Result<Value> {
        self.expect('[')?;
        let mut items: List = Vec::new();
        loop {
            self.skip_ws_nl();
            if self.cur() == ']' {
                self.bump();
                return Ok(Value::make_list(items));
            }
            items.push(self.val()?);
            self.skip_ws_nl();
            if self.cur() == ',' {
                self.bump();
            } else if self.cur() != ']' {
                return Err(self.err(&format!(
                    "expected ',' or ']' in list, found '{}'",
                    self.cur()
                )));
            }
        }
    }

    fn dict_body(&mut self) -> Result<Dict> {
        let mut d = Dict::new();
        loop {
            self.skip_ws_nl();
            match self.cur() {
                '}' => {
                    self.bump();
                    return Ok(d);
                }
                ',' => {
                    self.bump();
                    continue;
                }
                '\0' => return Err(self.err("unterminated dict")),
                _ => {}
            }
            let name = self.id()?;
            self.skip_ws();
            let v = if self.cur() == ':' {
                self.bump();
                self.skip_ws_nl();
                self.val()?
            } else {
                Value::make_marker()
            };
            d.insert(name, v);
        }
    }

    fn str_lit(&mut self) -> Result<String> {
        self.expect('"')?;
        let mut out = String::new();
        loop {
            match self.bump() {
                '\0' => return Err(self.err("unterminated string")),
                '"' => return Ok(out),
                '\\' => out.push(self.escape(false)?),
                c => out.push(c),
            }
        }
    }

    fn uri_lit(&mut self) -> Result<String> {
        self.expect('`')?;
        let mut out = String::new();
        loop {
            match self.bump() {
                '\0' => return Err(self.err("unterminated uri")),
                '`' => return Ok(out),
                // In a Uri only \` is an escape; other backslashes (\#, \/)
                // are kept as written - they're URI-level escapes.
                '\\' if self.cur() == '`' => out.push(self.bump()),
                c => out.push(c),
            }
        }
    }

    fn escape(&mut self, _uri: bool) -> Result<char> {
        Ok(match self.bump() {
            'b' => '\u{8}',
            'f' => '\u{c}',
            'n' => '\n',
            'r' => '\r',
            't' => '\t',
            '"' => '"',
            '\\' => '\\',
            '$' => '$',
            '`' => '`',
            'u' => {
                let hex: String = (0..4).map(|_| self.bump()).collect();
                let code =
                    u32::from_str_radix(&hex, 16).map_err(|_| self.err("invalid \\u escape"))?;
                char::from_u32(code).ok_or_else(|| self.err("invalid \\u escape"))?
            }
            c => return Err(self.err(&format!("invalid escape \\{c}"))),
        })
    }

    fn ref_lit(&mut self) -> Result<Value> {
        self.expect('@')?;
        let mut id = String::new();
        while is_id(self.cur()) || matches!(self.cur(), ':' | '-' | '.' | '~') {
            id.push(self.bump());
        }
        if id.is_empty() {
            return Err(self.err("Invalid empty Ref"));
        }
        // optional display string: `@id "Dis"`
        let save = (self.i, self.line);
        self.skip_ws();
        if self.cur() == '"' {
            let dis = self.str_lit()?;
            return Ok(Value::make_ref_with_dis(&id, &dis));
        }
        (self.i, self.line) = save;
        Ok(Ref::make(&id, None).into())
    }

    fn keyword_or_xstr(&mut self) -> Result<Value> {
        let start = self.i;
        let mut word = String::new();
        while is_id(self.cur()) {
            word.push(self.bump());
        }
        if self.cur() == '(' {
            self.bump();
            self.skip_ws();
            if self.cur() == '"' {
                let v = self.str_lit()?;
                self.skip_ws();
                self.expect(')')?;
                return Ok(XStr::make(word.as_str(), v.as_str()).into());
            }
            if word == "C" {
                let lat = self.plain_number()?;
                self.skip_ws();
                self.expect(',')?;
                self.skip_ws();
                let lng = self.plain_number()?;
                self.skip_ws();
                self.expect(')')?;
                return Ok(Coord::make(lat, lng).into());
            }
            if word == "Bin" {
                // Zinc 2.0: Bin(text/plain; charset=utf8), mime unquoted
                let mut mime = String::new();
                while !matches!(self.cur(), ')' | '\0' | '\n') {
                    mime.push(self.bump());
                }
                self.expect(')')?;
                let mime = mime.split(';').map(str::trim).collect::<Vec<_>>().join(";");
                return Ok(XStr::make("Bin", mime.as_str()).into());
            }
            return Err(self.err(&format!("expected a string in {word}(...)")));
        }
        Ok(match word.as_str() {
            "N" => Value::Null,
            "M" => Value::make_marker(),
            "R" => Value::make_remove(),
            "NA" => Value::make_na(),
            "T" => Value::make_true(),
            "F" => Value::make_false(),
            "NaN" => Value::make_number(f64::NAN),
            "INF" => Value::make_number(f64::INFINITY),
            _ => {
                self.i = start;
                return Err(self.err(&format!("unexpected identifier {word}")));
            }
        })
    }

    fn plain_number(&mut self) -> Result<f64> {
        let mut t = String::new();
        if self.cur() == '-' {
            t.push(self.bump());
        }
        while self.cur().is_ascii_digit() || matches!(self.cur(), '.' | 'e' | 'E' | '+' | '-' | '_')
        {
            let c = self.bump();
            if c != '_' {
                t.push(c);
            }
        }
        t.parse::<f64>()
            .map_err(|_| self.err(&format!("invalid number {t}")))
    }

    /// Number (with unit), Date, Time or DateTime: decided by shape.
    fn numeric(&mut self) -> Result<Value> {
        let start = self.i;
        let neg = self.cur() == '-';
        let mut digits = 0;
        let mut j = self.i + usize::from(neg);
        while self.s.get(j).is_some_and(char::is_ascii_digit) {
            digits += 1;
            j += 1;
        }
        let next = self.s.get(j).copied().unwrap_or('\0');
        if !neg && digits == 4 && next == '-' {
            return self.date_or_datetime();
        }
        if !neg && (digits == 2 || digits == 1) && next == ':' {
            return self.time_lit().map(Value::make_time);
        }
        self.i = start;
        self.number()
    }

    fn number(&mut self) -> Result<Value> {
        let mut t = String::new();
        if self.cur() == '-' {
            t.push(self.bump());
        }
        let mut seen_e = false;
        loop {
            let c = self.cur();
            if c.is_ascii_digit() || c == '.' && self.peek(1).is_ascii_digit() {
                t.push(self.bump());
            } else if c == '_' && self.peek(1).is_ascii_digit() {
                self.bump();
            } else if (c == 'e' || c == 'E')
                && !seen_e
                && (self.peek(1).is_ascii_digit()
                    || (matches!(self.peek(1), '+' | '-') && self.peek(2).is_ascii_digit()))
            {
                seen_e = true;
                t.push(self.bump());
                if matches!(self.cur(), '+' | '-') {
                    t.push(self.bump());
                }
            } else {
                break;
            }
        }
        let value: f64 = t
            .parse()
            .map_err(|_| self.err(&format!("invalid number {t}")))?;
        let mut unit = String::new();
        while is_unit_char(self.cur()) {
            unit.push(self.bump());
        }
        if unit.is_empty() {
            return Ok(Value::make_number(value));
        }
        match get_unit(&unit) {
            Some(u) => Ok(Value::make_number_unit(value, u)),
            // Not in the Haystack unit database: keep the number. (libhaystack's
            // Number can only hold database units - a known limitation.)
            None => Ok(Value::make_number(value)),
        }
    }

    fn take_digits(&mut self, n: usize) -> Result<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let c = self.bump();
            let d = c
                .to_digit(10)
                .ok_or_else(|| self.err(&format!("expected digit, found '{c}'")))?;
            v = v * 10 + d;
        }
        Ok(v)
    }

    fn time_lit(&mut self) -> Result<Time> {
        let h = if self.peek(1) == ':' {
            self.take_digits(1)?
        } else {
            self.take_digits(2)?
        };
        self.expect(':')?;
        let m = self.take_digits(2)?;
        let mut s = 0;
        let mut nanos = 0u32;
        if self.cur() == ':' {
            self.bump();
            s = self.take_digits(2)?;
            if self.cur() == '.' && self.peek(1).is_ascii_digit() {
                self.bump();
                let mut frac = String::new();
                while self.cur().is_ascii_digit() {
                    frac.push(self.bump());
                }
                frac.truncate(9);
                while frac.len() < 9 {
                    frac.push('0');
                }
                nanos = frac.parse().unwrap_or(0);
            }
        }
        chrono::NaiveTime::from_hms_nano_opt(h, m, s, nanos)
            .map(Time::from)
            .ok_or_else(|| self.err("invalid time"))
    }

    fn date_or_datetime(&mut self) -> Result<Value> {
        let start = self.i;
        let y = self.take_digits(4)? as i32;
        self.expect('-')?;
        let mo = self.take_digits(2)?;
        self.expect('-')?;
        let d = self.take_digits(2)?;
        if self.cur() != 'T' {
            return Date::from_ymd(y, mo, d)
                .map(Value::make_date)
                .map_err(|e| self.err(&e));
        }
        // DateTime: YYYY-MM-DDThh:mm:ss[.fff](Z|±hh:mm)[ TzName]
        self.bump();
        let mut iso: String = self.s[start..self.i].iter().collect();
        while self.cur().is_ascii_digit() || matches!(self.cur(), ':' | '.') {
            iso.push(self.bump());
        }
        if self.cur() == 'Z' {
            iso.push(self.bump());
        } else if matches!(self.cur(), '+' | '-') {
            for _ in 0..6 {
                iso.push(self.bump());
            }
        } else {
            return Err(self.err("DateTime needs Z or an offset"));
        }
        // optional timezone name after one space
        let mut tz = None;
        // Haystack timezone names are capitalized (UTC, London, New_York,
        // GMT+5); a lowercase word after the DateTime is the next tag.
        if self.cur() == ' ' && self.peek(1).is_ascii_uppercase() {
            self.bump();
            let mut name = String::new();
            while is_id(self.cur()) || matches!(self.cur(), '-' | '+' | '/') {
                name.push(self.bump());
            }
            tz = Some(name);
        }
        let tz = tz.unwrap_or_else(|| "UTC".into());
        let tz = TZ_ALIASES
            .iter()
            .find(|(a, _)| *a == tz)
            .map(|(_, b)| (*b).to_string())
            .unwrap_or(tz);
        DateTime::parse_from_rfc3339_with_timezone(&iso, &tz)
            .map(Value::make_datetime)
            .map_err(|e| self.err(&format!("{e} ({iso} {tz})")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn num(v: &Value) -> f64 {
        match v {
            Value::Number(n) => n.value,
            other => panic!("not a number: {other:?}"),
        }
    }

    #[test]
    fn big_exponents_are_not_clamped() {
        assert_eq!(num(&read_value("2.4e20").unwrap()), 2.4e20);
        assert_eq!(num(&read_value("123e-10").unwrap()), 1.23e-8);
        assert_eq!(num(&read_value("1_000").unwrap()), 1000.0);
    }

    #[test]
    fn row_after_nested_grid_is_kept() {
        let g = read_grid("ver:\"3.0\"\nval\n<<\nver:\"3.0\"\nx,y\n4,6\n\n>>\n\"foo\"\n").unwrap();
        assert_eq!(g.rows.len(), 2);
    }

    #[test]
    fn xstr_c_is_not_a_coord() {
        assert!(matches!(read_value("C(\"\")").unwrap(), Value::XStr(_)));
        assert!(matches!(read_value("C(12,-34)").unwrap(), Value::Coord(_)));
    }

    #[test]
    fn underscore_names_and_short_rows() {
        let g = read_grid("ver:\"3.0\"\n_foo,b\n1\n").unwrap();
        assert_eq!(g.columns[0].name, "_foo");
        assert_eq!(g.rows.len(), 1);
    }

    #[test]
    fn errors() {
        assert!(read_grid("ver:\"3.0\"\nfoo\n@\n").is_err());
        assert!(read_grid("ver:\"1.0\"\nfoo\n1\n").is_err());
    }
}
