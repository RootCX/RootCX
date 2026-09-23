//! The rule syntax tree. It is the only thing Core compiles to SQL, and its
//! canonical serialization (after type resolution) is what a rule's tag hashes.

use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "k", rename_all = "snake_case")]
pub(crate) enum Expr {
    Field { name: String },
    /// A string literal. After type resolution it is text; a literal compared
    /// with a date, timestamp or uuid becomes [`Expr::Typed`].
    Text { value: String },
    /// Digits with an optional fraction, exactly as written.
    Number { value: String },
    Bool { value: bool },
    /// A text literal resolved against a date, timestamp or uuid, normalized.
    Typed { ty: LiteralType, value: String },
    Interval { amount: u32, unit: IntervalUnit },
    Not { e: Box<Expr> },
    Neg { e: Box<Expr> },
    Binary { op: BinOp, l: Box<Expr>, r: Box<Expr> },
    IsNull { e: Box<Expr>, negated: bool },
    Between { e: Box<Expr>, lo: Box<Expr>, hi: Box<Expr>, negated: bool },
    In { e: Box<Expr>, list: Vec<Expr>, negated: bool },
    Call { func: Func, args: Vec<Expr> },
    /// Core-generated only: every element of a text array field is in the set.
    ArrayWithin { field: String, values: Vec<String> },
    /// Core-generated only: excludes NaN and ±Infinity, which PostgreSQL orders
    /// above every number and which `scale()` maps to NULL.
    Finite { e: Box<Expr>, numeric: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum LiteralType {
    Date,
    Timestamp,
    Uuid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum IntervalUnit {
    Days,
    Hours,
    Minutes,
    Seconds,
}

impl IntervalUnit {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Days => "days",
            Self::Hours => "hours",
            Self::Minutes => "minutes",
            Self::Seconds => "seconds",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Match,
    Add,
    Sub,
    Mul,
}

impl BinOp {
    pub(crate) fn sql(self) -> &'static str {
        match self {
            Self::Or => "OR",
            Self::And => "AND",
            Self::Eq => "=",
            Self::Ne => "<>",
            Self::Lt => "<",
            Self::Le => "<=",
            Self::Gt => ">",
            Self::Ge => ">=",
            Self::Match => "~",
            Self::Add => "+",
            Self::Sub => "-",
            Self::Mul => "*",
        }
    }

    pub(crate) fn is_comparison(self) -> bool {
        matches!(self, Self::Eq | Self::Ne | Self::Lt | Self::Le | Self::Gt | Self::Ge)
    }
}

/// The allow-list. Every function is `IMMUTABLE` in `pg_proc`; a test asserts
/// it against the real catalog so a PostgreSQL upgrade cannot change that
/// silently.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Func {
    Length,
    Btrim,
    Upper,
    Lower,
    Coalesce,
    Scale,
    Trunc,
    Abs,
    JsonbTypeof,
    JsonbArrayLength,
}

impl Func {
    pub(crate) const NAMES: &'static str =
        "length, btrim, trim, upper, lower, coalesce, scale, trunc, abs, jsonb_typeof, jsonb_array_length";

    /// `trim(x)` is PostgreSQL's `TRIM(BOTH FROM x)`, which is `btrim(x)`.
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "length" => Self::Length,
            "btrim" | "trim" => Self::Btrim,
            "upper" => Self::Upper,
            "lower" => Self::Lower,
            "coalesce" => Self::Coalesce,
            "scale" => Self::Scale,
            "trunc" => Self::Trunc,
            "abs" => Self::Abs,
            "jsonb_typeof" => Self::JsonbTypeof,
            "jsonb_array_length" => Self::JsonbArrayLength,
            _ => return None,
        })
    }

    pub(crate) fn sql_name(self) -> &'static str {
        match self {
            Self::Length => "length",
            Self::Btrim => "btrim",
            Self::Upper => "upper",
            Self::Lower => "lower",
            Self::Coalesce => "coalesce",
            Self::Scale => "scale",
            Self::Trunc => "trunc",
            Self::Abs => "abs",
            Self::JsonbTypeof => "jsonb_typeof",
            Self::JsonbArrayLength => "jsonb_array_length",
        }
    }

    pub(crate) fn arity(self) -> usize {
        if self == Self::Coalesce { 2 } else { 1 }
    }
}

impl Expr {
    pub(crate) fn fields<'a>(&'a self, out: &mut Vec<&'a str>) {
        match self {
            Self::Field { name } | Self::ArrayWithin { field: name, .. } => {
                if !out.contains(&name.as_str()) {
                    out.push(name);
                }
            }
            Self::Text { .. } | Self::Number { .. } | Self::Bool { .. } | Self::Typed { .. }
            | Self::Interval { .. } => {}
            Self::Not { e } | Self::Neg { e } | Self::IsNull { e, .. } | Self::Finite { e, .. } => e.fields(out),
            Self::Binary { l, r, .. } => {
                l.fields(out);
                r.fields(out);
            }
            Self::Between { e, lo, hi, .. } => {
                e.fields(out);
                lo.fields(out);
                hi.fields(out);
            }
            Self::In { e, list, .. } => {
                e.fields(out);
                list.iter().for_each(|item| item.fields(out));
            }
            Self::Call { args, .. } => args.iter().for_each(|arg| arg.fields(out)),
        }
    }
}
