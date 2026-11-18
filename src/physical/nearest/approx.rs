//! \[EXTENSION\] Planning an approximate index's nearest read (the vector
//! design document R9.2, R9.4, R13.1, R13.2, R13.4, R13.5; RULES.md
//! D94). An approximate index is a candidate only when the FETCH
//! FIRST asks for one, FETCH APPROX or, of neither APPROX nor EXACT, the
//! session's VECTOR.APPROX_DEFAULT ON; and only for a call of its own
//! metric. Its WHERE condition is applied before it (pre-filter: the
//! ordinary plan, no approximate index), as it searches (in-filter) or
//! after it (post-filter), by the condition's estimated selectivity
//! unless a hint names the strategy.

use super::super::PhysicalPlanner;
use crate::catalog::{IndexDesc, QualifiedName};
use crate::index::{Approximation, FilterHint, NearestQuery};
use crate::logical::{Cond, Hints, Search, SearchTarget, Strategy};
use crate::settings::{VECTOR_APPROX_DEFAULT, VECTOR_APPROX_FALLBACK, VECTOR_BEAM_WIDTH, VECTOR_SEARCH_LIST};
use crate::storage::cost_of;

/// [R13.2] Below this estimated share of the rows, the condition is
/// applied first, by the ordinary plan: pre-filter.
pub const PRE_FILTER_BELOW: f64 = 0.01;
/// [R13.2] Above this estimated share, the index searches unfiltered for
/// more rows than asked and the condition is applied after: post-filter.
pub const POST_FILTER_ABOVE: f64 = 0.5;

/// \[EXTENSION\] What a nearest read asks of an approximate index.
#[derive(Debug, Clone, PartialEq)]
pub struct ApproximateRead {
    /// The call's metric, the index's too.
    pub metric: String,
    /// The accuracy and parameters asked: the query's, else the session's
    /// VECTOR.SEARCH_LIST and VECTOR.BEAM_WIDTH, else none, the index's
    /// own (R9.4).
    pub target: SearchTarget,
    /// In-filter or post-filter: a pre-filter reads no approximate index.
    pub strategy: Strategy,
    /// The share of the rows the condition is estimated to keep.
    pub selectivity: f64,
    /// VECTOR.APPROX_FALLBACK: whether a search that finds too few rows,
    /// or whose index declines, reads the table exactly, or is refused.
    pub fallback: bool,
}

impl ApproximateRead {
    /// It as EXPLAIN shows it (R13.5).
    pub fn render(&self) -> String {
        let shown = |what: &str, n: Option<u32>| n.map_or_else(|| format!("{what}=index"), |n| format!("{what}={n}"));
        let accuracy = self.target.accuracy.map_or_else(|| "index".to_owned(), |a| format!("{a}%"));
        format!(
            "approximate {} {}, target accuracy={accuracy}, {}, {}, {}, rerank=every candidate by exact distance",
            self.metric,
            self.strategy.word(),
            shown("search list", self.target.search_list),
            shown("beam width", self.target.beam_width),
            shown("probes", self.target.probes),
        )
    }

    /// The request of a search of `first` rows, `widen` times wider than
    /// asked, of rows no farther than `radius` if a range is asked.
    pub fn approximation(&self, first: usize, widen: u32, radius: Option<f64>) -> Approximation {
        let filter = match self.strategy {
            Strategy::Pre | Strategy::In => FilterHint::InFilter,
            Strategy::Post => FilterHint::PostFilter(self.selectivity),
        };
        Approximation { widen, radius, filter, ..probe(self.target, first) }
    }
}

/// The request `target` makes of a search of `first` rows, as the
/// planner asks a method whether it serves it.
pub(super) fn probe(target: SearchTarget, first: usize) -> Approximation {
    Approximation {
        first,
        search_list: target.search_list,
        beam_width: target.beam_width,
        probes: target.probes,
        accuracy: target.accuracy,
        widen: 1,
        radius: None,
        filter: FilterHint::None,
    }
}

