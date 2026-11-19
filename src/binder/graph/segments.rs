//! \[EXTENSION\] FOR EACH SEGMENT (e1, e2) IN p WHERE `<condition>` [GS
//! §11.1]: a postfilter of every two edges one after the other in path
//! p, the edges bound to two element variables of its own, which the
//! condition reads as it reads any other. And the order path patterns
//! whose prefilters read one another's variables are searched in.

use super::super::{Binder, Scope, rule};
use super::scope::Graphing;
use crate::ast::GraphPattern;
use crate::error::Result;
use crate::logical::SegmentShape;

/// The edge variables of the segment filters of `pattern`: name, an
/// edge's, and where each is written.
pub(super) fn segment_variables(pattern: &GraphPattern) -> Vec<(String, bool, usize)> {
    pattern.segments.iter().flat_map(|s| [&s.first, &s.second]).map(|v| (v.name.clone(), true, v.at)).collect()
}

/// The segment filters of `pattern`, their conditions bound by `measure`
/// in `scope`, their variables numbered of `names`.
///
/// # Errors
///
/// SQLCODE -103 if a condition breaks a rule, or its path is no path or
/// subpath variable.
pub(super) fn segments(
    measure: &Binder<'_>,
    scope: Scope<'_>,
    (pattern, names, graphing): (&GraphPattern, &[String], &Graphing),
) -> Result<Vec<SegmentShape>> {
    let mut out = Vec::new();
    for s in &pattern.segments {
        let name = &s.path.name;
        let whole = graphing.paths.iter().position(|p| p.as_deref() == Some(name.as_str())).map(|p| (p, None));
        let part = graphing.subpaths.borrow().iter().find(|p| &p.0 == name).map(|p| (p.1, Some((p.2, p.3))));
        let (path, span) =
            whole.or(part).ok_or_else(|| rule(format!("[GS §11.1] {name} of FOR EACH SEGMENT is no path variable")).or_at(s.path.at))?;
        let var = |v: &crate::ast::GraphVariable| names.iter().position(|n| n == &v.name).unwrap_or(usize::MAX);
        let cond = measure.search_condition(&s.condition, scope)?;
        out.push(SegmentShape { path, span, first: var(&s.first), second: var(&s.second), cond });
    }
    Ok(out)
}

/// \[GS §6.8\] The path patterns of `g` whose prefilters read another's
/// variables, each searched after those it reads (RULES.md D108).
///
/// # Errors
///
/// SQLCODE -103 if two read each other's, directly or through others,
/// as then neither is bound where the other's prefilter is tested,
/// before its selector.
pub(super) fn acyclic(g: &crate::logical::GraphMatch) -> Result<()> {
    let count = g.paths.len();
    let depends: Vec<Vec<usize>> = (0..count).map(|n| g.depends(n)).collect();
    for n in 0..count {
        let (mut reached, mut todo) = (Vec::new(), depends.get(n).cloned().unwrap_or_default());
        while let Some(m) = todo.pop() {
            if m == n {
                let msg = format!(
                    "[GS §6.8] the prefilters of path pattern {} read a variable of a path pattern whose prefilters read one of it, so neither is bound before the other's selector",
                    n.saturating_add(1)
                );
                return Err(rule(msg));
            }
            if !reached.contains(&m) {
                reached.push(m);
                todo.extend(depends.get(m).cloned().unwrap_or_default());
            }
        }
    }
    Ok(())
}
