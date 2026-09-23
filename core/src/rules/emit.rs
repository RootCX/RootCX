//! SQL emission from a resolved rule tree. Every node is parenthesised, every
//! identifier quoted, every function `pg_catalog`-qualified, and every literal
//! quoted by Core, so the output's meaning never depends on precedence, search
//! path or the author's spelling.

use super::ast::{Expr, Func, LiteralType};
use crate::manifest::{quote_ident, quote_literal};

pub(crate) fn sql(expr: &Expr) -> String {
    match expr {
        Expr::Field { name } => quote_ident(name),
        Expr::Text { value } => quote_literal(value),
        Expr::Number { value } => value.clone(),
        Expr::Bool { value } => if *value { "TRUE" } else { "FALSE" }.into(),
        Expr::Typed { ty, value } => {
            let ty = match ty {
                LiteralType::Date => "date",
                LiteralType::Timestamp => "timestamptz",
                LiteralType::Uuid => "uuid",
            };
            format!("{}::pg_catalog.{ty}", quote_literal(value))
        }
        Expr::Interval { amount, unit } => {
            format!("'{amount} {}'::pg_catalog.interval", unit.as_str())
        }
        Expr::Not { e } => format!("(NOT {})", sql(e)),
        Expr::Neg { e } => format!("(- {})", sql(e)),
        Expr::Binary { op, l, r } => format!("({} {} {})", sql(l), op.sql(), sql(r)),
        Expr::IsNull { e, negated } => {
            format!("({} IS {}NULL)", sql(e), if *negated { "NOT " } else { "" })
        }
        Expr::Between { e, lo, hi, negated } => format!(
            "({} {}BETWEEN {} AND {})",
            sql(e), if *negated { "NOT " } else { "" }, sql(lo), sql(hi),
        ),
        Expr::In { e, list, negated } => format!(
            "({} {}IN ({}))",
            sql(e),
            if *negated { "NOT " } else { "" },
            list.iter().map(sql).collect::<Vec<_>>().join(", "),
        ),
        Expr::Call { func: Func::Coalesce, args } => {
            format!("COALESCE({}, {})", sql(&args[0]), sql(&args[1]))
        }
        // Guarded so a rule can never raise on a non-array value: the error
        // would abort the write with a message instead of a rule violation.
        Expr::Call { func: Func::JsonbArrayLength, args } => {
            let arg = sql(&args[0]);
            format!(
                "(CASE WHEN pg_catalog.jsonb_typeof({arg}) = 'array' THEN pg_catalog.jsonb_array_length({arg}) END)"
            )
        }
        Expr::Call { func, args } => format!(
            "pg_catalog.{}({})",
            func.sql_name(),
            args.iter().map(sql).collect::<Vec<_>>().join(", "),
        ),
        Expr::ArrayWithin { field, values } => format!(
            "({} <@ ARRAY[{}]::pg_catalog.text[])",
            quote_ident(field),
            values.iter().map(|v| quote_literal(v)).collect::<Vec<_>>().join(", "),
        ),
        Expr::Finite { e, numeric } => {
            let ty = if *numeric { "numeric" } else { "float8" };
            let e = sql(e);
            format!("({e} < 'Infinity'::pg_catalog.{ty} AND {e} > '-Infinity'::pg_catalog.{ty})")
        }
    }
}