/// Of the approximate index `current` found before and `candidate`, each
/// with the position of the column in its key, the one a read takes: one
/// that serves the call's metric (`served`, `IndexMethod::serves_metric`),
/// the index a hint names first (R13.4), else the first found.
pub(super) fn preferred(
    current: Option<(IndexDesc, usize)>,
    (candidate, served): ((IndexDesc, usize), bool),
    asked: Option<SearchTarget>,
    hints: Option<&Hints>,
) -> Option<(IndexDesc, usize)> {
    if asked.is_none() || !served {
        return current;
    }
    let named = hints.and_then(|h| h.index_of(&candidate.0.table.ident)).is_some_and(|i| i.eq_ignore_ascii_case(&candidate.0.name));
    if named || current.is_none() { Some(candidate) } else { current }
}

impl PhysicalPlanner<'_> {
    /// The hints of the FETCH FIRST planned, if any.
    pub(super) fn hints(&self) -> Option<&Hints> {
        self.fetch.map(|f| f.hints.as_ref())
    }

    /// [R13.1] The target of an approximate search of `table`, if the
    /// query asks for one and no hint bars the table's indexes.
    pub(super) fn approximation(&self, table: &QualifiedName) -> Option<SearchTarget> {
        let fetch = self.fetch?;
        if fetch.hints.bars(&table.ident) {
            return None;
        }
        let mut target = match &fetch.search {
            Search::Exact => return None,
            Search::Approximate(target) => *target,
            Search::Unstated => {
                if !self.db.settings.is_on(VECTOR_APPROX_DEFAULT) {
                    return None;
                }
                SearchTarget::default()
            }
        };
        let setting = |key| u32::try_from(self.db.settings.count(key)).ok().filter(|&n| n > 0);
        target.search_list = target.search_list.or_else(|| setting(VECTOR_SEARCH_LIST));
        target.beam_width = target.beam_width.or_else(|| setting(VECTOR_BEAM_WIDTH));
        Some(target)
    }

    /// [R13.2] The read of an approximate index for `query` over `table`
    /// filtered by `condition`: none of a pre-filter, whose condition the
    /// ordinary plan applies first.
    pub(super) fn approximate_read(
        &self,
        target: SearchTarget,
        (table, condition): (&QualifiedName, Option<&Cond>),
        query: &NearestQuery,
    ) -> Option<ApproximateRead> {
        let selectivity = self.selectivity(table, condition).clamp(f64::MIN_POSITIVE, 1.0_f64);
        let hinted = self.hints().and_then(|h| h.filter);
        let strategy = hinted.unwrap_or(if condition.is_none() || selectivity > POST_FILTER_ABOVE {
            Strategy::Post
        } else if selectivity < PRE_FILTER_BELOW {
            Strategy::Pre
        } else {
            Strategy::In
        });
        if strategy == Strategy::Pre {
            return None;
        }
        let fallback = self.db.settings.is_on(VECTOR_APPROX_FALLBACK);
        Some(ApproximateRead { metric: query.metric.clone()?, target, strategy, selectivity, fallback })
    }

    /// The cost of an approximate read of `first` rows: a descent, then a
    /// fetch of each candidate of its search list, or of as many as the
    /// condition keeps `first` of.
    pub(super) fn approximate_cost(&self, table: &QualifiedName, read: &ApproximateRead, first: usize) -> Option<f64> {
        let storage = self.db.storage_of(table).ok()?;
        let model = self.db.cost();
        let all = cost_of(storage.row_count());
        let list = read.target.search_list.map_or(cost_of(first), f64::from).max(cost_of(first));
        let candidates = (list / read.selectivity).min(all);
        Some(model.index_descent * (all + 1.0_f64).log2() + candidates * storage.fetch_cost() + model.sort(cost_of(first)))
    }
}
