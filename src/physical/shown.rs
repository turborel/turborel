//! \[DESIGN\] What EXPLAIN shows of an operator: text made of the names of
//! its input's columns, when a plan is made to be shown; none, when it is
//! made to run, so a statement run pays nothing for it.

use super::PhysicalPlanner;
use crate::logical::LogicalPlan;

/// The names an operator's description is made of, if the plan is shown.
#[derive(Clone, Copy)]
pub(super) struct Shown<'a> {
    names: Option<&'a [String]>,
}

impl<'a> Shown<'a> {
    /// Of `names`, given if the plan is shown.
    pub(super) fn of(names: Option<&'a [String]>) -> Shown<'a> {
        Shown { names }
    }

    /// The names, if the plan is shown.
    pub(super) fn names(self) -> Option<&'a [String]> {
        self.names
    }

    /// `render` of the names, if the plan is shown; else nothing.
    pub(super) fn text(self, render: impl FnOnce(&[String]) -> String) -> String {
        self.names.map(render).unwrap_or_default()
    }
}

impl PhysicalPlanner<'_> {
    /// `names`, an input's column names, as the operators over it are
    /// described by: not at all, of a plan to run.
    pub(super) fn showing<'n>(&self, names: &'n [String]) -> Shown<'n> {
        Shown::of(self.shown.then_some(names))
    }

    /// The names of `input`'s columns, if the operators over it are
    /// described.
    pub(super) fn names_of(&self, input: &LogicalPlan) -> Option<Vec<String>> {
        self.shown.then(|| input.names())
    }

    /// `render` of the names of `input`'s columns, if the operators over it
    /// are described; else nothing.
    pub(super) fn described(&self, input: &LogicalPlan, render: impl FnOnce(&[String]) -> String) -> String {
        if self.shown { render(&input.names()) } else { String::new() }
    }
}

// Unit tests: a plan shown is described, one run is not (RULES.md R-TEST-1).
#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] an operator is described by its input's column
    /// names when the plan is shown, and by nothing when it is run.
    #[test]
    fn an_operator_is_described_only_when_the_plan_is_shown() {
        let names = ["A".to_string(), "B".to_string()];
        assert_eq!(Shown::of(Some(&names)).text(|n| n.join(", ")), "A, B");
        assert_eq!(Shown::of(None).text(|n| n.join(", ")), "");
        assert_eq!(Shown::of(Some(&names)).names().map(<[String]>::len), Some(2));
        assert!(Shown::of(None).names().is_none());
    }
}
