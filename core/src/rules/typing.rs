//! Type resolution. Every operand is checked against the manifest so that the
//! emitted SQL resolves to one immutable PostgreSQL operator or function, and
//! text literals compared with dates, timestamps or uuids are normalized into
//! unambiguous typed literals.

use std::collections::HashMap;

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};

use super::ast::{BinOp, Expr, Func, LiteralType};
use crate::data_types::FieldType;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Ty {
    Bool,
    Text,
    /// An integer-valued function result such as `length(x)`.
    Int,
    Float,
    Numeric,
    Date,
    Timestamp,
    Interval,
    Json,
    Uuid,
    Array,
    /// A numeric literal: adopts the numeric type it meets.
    NumLit,
    /// A string literal: text, or a date/timestamp/uuid literal by context.
    TextLit,
}

impl Ty {
    fn name(self) -> &'static str {
        match self {
            Ty::Bool => "boolean",
            Ty::Text | Ty::TextLit => "text",
            Ty::Int => "integer",
            Ty::Float => "number",
            Ty::Numeric => "decimal",
            Ty::Date => "date",
            Ty::Timestamp => "timestamp",
            Ty::Interval => "interval",
            Ty::Json => "json",
            Ty::Uuid => "uuid",
            Ty::Array => "array",
            Ty::NumLit => "number literal",
        }
    }

    fn numeric(self) -> bool {
        matches!(self, Ty::Int | Ty::Float | Ty::Numeric | Ty::NumLit)
    }

    fn text(self) -> bool {
        matches!(self, Ty::Text | Ty::TextLit)
    }

    fn orderable(self) -> bool {
        self.numeric() || matches!(self, Ty::Date | Ty::Timestamp | Ty::Interval)
    }

    pub(crate) fn of(field_type: &FieldType) -> Ty {
        match field_type {
            FieldType::Text | FieldType::File => Ty::Text,
            FieldType::Number => Ty::Float,
            FieldType::Decimal(_) => Ty::Numeric,
            FieldType::Boolean => Ty::Bool,
            FieldType::Date => Ty::Date,
            FieldType::Timestamp => Ty::Timestamp,
            FieldType::Json => Ty::Json,
            FieldType::Uuid | FieldType::EntityLink => Ty::Uuid,
            FieldType::TextArray | FieldType::NumberArray => Ty::Array,
        }
    }
}

pub(crate) struct Field {
    pub(crate) ty: Ty,
    pub(crate) sensitive: bool,
    /// The column's PostgreSQL type; part of a rule's tag, so a type change
    /// recompiles every rule that reads the column.
    pub(crate) pg: String,
}

pub(crate) type Fields = HashMap<String, Field>;

pub(crate) struct Checker<'a> {
    pub(crate) fields: &'a Fields,
}

type Result<T> = std::result::Result<T, String>;

