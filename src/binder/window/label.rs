//! \[DESIGN\] The label of a window function, its column's display name
//! and how EXPLAIN shows it: its function and arguments as they are
//! written, over the columns of the window's input.

use crate::logical::{Nested, Pick, WindowCall, WindowKind, render_expr};

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

/// The columns `names` of a window's input, then those of the nested
/// window functions `nested` of a call, each shown as it is written over
/// the names before it [ISO/IEC 9075-2:2023 §6.11].
pub(super) fn nested_names(nested: &[Nested], names: &[String]) -> Vec<String> {
    let mut all = names.to_vec();
    for n in nested {
        let delta = match &n.delta {
            None => String::new(),
            Some((minus, offset)) => format!(" {} {}", if *minus { "-" } else { "+" }, render_expr(offset, &all)),
        };
        let shown = match &n.value {
            None => format!("ROW_NUMBER({}{delta})", n.marker.render()),
            Some((value, default)) => {
                format!("VALUE_OF({} AT {}{delta}, {})", render_expr(value, &all), n.marker.render(), render_expr(default, &all))
            }
        };
        all.push(shown);
    }
    all
}
