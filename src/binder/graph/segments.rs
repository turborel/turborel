//! \[EXTENSION\] FOR EACH SEGMENT (e1, e2) IN p WHERE `<condition>` [GS
//! §11.1]: a postfilter of every two edges one after the other in path
//! p, the edges bound to two element variables of its own, which the
//! condition reads as it reads any other.

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
