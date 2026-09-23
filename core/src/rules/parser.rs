//! Lexer and parser for the rule language.
//!
//! The language is a strict subset of PostgreSQL expression syntax with the same
//! precedence and the same tokenization, so a legacy check string means the same
//! thing to Core and to PostgreSQL. Anything PostgreSQL could read differently —
//! adjacent strings, prefixed strings, chained comparisons, operator runs the
//! PostgreSQL lexer would glue together — is refused rather than guessed.

use super::ast::{BinOp, Expr, Func, IntervalUnit};

pub(crate) const MAX_SOURCE_BYTES: usize = 2048;
pub(crate) const MAX_DEPTH: usize = 24;
pub(crate) const MAX_NODES: usize = 256;
pub(crate) const MAX_IN_ITEMS: usize = 100;
pub(crate) const MAX_STRING_BYTES: usize = 256;

/// A refusal with its 1-based line and column in the rule source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SyntaxError {
    pub(crate) line: usize,
    pub(crate) column: usize,
    pub(crate) message: String,
}

impl std::fmt::Display for SyntaxError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}:{}: {}", self.line, self.column, self.message)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kw {
    And,
    Or,
    Not,
    Is,
    Null,
    True,
    False,
    Between,
    In,
    Interval,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Tok {
    Ident(String),
    Kw(Kw),
    Number(String),
    Str(String),
    Op(BinOp),
    Minus,
    LParen,
    RParen,
    Comma,
    Eof,
    /// A lexing error. Tokens are read ahead, but the error is only reported
    /// if parsing reaches it, so the earliest problem is the one reported.
    Bad(Box<SyntaxError>),
}

impl Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Ident(name) => format!("'{name}'"),
            Tok::Kw(kw) => format!("{kw:?}").to_uppercase(),
            Tok::Number(n) => n.clone(),
            Tok::Str(_) => "a string".into(),
            Tok::Op(op) => format!("'{}'", op.sql()),
            Tok::Minus => "'-'".into(),
            Tok::LParen => "'('".into(),
            Tok::RParen => "')'".into(),
            Tok::Comma => "','".into(),
            Tok::Eof => "end of rule".into(),
            Tok::Bad(error) => error.message.clone(),
        }
    }
}

/// Characters PostgreSQL's lexer reads as part of one operator token.
const OP_CHARS: &str = "+-*/<>=~!@#%^&|`?";

fn keyword(word: &str) -> Option<Kw> {
    Some(match word.to_ascii_lowercase().as_str() {
        "and" => Kw::And,
        "or" => Kw::Or,
        "not" => Kw::Not,
        "is" => Kw::Is,
        "null" => Kw::Null,
        "true" => Kw::True,
        "false" => Kw::False,
        "between" => Kw::Between,
        "in" => Kw::In,
        "interval" => Kw::Interval,
        _ => return None,
    })
}

/// Words PostgreSQL treats as syntax that would change the meaning of an
/// expression if Core read them as field names.
const RESERVED: &[&str] = &[
    "all", "any", "array", "asymmetric", "both", "case", "cast", "collate", "current_date",
    "current_role", "current_time", "current_timestamp", "current_user", "distinct", "else",
    "end", "exists", "extract", "from", "ilike", "isnull", "leading", "like", "localtime",
    "localtimestamp", "notnull", "overlaps", "row", "select", "session_user", "similar", "some",
    "symmetric", "then", "trailing", "unknown", "user", "values", "when", "where",
];

struct Lexer<'a> {
    src: &'a str,
    pos: usize,
}

impl<'a> Lexer<'a> {
    fn error(&self, at: usize, message: impl Into<String>) -> SyntaxError {
        let before = &self.src[..at.min(self.src.len())];
        let line = before.matches('\n').count() + 1;
        let column = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
        SyntaxError { line, column, message: message.into() }
    }

    fn tokens(mut self) -> Vec<(Tok, usize)> {
        let mut out = Vec::new();
        loop {
            let (tok, at) = match self.next() {
                Ok(token) => token,
                Err(error) => (Tok::Bad(Box::new(error)), self.src.len()),
            };
            let done = matches!(tok, Tok::Eof | Tok::Bad(_));
            out.push((tok, at));
            if done {
                return out;
            }
        }
    }

