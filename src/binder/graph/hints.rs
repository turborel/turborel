//! \[EXTENSION\] [GS §16 R16.8] The hints of a GRAPH_TABLE, checked: each
//! names a variable of its pattern or an element table of its graph, as
//! the hint asks.

use super::grf;
use crate::ast::GraphHints;
use crate::error::Result;
use crate::logical::{ElementSource, VarShape};

/// `hints`, each name checked against the variables `vars` and the
/// element tables `tables`.
///
/// # Errors
///
/// SQLCODE -103 (GRF-0070) of a hint naming no variable of the pattern,
/// GRAPH_DIRECTION of a vertex variable, or USE_ADJACENCY of no edge table
/// with an adjacency index.
pub(super) fn checked(hints: &GraphHints, vars: &[VarShape], tables: &[ElementSource]) -> Result<GraphHints> {
    let var = |(name, at): &(String, usize)| -> Result<&VarShape> {
        vars.iter()
            .find(|v| v.name.eq_ignore_ascii_case(name))
            .ok_or_else(|| grf("GRF-0070", format!("a hint names {name}, no variable of the pattern (R16.8)")).or_at(*at))
    };
    if let Some(start) = &hints.start {
        var(start)?;
    }
    for reverse in &hints.reverse {
        if !var(reverse)?.edge {
            return Err(grf("GRF-0070", format!("GRAPH_DIRECTION names {}, no edge variable (R16.8)", reverse.0)).or_at(reverse.1));
        }
    }
    for (alias, at) in &hints.use_adjacency {
        let indexed = tables.iter().any(|t| t.alias.eq_ignore_ascii_case(alias) && t.ends.is_some() && t.adjacency.is_some());
        if !indexed {
            return Err(grf("GRF-0070", format!("USE_ADJACENCY names {alias}, no edge table with an adjacency index (R16.8)")).or_at(*at));
        }
    }
    Ok(hints.clone())
}
