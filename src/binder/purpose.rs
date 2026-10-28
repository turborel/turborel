//! What a query is bound for: to be run, or as a constraint. Split from
//! binder.rs (RULES.md R-SIZE-2).

/// What a query is bound for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Purpose {
    /// To be run.
    Query,
    /// [ISO/IEC 9075:1992 §11.9] The `<search condition>` of a CHECK,
    /// kept and evaluated whenever what it reads changes: one with no
    /// authorization identifier in it \[SR5\] and not possibly
    /// non-deterministic [SR6, §4.10], whose tables it holds REFERENCES on
    /// \[AR1\].
    Constraint,
    /// A view such a `<search condition>` reads, which "generally
    /// contains" what the view's query does \[SR5, SR6\]: bound as its
    /// owner's, as a view always is.
    UnderConstraint,
}

impl Purpose {
    /// Whether the query is part of a constraint [§11.9 SR5, SR6].
    pub(super) fn constrains(self) -> bool {
        matches!(self, Purpose::Constraint | Purpose::UnderConstraint)
    }

    /// The purpose of a view the query reads.
    pub(super) fn of_view(self) -> Purpose {
        if self.constrains() { Purpose::UnderConstraint } else { Purpose::Query }
    }
}
