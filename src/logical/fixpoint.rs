//! [ISO/IEC 9075-2:2023 §7.17 GR2c ii] How the rows of a recursive stratum
//! of a `<with list>` are found, of logical plans and of physical ones
//! alike: the iteration the General Rules give, or one over only the rows
//! each iteration adds, where that gives the same rows.

/// [ISO/IEC 9075-2:2023 §7.17 GR2c ii] How the rows of the members of a
/// recursive stratum are found, of plans `P`. The plans read the rows each
/// member had after the last iteration as work tables (`WorkTable`).
#[derive(Debug, Clone)]
pub enum Fixpoint<P> {
    /// As the General Rules say: each member's query, one for each member
    /// in order, evaluated over every row of the last iteration, until no
    /// member's result has more rows than that iteration gave it.
    Whole(Vec<P>),
    /// The stratum's one member, whose query is the UNION of `anchor`, its
    /// non-recursive operand, and `step`, its recursive one, which
    /// distributes over UNION ALL: `anchor` evaluated once, then `step`
    /// over only the rows the last iteration added, until it adds none; of
    /// UNION DISTINCT, `distinct`, a row added before not added again. It
    /// gives the rows `Whole` gives, with fewer evaluated.
    Delta {
        /// The non-recursive operand.
        anchor: Box<P>,
        /// The recursive operand.
        step: Box<P>,
        /// UNION DISTINCT: no row twice.
        distinct: bool,
    },
}

impl<P> Fixpoint<P> {
    /// Its plans, in order: each member's query, or the anchor and the
    /// step.
    pub fn plans(&self) -> (Option<&P>, Option<&P>, &[P]) {
        match self {
            Fixpoint::Whole(plans) => (None, None, plans),
            Fixpoint::Delta { anchor, step, .. } => (Some(anchor), Some(step), &[]),
        }
    }

    /// The same iteration, each of its plans `f` of it.
    #[must_use]
    pub fn map<Q>(self, mut f: impl FnMut(P) -> Q) -> Fixpoint<Q> {
        match self {
            Fixpoint::Whole(plans) => Fixpoint::Whole(plans.into_iter().map(f).collect()),
            Fixpoint::Delta { anchor, step, distinct } => {
                let anchor = Box::new(f(*anchor));
                Fixpoint::Delta { anchor, step: Box::new(f(*step)), distinct }
            }
        }
    }

    /// The same iteration, each of its plans `f` of a reference to it.
    pub fn map_ref<Q>(&self, mut f: impl FnMut(&P) -> Q) -> Fixpoint<Q> {
        match self {
            Fixpoint::Whole(plans) => Fixpoint::Whole(plans.iter().map(f).collect()),
            Fixpoint::Delta { anchor, step, distinct } => {
                let anchor = Box::new(f(anchor));
                Fixpoint::Delta { anchor, step: Box::new(f(step)), distinct: *distinct }
            }
        }
    }

    /// Calls `f` with each of its plans, each to be changed in place.
    pub fn for_each_mut(&mut self, f: &mut dyn FnMut(&mut P)) {
        match self {
            Fixpoint::Whole(plans) => plans.iter_mut().for_each(f),
            Fixpoint::Delta { anchor, step, .. } => {
                f(anchor);
                f(step);
            }
        }
    }

    /// How it is shown by EXPLAIN.
    pub fn word(&self) -> &'static str {
        match self {
            Fixpoint::Whole(_) => "whole",
            Fixpoint::Delta { distinct: false, .. } => "delta",
            Fixpoint::Delta { distinct: true, .. } => "delta distinct",
        }
    }
}