    fn next(&mut self) -> Result<(Tok, usize), SyntaxError> {
        let bytes = self.src.as_bytes();
        while self.pos < bytes.len() && matches!(bytes[self.pos], b' ' | b'\t' | b'\n' | b'\r') {
            self.pos += 1;
        }
        let start = self.pos;
        let Some(&c) = bytes.get(self.pos) else { return Ok((Tok::Eof, start)) };
        if !c.is_ascii() {
            return Err(self.error(start, "only ASCII is allowed outside string literals"));
        }
        let tok = match c {
            b'(' => { self.pos += 1; Tok::LParen }
            b')' => { self.pos += 1; Tok::RParen }
            b',' => { self.pos += 1; Tok::Comma }
            b'\'' => Tok::Str(self.string(start)?),
            b'"' => return Err(self.error(start, "quoted identifiers are not allowed; use the field name")),
            b'$' => return Err(self.error(start, "'$' is not allowed (no parameters or dollar quoting)")),
            b':' => return Err(self.error(start, "casts are not allowed; Core types literals itself")),
            b'.' => return Err(self.error(start, "qualified names and '.' are not allowed")),
            b';' | b'[' | b']' | b'{' | b'}' | b'\\' => {
                return Err(self.error(start, format!("'{}' is not allowed", c as char)));
            }
            b'0'..=b'9' => Tok::Number(self.number(start)?),
            b'a'..=b'z' | b'A'..=b'Z' | b'_' => self.word(start)?,
            _ if OP_CHARS.contains(c as char) => self.operator(start)?,
            _ => return Err(self.error(start, format!("'{}' is not allowed", c as char))),
        };
        Ok((tok, start))
    }

    fn string(&mut self, start: usize) -> Result<String, SyntaxError> {
        let bytes = self.src.as_bytes();
        let mut value = String::new();
        let mut i = start + 1;
        loop {
            match bytes.get(i) {
                None => return Err(self.error(start, "unterminated string literal")),
                Some(b'\'') if bytes.get(i + 1) == Some(&b'\'') => {
                    value.push('\'');
                    i += 2;
                }
                Some(b'\'') => break,
                Some(_) => {
                    let ch = self.src[i..].chars().next().expect("in bounds");
                    if ch == '\0' {
                        return Err(self.error(i, "NUL is not allowed in a string literal"));
                    }
                    value.push(ch);
                    i += ch.len_utf8();
                }
            }
        }
        self.pos = i + 1;
        if value.len() > MAX_STRING_BYTES {
            return Err(self.error(start, format!("string literals are limited to {MAX_STRING_BYTES} bytes")));
        }
        Ok(value)
    }

    fn number(&mut self, start: usize) -> Result<String, SyntaxError> {
        let bytes = self.src.as_bytes();
        let digits = |mut i: usize| {
            while bytes.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            i
        };
        let mut end = digits(start);
        if bytes.get(end) == Some(&b'.') {
            let fraction = digits(end + 1);
            if fraction == end + 1 {
                return Err(self.error(start, "write numbers as digits with an optional fraction, like 12 or 0.5"));
            }
            end = fraction;
        }
        if bytes.get(end).is_some_and(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.')) {
            return Err(self.error(start, "write numbers as digits with an optional fraction, like 12 or 0.5"));
        }
        if end - start > 40 {
            return Err(self.error(start, "numeric literals are limited to 40 characters"));
        }
        self.pos = end;
        Ok(self.src[start..end].to_string())
    }