impl Checker<'_> {
    /// Resolve a rule that must be boolean; returns the canonical tree.
    pub(crate) fn boolean(&self, expr: &Expr) -> Result<Expr> {
        let (expr, ty) = self.check(expr)?;
        if ty != Ty::Bool {
            return Err(format!("the rule must be a boolean condition, not {}", ty.name()));
        }
        Ok(expr)
    }

    /// Resolve an index key expression: any scalar, never a bare literal.
    pub(crate) fn scalar(&self, expr: &Expr) -> Result<Expr> {
        let (expr, ty) = self.check(expr)?;
        let mut fields = Vec::new();
        expr.fields(&mut fields);
        if fields.is_empty() || matches!(ty, Ty::NumLit | Ty::TextLit) {
            return Err("an index expression must use at least one field".into());
        }
        Ok(expr)
    }

    fn check(&self, expr: &Expr) -> Result<(Expr, Ty)> {
        Ok(match expr {
            Expr::Field { name } => {
                let field = self.fields.get(name).ok_or_else(|| format!("unknown field '{name}'"))?;
                (expr.clone(), field.ty)
            }
            Expr::Text { .. } => (expr.clone(), Ty::TextLit),
            Expr::Number { .. } => (expr.clone(), Ty::NumLit),
            Expr::Bool { .. } => (expr.clone(), Ty::Bool),
            Expr::Interval { .. } => (expr.clone(), Ty::Interval),
            Expr::Typed { ty, .. } => (expr.clone(), match ty {
                LiteralType::Date => Ty::Date,
                LiteralType::Timestamp => Ty::Timestamp,
                LiteralType::Uuid => Ty::Uuid,
            }),
            Expr::Not { e } => {
                let (e, ty) = self.check(e)?;
                expect(ty == Ty::Bool, || format!("NOT needs a boolean, not {}", ty.name()))?;
                (Expr::Not { e: Box::new(e) }, Ty::Bool)
            }
            Expr::Neg { e } => {
                let (e, ty) = self.check(e)?;
                expect(ty.numeric(), || format!("unary '-' needs a number, not {}", ty.name()))?;
                (Expr::Neg { e: Box::new(e) }, ty)
            }
            Expr::IsNull { e, negated } => {
                let (e, _) = self.check(e)?;
                (Expr::IsNull { e: Box::new(e), negated: *negated }, Ty::Bool)
            }
            Expr::Binary { op, l, r } => self.binary(*op, l, r)?,
            Expr::Between { e, lo, hi, negated } => {
                let (e, ety) = self.check(e)?;
                expect(ety.orderable(), || format!("BETWEEN needs numbers, dates, timestamps or intervals, not {}", ety.name()))?;
                let (lo, _) = self.unify(ety, lo, "BETWEEN")?;
                let (hi, _) = self.unify(ety, hi, "BETWEEN")?;
                (Expr::Between { e: Box::new(e), lo: Box::new(lo), hi: Box::new(hi), negated: *negated }, Ty::Bool)
            }
            Expr::In { e, list, negated } => {
                let (e, ety) = self.check(e)?;
                expect(
                    ety.numeric() || ety.text() || matches!(ety, Ty::Date | Ty::Uuid),
                    || format!("IN needs text, numbers, dates or uuids, not {}", ety.name()),
                )?;
                let mut items = Vec::with_capacity(list.len());
                for item in list {
                    expect(is_literal(item), || "IN lists contain literals only".to_string())?;
                    items.push(self.unify(ety, item, "IN")?.0);
                }
                (Expr::In { e: Box::new(e), list: items, negated: *negated }, Ty::Bool)
            }
            Expr::Call { func, args } => self.call(*func, args)?,
            Expr::ArrayWithin { .. } => (expr.clone(), Ty::Bool),
            Expr::Finite { e, numeric } => {
                let (e, _) = self.check(e)?;
                (Expr::Finite { e: Box::new(e), numeric: *numeric }, Ty::Bool)
            }
        })
    }

    /// Check `other` against an already-resolved side of type `ty`, turning a
    /// string literal into a typed literal where the context demands one.
    fn unify(&self, ty: Ty, other: &Expr, what: &str) -> Result<(Expr, Ty)> {
        let (other, oty) = self.check(other)?;
        let (other, oty) = match (ty, oty, &other) {
            (Ty::Date | Ty::Timestamp | Ty::Uuid, Ty::TextLit, Expr::Text { value }) => {
                let typed = typed_literal(ty, value)?;
                (typed, ty)
            }
            _ => (other, oty),
        };
        expect(compatible(ty, oty), || {
            format!("{what} cannot compare {} with {}", ty.name(), oty.name())
        })?;
        Ok((other, oty))
    }

    fn binary(&self, op: BinOp, l: &Expr, r: &Expr) -> Result<(Expr, Ty)> {
        let (l, lty) = self.check(l)?;
        let wrap = |l: Expr, r: Expr| Expr::Binary { op, l: Box::new(l), r: Box::new(r) };
        match op {
            BinOp::And | BinOp::Or => {
                let (r, rty) = self.check(r)?;
                expect(lty == Ty::Bool && rty == Ty::Bool, || format!("{} needs booleans", op.sql()))?;
                Ok((wrap(l, r), Ty::Bool))
            }
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                // A literal on the left takes its type from the right side.
                let (l, r, lty, rty) = if matches!(lty, Ty::TextLit) {
                    let (r, rty) = self.check(r)?;
                    let (l, lty) = self.unify(rty, &l, op.sql())?;
                    (l, r, lty, rty)
                } else {
                    let (r, rty) = self.unify(lty, r, op.sql())?;
                    (l, r, lty, rty)
                };
                if !matches!(op, BinOp::Eq | BinOp::Ne) {
                    expect(lty.orderable() && rty.orderable(), || {
                        if lty.text() || rty.text() {
                            format!("'{}' cannot order text: the result would depend on collation", op.sql())
                        } else {
                            format!("'{}' cannot order {}", op.sql(), lty.name())
                        }
                    })?;
                }
                expect(!matches!(lty, Ty::Json | Ty::Array), || format!("'{}' cannot compare {}", op.sql(), lty.name()))?;
                Ok((wrap(l, r), Ty::Bool))
            }
            BinOp::Match => {
                expect(lty == Ty::Text, || format!("'~' needs a text field, not {}", lty.name()))?;
                let Expr::Text { value } = r else {
                    return Err("'~' needs a quoted pattern on its right".into());
                };
                super::pattern::validate(value)?;
                Ok((wrap(l, r.clone()), Ty::Bool))
            }
            BinOp::Add | BinOp::Sub | BinOp::Mul => {
                let (r, rty) = self.check(r)?;
                let ty = match (op, lty, rty) {
                    (BinOp::Sub, Ty::Timestamp, Ty::Timestamp) => Ty::Interval,
                    (BinOp::Sub, Ty::Date, Ty::Date) => Ty::Int,
                    (_, a, b) if a.numeric() && b.numeric() => arithmetic(a, b),
                    (_, a, b) => return Err(format!("'{}' cannot combine {} and {}", op.sql(), a.name(), b.name())),
                };
                Ok((wrap(l, r), ty))
            }
        }
    }

    fn call(&self, func: Func, args: &[Expr]) -> Result<(Expr, Ty)> {
        let mut resolved = Vec::with_capacity(args.len());
        let mut types = Vec::with_capacity(args.len());
        for arg in args {
            let (arg, ty) = self.check(arg)?;
            resolved.push(arg);
            types.push(ty);
        }
        let name = func.sql_name();
        let wrong = |ty: Ty| format!("{name}() does not accept {}", ty.name());
        let ty = match func {
            Func::Length => { expect(types[0].text(), || wrong(types[0]))?; Ty::Int }
            Func::Btrim | Func::Upper | Func::Lower => { expect(types[0].text(), || wrong(types[0]))?; Ty::Text }
            Func::Scale => { expect(types[0] == Ty::Numeric, || format!("scale() needs a decimal field, not {}", types[0].name()))?; Ty::Int }
            Func::Trunc => {
                expect(matches!(types[0], Ty::Float | Ty::Numeric), || wrong(types[0]))?;
                types[0]
            }
            Func::Abs => { expect(matches!(types[0], Ty::Float | Ty::Numeric | Ty::Int), || wrong(types[0]))?; types[0] }
            Func::JsonbTypeof => { expect(types[0] == Ty::Json, || wrong(types[0]))?; Ty::Text }
            Func::JsonbArrayLength => { expect(types[0] == Ty::Json, || wrong(types[0]))?; Ty::Int }
            Func::Coalesce => {
                let first = types[0];
                expect(!matches!(first, Ty::NumLit | Ty::TextLit | Ty::Json | Ty::Array), || {
                    "coalesce() starts with a text, number, date, timestamp, uuid or boolean field".to_string()
                })?;
                let (fallback, fty) = self.unify(first, &args[1], "coalesce()")?;
                resolved[1] = fallback;
                if first.numeric() { arithmetic(first, fty) } else { first }
            }
        };
        Ok((Expr::Call { func, args: resolved }, ty))
    }
}

