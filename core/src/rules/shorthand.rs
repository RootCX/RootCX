//! Field shorthands: common single-field rules declared on the field and
//! compiled through the same typed tree as written checks.

use rootcx_types::FieldContract;
use serde_json::Value as JsonValue;

use super::ast::{BinOp, Expr, Func};
use super::typing::{Checker, Ty};

/// Kova's email rule: something, `@`, something, a dot, something; no spaces.
pub(crate) const EMAIL_PATTERN: &str = r"^[^[:space:]@]+@[^[:space:]@]+\.[^[:space:]@]+$";
const JSON_TYPES: &[&str] = &["object", "array", "string", "number", "boolean", "null"];

/// One compiled shorthand: its name suffix and its resolved tree.
pub(crate) struct Shorthand {
    pub(crate) kind: &'static str,
    pub(crate) tree: Expr,
}

pub(crate) fn compile(field: &FieldContract, ty: Ty, checker: &Checker) -> Result<Vec<Shorthand>, String> {
    let rules = &field.rules;
    let name = &field.name;
    let this = || Expr::Field { name: name.clone() };
    let bin = |op, l: Expr, r: Expr| Expr::Binary { op, l: Box::new(l), r: Box::new(r) };
    let call = |func, arg: Expr| Expr::Call { func, args: vec![arg] };
    let finite = || Expr::Finite { e: Box::new(this()), numeric: ty == Ty::Numeric };
    let number = |n: u64| Expr::Number { value: n.to_string() };
    let needs = |ok: bool, key: &str, what: &str| {
        if ok { Ok(()) } else { Err(format!("field '{name}': '{key}' applies to {what} fields")) }
    };
    let numeric = matches!(ty, Ty::Float | Ty::Numeric);

    let mut out = Vec::new();
    let mut push = |kind, tree: Expr| -> Result<(), String> {
        let tree = checker.boolean(&tree).map_err(|e| format!("field '{name}': {e}"))?;
        out.push(Shorthand { kind, tree });
        Ok(())
    };

    for (key, kind, op, value) in [
        ("minimum", "min", BinOp::Ge, &rules.minimum),
        ("maximum", "max", BinOp::Le, &rules.maximum),
        ("exclusive_minimum", "xmin", BinOp::Gt, &rules.exclusive_minimum),
        ("exclusive_maximum", "xmax", BinOp::Lt, &rules.exclusive_maximum),
    ] {
        let Some(value) = value else { continue };
        needs(numeric, key, "number and decimal")?;
        let bound = bound(name, key, value, ty)?;
        push(kind, bin(BinOp::And, bin(op, this(), bound), finite()))?;
    }
    if rules.integer {
        needs(numeric, "integer", "number and decimal")?;
        let whole = bin(BinOp::Eq, this(), call(Func::Trunc, this()));
        push("int", bin(BinOp::And, whole, finite()))?;
    }
    if let Some(scale) = rules.max_scale {
        needs(ty == Ty::Numeric, "max_scale", "decimal")?;
        let bounded = bin(BinOp::Le, call(Func::Scale, this()), number(scale.into()));
        push("scale", bin(BinOp::And, bounded, finite()))?;
    }
    let text = ty == Ty::Text;
    if let (Some(min), Some(max)) = (rules.min_length, rules.max_length) {
        if min > max {
            return Err(format!("field '{name}': min_length exceeds max_length"));
        }
    }
    if let Some(min) = rules.min_length {
        needs(text, "min_length", "text")?;
        push("minlen", bin(BinOp::Ge, call(Func::Length, this()), number(min.into())))?;
    }
    if let Some(max) = rules.max_length {
        needs(text, "max_length", "text")?;
        push("maxlen", bin(BinOp::Le, call(Func::Length, this()), number(max.into())))?;
    }
    if rules.not_blank {
        needs(text, "not_blank", "text")?;
        push("notblank", bin(BinOp::Gt, call(Func::Length, call(Func::Btrim, this())), number(0)))?;
    }
    if let Some(format) = &rules.format {
        needs(text, "format", "text")?;
        if format != "email" {
            return Err(format!("field '{name}': unknown format '{format}'; supported: email"));
        }
        push("format", bin(BinOp::Match, this(), Expr::Text { value: EMAIL_PATTERN.into() }))?;
    }
    if let Some(pattern) = &rules.pattern {
        needs(text, "pattern", "text")?;
        push("pattern", bin(BinOp::Match, this(), Expr::Text { value: pattern.clone() }))?;
    }
    if let Some(json_type) = &rules.json_type {
        needs(ty == Ty::Json, "json_type", "json")?;
        if !JSON_TYPES.contains(&json_type.as_str()) {
            return Err(format!("field '{name}': json_type must be one of {}", JSON_TYPES.join(", ")));
        }
        push("jsontype", bin(BinOp::Eq, call(Func::JsonbTypeof, this()), Expr::Text { value: json_type.clone() }))?;
    }
    if let Some(max) = rules.max_items {
        needs(ty == Ty::Json, "max_items", "json")?;
        push("maxitems", bin(BinOp::Le, call(Func::JsonbArrayLength, this()), number(max.into())))?;
    }
    Ok(out)
}

/// A bound as a literal of the field's type. Decimal bounds are JSON strings,
/// like every decimal value the Core accepts, so no precision is lost.
fn bound(name: &str, key: &str, value: &JsonValue, ty: Ty) -> Result<Expr, String> {
    let text = match (ty, value) {
        (Ty::Numeric, JsonValue::String(text)) => text.trim().to_string(),
        (Ty::Numeric, _) => return Err(format!("field '{name}': decimal '{key}' must be a JSON string such as \"0.01\"")),
        (_, JsonValue::Number(number)) => match number.as_i64() {
            Some(integer) => integer.to_string(),
            None => number.as_f64().map(|float| float.to_string()).unwrap_or_default(),
        },
        _ => return Err(format!("field '{name}': number '{key}' must be a JSON number")),
    };
    let (negative, digits) = match text.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, text.as_str()),
    };
    let valid = {
        let mut parts = digits.splitn(2, '.');
        let whole = parts.next().unwrap_or("");
        let fraction = parts.next();
        !whole.is_empty()
            && whole.bytes().all(|b| b.is_ascii_digit())
            && fraction.is_none_or(|f| !f.is_empty() && f.bytes().all(|b| b.is_ascii_digit()))
            && digits.len() <= 40
    };
    if !valid {
        return Err(format!("field '{name}': '{key}' must be a finite number written in digits, not '{text}'"));
    }
    let literal = Expr::Number { value: digits.to_string() };
    Ok(if negative { Expr::Neg { e: Box::new(literal) } } else { literal })
}