    fn word(&mut self, start: usize) -> Result<Tok, SyntaxError> {
        let bytes = self.src.as_bytes();
        let mut end = start;
        while bytes.get(end).is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_') {
            end += 1;
        }
        if bytes.get(end) == Some(&b'$') {
            return Err(self.error(end, "'$' is not allowed in names"));
        }
        let word = &self.src[start..end];
        self.pos = end;
        if let Some(kw) = keyword(word) {
            if bytes.get(end) == Some(&b'\'') && kw != Kw::Interval {
                return Err(self.error(start, "prefixed string literals are not allowed"));
            }
            return Ok(Tok::Kw(kw));
        }
        if bytes.get(end) == Some(&b'\'') {
            // PostgreSQL reads E'…', U&'…', B'…' and `type '…'` as special literals.
            return Err(self.error(start, "prefixed string literals are not allowed"));
        }
        let lower = word.to_ascii_lowercase();
        if RESERVED.contains(&lower.as_str()) {
            return Err(self.error(start, format!("'{word}' is not part of the rule language")));
        }
        Ok(Tok::Ident(word.to_string()))
    }

    /// Mirrors PostgreSQL's scan.l: read the whole operator run, refuse comments,
    /// and strip trailing `+`/`-` only when the run holds none of `~!@#^&|`?%`.
    fn operator(&mut self, start: usize) -> Result<Tok, SyntaxError> {
        let bytes = self.src.as_bytes();
        let mut end = start;
        while bytes.get(end).is_some_and(|b| OP_CHARS.contains(*b as char)) {
            end += 1;
        }
        let run = &self.src[start..end];
        if run.contains("--") || run.contains("/*") {
            return Err(self.error(start, "comments are not allowed"));
        }
        let mut len = run.len();
        if len > 1 && run.ends_with(['+', '-']) && !run.contains(|c| "~!@#^&|`?%".contains(c)) {
            while len > 1 && matches!(bytes[start + len - 1], b'+' | b'-') {
                len -= 1;
            }
        }
        self.pos = start + len;
        Ok(match &run[..len] {
            "=" => Tok::Op(BinOp::Eq),
            "<>" | "!=" => Tok::Op(BinOp::Ne),
            "<" => Tok::Op(BinOp::Lt),
            "<=" => Tok::Op(BinOp::Le),
            ">" => Tok::Op(BinOp::Gt),
            ">=" => Tok::Op(BinOp::Ge),
            "~" => Tok::Op(BinOp::Match),
            "+" => Tok::Op(BinOp::Add),
            "-" => Tok::Minus,
            "*" => Tok::Op(BinOp::Mul),
            other => {
                return Err(self.error(start, format!(
                    "operator '{other}' is not allowed; allowed: = <> != < <= > >= + - * ~"
                )));
            }
        })
    }
}

struct Parser<'a> {
    lexer: Lexer<'a>,
    tokens: Vec<(Tok, usize)>,
    at: usize,
    depth: usize,
    nodes: usize,
}

/// Parse one rule. Depth and node limits are enforced while descending, so an
/// adversarial input cannot exhaust Core's stack.
pub(crate) fn parse(src: &str) -> Result<Expr, SyntaxError> {
    let lexer = Lexer { src, pos: 0 };
    if src.len() > MAX_SOURCE_BYTES {
        return Err(lexer.error(0, format!("rules are limited to {MAX_SOURCE_BYTES} bytes")));
    }
    let tokens = Lexer { src, pos: 0 }.tokens();
    let mut parser = Parser { lexer, tokens, at: 0, depth: 0, nodes: 0 };
    if parser.peek() == &Tok::Eof {
        return Err(parser.error_here("the rule is empty"));
    }
    let expr = parser.or()?;
    match parser.peek() {
        Tok::Eof => Ok(expr),
        Tok::Op(op) if op.is_comparison() => Err(parser.error_here(
            "comparisons do not chain; add parentheses, as PostgreSQL requires",
        )),
        Tok::Kw(Kw::Is) => Err(parser.error_here("add parentheses around the IS test")),
        other => {
            let other = other.describe();
            Err(parser.error_here(format!("unexpected {other}")))
        }
    }
}