fn expect(ok: bool, message: impl FnOnce() -> String) -> Result<()> {
    if ok { Ok(()) } else { Err(message()) }
}

fn is_literal(expr: &Expr) -> bool {
    match expr {
        Expr::Text { .. } | Expr::Number { .. } | Expr::Bool { .. } => true,
        Expr::Neg { e } => matches!(**e, Expr::Number { .. }),
        _ => false,
    }
}

/// PostgreSQL's implicit numeric promotions are all immutable; the result type
/// only decides which later operations are accepted.
fn arithmetic(a: Ty, b: Ty) -> Ty {
    if a == Ty::Float || b == Ty::Float {
        Ty::Float
    } else if a == Ty::Numeric || b == Ty::Numeric {
        Ty::Numeric
    } else if a == Ty::Int || b == Ty::Int {
        Ty::Int
    } else {
        Ty::NumLit
    }
}

/// Same type family, and never two literals: `'a' = 'b'` constrains nothing.
fn compatible(a: Ty, b: Ty) -> bool {
    let same_family = (a.numeric() && b.numeric()) || (a.text() && b.text()) || a == b;
    same_family && !(a == Ty::TextLit && b == Ty::TextLit)
}

/// Only unambiguous spellings: special values such as 'today', 'now', 'epoch'
/// or 'infinity' depend on the moment or are not real instants.
fn typed_literal(ty: Ty, value: &str) -> Result<Expr> {
    let (ty, value) = match ty {
        Ty::Date => {
            let ok = value.len() == 10 && NaiveDate::parse_from_str(value, "%Y-%m-%d").is_ok();
            expect(ok, || format!("'{value}' is not a date; write 'YYYY-MM-DD'"))?;
            (LiteralType::Date, value.to_string())
        }
        Ty::Timestamp => {
            let parsed = DateTime::parse_from_rfc3339(value)
                .map_err(|_| format!("'{value}' is not a timestamp; write RFC 3339 with an offset, like '2026-01-01T00:00:00Z'"))?;
            (LiteralType::Timestamp, parsed.with_timezone(&Utc).to_rfc3339_opts(SecondsFormat::AutoSi, true))
        }
        Ty::Uuid => {
            let parsed = uuid::Uuid::parse_str(value).ok().filter(|_| value.len() == 36)
                .ok_or_else(|| format!("'{value}' is not a canonical uuid"))?;
            (LiteralType::Uuid, parsed.hyphenated().to_string())
        }
        _ => unreachable!("typed literals are dates, timestamps or uuids"),
    };
    Ok(Expr::Typed { ty, value })
}
