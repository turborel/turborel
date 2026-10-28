//! The logical plan as text: expressions, conditions and sort keys, for
//! EXPLAIN and error messages. Split from logical.rs (RULES.md R-SIZE-2).

use std::fmt::Write;

use super::{Cond, Expr, Quantifier, SortKey};
use crate::catalog::MatchType;

/// The display name of column `i`, or `#i` if `names` has none for it.
pub fn column_name(names: &[String], i: usize) -> String {
    names.get(i).cloned().unwrap_or_else(|| format!("#{i}"))
}

/// `e` as text, with columns named from `names`.
pub fn render_expr(e: &Expr, names: &[String]) -> String {
    match e {
        Expr::Column(i) => column_name(names, *i),
        Expr::Const(v) => v.to_literal(),
        Expr::Neg(x) => format!("-{}", render_expr(x, names)),
        Expr::Arith(a, op, b) => format!("({} {} {})", render_expr(a, names), op.symbol(), render_expr(b, names)),
        Expr::Outer(_, name) => name.clone(),
        Expr::Call(f, args) => {
            format!("{}({})", f.0.name(), args.iter().map(|a| render_expr(a, names)).collect::<Vec<_>>().join(", "))
        }
        Expr::Case(whens, otherwise) => {
            let mut text = String::from("CASE");
            for (c, result) in whens {
                // Writing to a String cannot fail.
                let _ = write!(text, " WHEN {} THEN {}", render_cond(c, names), render_expr(result, names));
            }
            let _ = write!(text, " ELSE {} END", render_expr(otherwise, names));
            text
        }
        Expr::Cast(x, t) => format!("CAST({} AS {t})", render_expr(x, names)),
        Expr::Domain(x, d) => {
            // The CAST to the domain's data type it holds is shown as to the domain.
            let operand = if let Expr::Cast(operand, _) = &**x { operand } else { x };
            format!("CAST({} AS {})", render_expr(operand, names), d.domain)
        }
        Expr::Subquery(_, column) => format!("(<subquery>).{}", column.saturating_add(1)),
    }
}

/// A row of values as text: one alone, or in parentheses.
fn render_row(values: &[Expr], names: &[String]) -> String {
    match values {
        [one] => render_expr(one, names),
        _ => format!("({})", values.iter().map(|e| render_expr(e, names)).collect::<Vec<_>>().join(", ")),
    }
}

/// `c` as text, with columns named from `names`.
pub fn render_cond(c: &Cond, names: &[String]) -> String {
    match c {
        Cond::Const(t) => format!("{t:?}").to_uppercase(),
        Cond::Or(a, b) => format!("({} OR {})", render_cond(a, names), render_cond(b, names)),
        Cond::And(a, b) => format!("{} AND {}", render_cond(a, names), render_cond(b, names)),
        Cond::Not(a) => format!("NOT ({})", render_cond(a, names)),
        Cond::Compare(a, op, b) => format!("{} {} {}", render_expr(a, names), op.symbol(), render_expr(b, names)),
        Cond::IsNull(e, not) => format!("{} IS {}NULL", render_expr(e, names), if *not { "NOT " } else { "" }),
        Cond::Exists(_) => "EXISTS (<subquery>)".to_string(),
        Cond::Quantified(values, op, q, _) => {
            let quantifier = match q {
                Quantifier::All => " ALL",
                Quantifier::Some => " ANY",
                Quantifier::One => "",
            };
            format!("{} {}{quantifier} (<subquery>)", render_row(values, names), op.symbol())
        }
        Cond::Is(a, t, not) => {
            format!("({}) IS {}{}", render_cond(a, names), if *not { "NOT " } else { "" }, format!("{t:?}").to_uppercase())
        }
        Cond::Unique(_) => "UNIQUE (<subquery>)".to_string(),
        Cond::Match { values, unique, match_type, .. } => {
            let kind = match match_type {
                MatchType::Simple => "",
                MatchType::Partial => " PARTIAL",
                MatchType::Full => " FULL",
            };
            format!("{} MATCH{}{kind} (<subquery>)", render_row(values, names), if *unique { " UNIQUE" } else { "" })
        }
        Cond::Like(e, pattern, escape) => format!(
            "{} LIKE {}{}",
            render_expr(e, names),
            render_expr(pattern, names),
            escape.as_ref().map(|x| format!(" ESCAPE {}", render_expr(x, names))).unwrap_or_default()
        ),
    }
}

/// Sort keys as text, with columns named from `names`.
pub fn render_keys(keys: &[SortKey], names: &[String]) -> String {
    keys.iter()
        .map(|k| format!("{}{}", column_name(names, k.column), if k.descending { " DESC" } else { "" }))
        .collect::<Vec<_>>()
        .join(", ")
}