impl Parser<'_> {
    fn peek(&self) -> &Tok {
        &self.tokens[self.at].0
    }

    fn peek2(&self) -> &Tok {
        &self.tokens[(self.at + 1).min(self.tokens.len() - 1)].0
    }

    fn bump(&mut self) -> Tok {
        let tok = self.tokens[self.at].0.clone();
        if !matches!(tok, Tok::Eof | Tok::Bad(_)) {
            self.at += 1;
        }
        tok
    }

    fn error_here(&self, message: impl Into<String>) -> SyntaxError {
        match &self.tokens[self.at].0 {
            Tok::Bad(error) => (**error).clone(),
            _ => self.lexer.error(self.tokens[self.at].1, message),
        }
    }

    fn expect(&mut self, tok: Tok, what: &str) -> Result<(), SyntaxError> {
        if self.peek() == &tok {
            self.bump();
            return Ok(());
        }
        let found = self.peek().describe();
        Err(self.error_here(format!("expected {what}, found {found}")))
    }

    fn node(&mut self, expr: Expr) -> Result<Expr, SyntaxError> {
        self.nodes += 1;
        if self.nodes > MAX_NODES {
            return Err(self.error_here(format!("rules are limited to {MAX_NODES} nodes")));
        }
        Ok(expr)
    }

    fn nested<T>(&mut self, f: impl FnOnce(&mut Self) -> Result<T, SyntaxError>) -> Result<T, SyntaxError> {
        self.depth += 1;
        if self.depth > MAX_DEPTH {
            return Err(self.error_here(format!("rules are limited to a nesting depth of {MAX_DEPTH}")));
        }
        let result = f(self);
        self.depth -= 1;
        result
    }

    fn binary(&mut self, op: BinOp, l: Expr, r: Expr) -> Result<Expr, SyntaxError> {
        self.node(Expr::Binary { op, l: Box::new(l), r: Box::new(r) })
    }

    fn or(&mut self) -> Result<Expr, SyntaxError> {
        let mut l = self.and()?;
        while self.peek() == &Tok::Kw(Kw::Or) {
            self.bump();
            let r = self.and()?;
            l = self.binary(BinOp::Or, l, r)?;
        }
        Ok(l)
    }

    fn and(&mut self) -> Result<Expr, SyntaxError> {
        let mut l = self.not()?;
        while self.peek() == &Tok::Kw(Kw::And) {
            self.bump();
            let r = self.not()?;
            l = self.binary(BinOp::And, l, r)?;
        }
        Ok(l)
    }

    fn not(&mut self) -> Result<Expr, SyntaxError> {
        if self.peek() == &Tok::Kw(Kw::Not) {
            self.bump();
            let e = self.nested(Self::not)?;
            return self.node(Expr::Not { e: Box::new(e) });
        }
        self.is()
    }

    fn is(&mut self) -> Result<Expr, SyntaxError> {
        let mut e = self.comparison()?;
        while self.peek() == &Tok::Kw(Kw::Is) {
            self.bump();
            let negated = self.peek() == &Tok::Kw(Kw::Not);
            if negated {
                self.bump();
            }
            self.expect(Tok::Kw(Kw::Null), "NULL (only IS [NOT] NULL is supported)")?;
            e = self.node(Expr::IsNull { e: Box::new(e), negated })?;
        }
        Ok(e)
    }

    fn comparison(&mut self) -> Result<Expr, SyntaxError> {
        let l = self.range()?;
        match self.peek().clone() {
            Tok::Op(op) if op.is_comparison() => {
                self.bump();
                let r = self.range()?;
                self.binary(op, l, r)
            }
            _ => Ok(l),
        }
    }

    fn range(&mut self) -> Result<Expr, SyntaxError> {
        let e = self.other()?;
        let negated = self.peek() == &Tok::Kw(Kw::Not) && matches!(self.peek2(), Tok::Kw(Kw::Between | Kw::In));
        if negated {
            self.bump();
        }
        match self.peek() {
            Tok::Kw(Kw::Between) => {
                self.bump();
                let lo = self.other()?;
                self.expect(Tok::Kw(Kw::And), "AND in BETWEEN")?;
                let hi = self.other()?;
                self.node(Expr::Between { e: Box::new(e), lo: Box::new(lo), hi: Box::new(hi), negated })
            }
            Tok::Kw(Kw::In) => {
                self.bump();
                self.expect(Tok::LParen, "'(' after IN")?;
                let mut list = vec![self.other()?];
                while self.peek() == &Tok::Comma {
                    self.bump();
                    list.push(self.other()?);
                    if list.len() > MAX_IN_ITEMS {
                        return Err(self.error_here(format!("IN lists are limited to {MAX_IN_ITEMS} items")));
                    }
                }
                self.expect(Tok::RParen, "')' closing the IN list")?;
                self.node(Expr::In { e: Box::new(e), list, negated })
            }
            _ => Ok(e),
        }
    }

    fn other(&mut self) -> Result<Expr, SyntaxError> {
        let mut l = self.additive()?;
        while self.peek() == &Tok::Op(BinOp::Match) {
            self.bump();
            let r = self.additive()?;
            l = self.binary(BinOp::Match, l, r)?;
        }
        Ok(l)
    }

    fn additive(&mut self) -> Result<Expr, SyntaxError> {
        let mut l = self.multiplicative()?;
        loop {
            let op = match self.peek() {
                Tok::Op(BinOp::Add) => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => return Ok(l),
            };
            self.bump();
            let r = self.multiplicative()?;
            l = self.binary(op, l, r)?;
        }
    }

    fn multiplicative(&mut self) -> Result<Expr, SyntaxError> {
        let mut l = self.unary()?;
        while self.peek() == &Tok::Op(BinOp::Mul) {
            self.bump();
            let r = self.unary()?;
            l = self.binary(BinOp::Mul, l, r)?;
        }
        Ok(l)
    }

    fn unary(&mut self) -> Result<Expr, SyntaxError> {
        match self.peek() {
            Tok::Minus => {
                self.bump();
                let e = self.nested(Self::unary)?;
                self.node(Expr::Neg { e: Box::new(e) })
            }
            Tok::Op(BinOp::Add) => Err(self.error_here("unary '+' is not allowed")),
            _ => self.atom(),
        }
    }

    fn atom(&mut self) -> Result<Expr, SyntaxError> {
        let at = self.at;
        match self.bump() {
            Tok::Number(value) => self.node(Expr::Number { value }),
            Tok::Str(value) => self.node(Expr::Text { value }),
            Tok::Kw(Kw::True) => self.node(Expr::Bool { value: true }),
            Tok::Kw(Kw::False) => self.node(Expr::Bool { value: false }),
            Tok::Kw(Kw::Null) => {
                self.at = at;
                Err(self.error_here("NULL is only allowed in IS [NOT] NULL"))
            }
            Tok::Kw(Kw::Interval) => self.interval(),
            Tok::LParen => {
                let e = self.nested(Self::or)?;
                self.expect(Tok::RParen, "')'")?;
                Ok(e)
            }
            Tok::Ident(name) if self.peek() == &Tok::LParen => {
                self.at = at;
                self.call(&name)
            }
            Tok::Ident(name) => {
                if name.bytes().any(|b| b.is_ascii_uppercase()) {
                    self.at = at;
                    return Err(self.error_here(format!("field names are lowercase: '{name}'")));
                }
                self.node(Expr::Field { name })
            }
            other => {
                self.at = at;
                let found = other.describe();
                Err(self.error_here(format!("expected a field, literal or function, found {found}")))
            }
        }
    }

    fn call(&mut self, name: &str) -> Result<Expr, SyntaxError> {
        let Some(func) = Func::from_name(&name.to_ascii_lowercase()) else {
            return Err(self.error_here(format!("function '{name}' is not allowed; allowed: {}", Func::NAMES)));
        };
        self.bump();
        self.bump();
        let args = self.nested(|p| {
            let mut args = vec![p.or()?];
            while p.peek() == &Tok::Comma {
                p.bump();
                args.push(p.or()?);
            }
            Ok(args)
        })?;
        self.expect(Tok::RParen, "')' closing the call")?;
        if args.len() != func.arity() {
            return Err(self.error_here(format!(
                "{name}() takes {} argument{} here",
                func.arity(),
                if func.arity() == 1 { "" } else { "s" },
            )));
        }
        self.node(Expr::Call { func, args })
    }

    fn interval(&mut self) -> Result<Expr, SyntaxError> {
        let Tok::Str(text) = self.bump() else {
            self.at -= 1;
            return Err(self.error_here("expected a quoted interval such as interval '62 days'"));
        };
        let invalid = || "intervals are written '<n> days|hours|minutes|seconds'".to_string();
        let mut words = text.split_whitespace();
        let (Some(amount), Some(unit), None) = (words.next(), words.next(), words.next()) else {
            self.at -= 1;
            return Err(self.error_here(invalid()));
        };
        let unit = match unit {
            "day" | "days" => IntervalUnit::Days,
            "hour" | "hours" => IntervalUnit::Hours,
            "minute" | "minutes" => IntervalUnit::Minutes,
            "second" | "seconds" => IntervalUnit::Seconds,
            _ => {
                self.at -= 1;
                return Err(self.error_here(invalid()));
            }
        };
        let amount = match amount.parse::<u32>() {
            Ok(n) if amount.bytes().all(|b| b.is_ascii_digit()) && n <= 1_000_000 => n,
            _ => {
                self.at -= 1;
                return Err(self.error_here(invalid()));
            }
        };
        self.node(Expr::Interval { amount, unit })
    }
}
