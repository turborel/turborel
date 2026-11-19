//! \[DESIGN\] A selector applied to the paths of one start as a search
//! of paths finds them [GS §7.2] [P1 §5.1]: per end, the partition of the
//! start and that end, the paths it keeps of those offered in order of
//! length or cost, and whether it is complete, so a search whose ends
//! are given may stop.

use std::collections::HashSet;

use super::ranked::Cost;
use super::states::Context;
use crate::error::Result;
use crate::graph::binding::PathBinding;
use crate::graph::limits::Meter;
use crate::graph::program::Selector;
use crate::graph::view::ElementId;

/// One partition: its end, the paths kept, and their measures, each once.
struct Part {
    end: ElementId,
    kept: Vec<PathBinding>,
    measures: Vec<Cost>,
}

/// The partitions of one start.
pub(super) struct Selection {
    selector: Selector,
    k: usize,
    parts: Vec<Part>,
}

impl Selection {
    /// The selection of `selector`.
    pub(super) fn new(selector: Selector) -> Selection {
        Selection { selector, k: usize::try_from(selector.k()).unwrap_or(usize::MAX), parts: Vec::new() }
    }

    /// Whether `part` keeps a path of `measure`, offered no earlier than
    /// those it holds.
    fn keeps(&self, part: &Part, measure: Cost) -> bool {
        match self.selector {
            Selector::All => true,
            Selector::Any(_) | Selector::Shortest(_) | Selector::Cheapest(_) => part.kept.len() < self.k,
            Selector::AnyShortest | Selector::AnyCheapest => part.kept.is_empty(),
            Selector::AllShortest | Selector::AllCheapest => part.measures.first().is_none_or(|&m| m == measure),
            Selector::ShortestGroups(_) => part.measures.contains(&measure) || part.measures.len() < self.k,
        }
    }

    /// Whether `part` keeps no path offered at `measure` or later.
    fn complete(&self, part: &Part, measure: Cost) -> bool {
        match self.selector {
            Selector::All => false,
            Selector::Any(_) | Selector::Shortest(_) | Selector::Cheapest(_) => part.kept.len() >= self.k,
            Selector::AnyShortest | Selector::AnyCheapest => !part.kept.is_empty(),
            Selector::AllShortest | Selector::AllCheapest => part.measures.first().is_some_and(|&m| m < measure),
            Selector::ShortestGroups(_) => part.measures.len() >= self.k && part.measures.last().is_some_and(|&m| m < measure),
        }
    }

    /// Offers `binding`, of `measure`, to its partition.
    ///
    /// # Errors
    ///
    /// GRF-0061 past MAX_PATHS_PER_PARTITION ON LIMIT ERROR.
    pub(super) fn offer(&mut self, binding: PathBinding, measure: f64, meter: &mut Meter) -> Result<bool> {
        let Some(end) = binding.end() else { return Ok(true) };
        let at = self.parts.iter().position(|p| p.end == end).unwrap_or_else(|| {
            self.parts.push(Part { end, kept: Vec::new(), measures: Vec::new() });
            self.parts.len().saturating_sub(1)
        });
        let measure = Cost(measure);
        let keeps = self.parts.get(at).is_some_and(|part| self.keeps(part, measure));
        let Some(part) = self.parts.get_mut(at).filter(|_| keeps) else { return Ok(true) };
        if part.measures.last() != Some(&measure) {
            part.measures.push(measure);
        }
        part.kept.push(binding);
        meter.paths(part.kept.len())
    }

    /// Whether every end given has a complete partition, paths now
    /// being offered at `measure`.
    pub(super) fn closed(&self, ctx: &Context<'_>, measure: f64) -> bool {
        let done: HashSet<ElementId> = self.parts.iter().filter(|p| self.complete(p, Cost(measure))).map(|p| p.end).collect();
        ctx.all_found(&done)
    }

    /// The paths kept, partition by partition.
    pub(super) fn bindings(self) -> Vec<PathBinding> {
        self.parts.into_iter().flat_map(|p| p.kept).collect()
    }
}
