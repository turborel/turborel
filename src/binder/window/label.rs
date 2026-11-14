//! \[DESIGN\] The label of a window function, its column's display name
//! and how EXPLAIN shows it: its function and arguments as they are
//! written, over the columns of the window's input.

use crate::logical::{Pick, WindowCall, WindowKind, render_expr};

/// `call`, its label its function and arguments over the columns `names`.
pub(super) fn labelled(call: WindowCall, names: &[String]) -> WindowCall {
    let shown = |e| render_expr(e, names);
    let label = match &call.kind {
        WindowKind::RowNumber => "ROW_NUMBER()".to_owned(),
        WindowKind::Rank => "RANK()".to_owned(),
        WindowKind::DenseRank => "DENSE_RANK()".to_owned(),
        WindowKind::PercentRank => "PERCENT_RANK()".to_owned(),
        WindowKind::CumeDist => "CUME_DIST()".to_owned(),
        WindowKind::Aggregate(a) => match &a.arg {
            None => format!("{}(*)", a.func.0.name()),
            Some(e) => {
                let distinct = if a.distinct { "DISTINCT " } else { "" };
                format!("{}({distinct}{}{})", a.func.0.name(), shown(e), a.func.0.label_suffix())
            }
        },
        WindowKind::Ntile(tiles) => format!("NTILE({})", shown(tiles)),
        WindowKind::Offset(o) => {
            let name = if o.lead { "LEAD" } else { "LAG" };
            format!("{name}({}, {}, {}){}", shown(&o.extent), o.offset, shown(&o.default), treated(o.ignore_nulls))
        }
        WindowKind::Value(v) => {
            let arg = shown(&v.arg);
            let called = match &v.pick {
                Pick::First => format!("FIRST_VALUE({arg})"),
                Pick::Last => format!("LAST_VALUE({arg})"),
                Pick::Nth { row, from_last } => {
                    format!("NTH_VALUE({arg}, {}){}", shown(row), if *from_last { " FROM LAST" } else { "" })
                }
            };
            format!("{called}{}", treated(v.ignore_nulls))
        }
    };
    WindowCall { label, ..call }
}

/// The `<window function null treatment>` shown: IGNORE NULLS, or nothing
/// of RESPECT NULLS, written or implied.
fn treated(ignore_nulls: bool) -> &'static str {
    if ignore_nulls { " IGNORE NULLS" } else { "" }
}
