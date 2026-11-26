//! \[DESIGN\] The strict dependencies (RULES.md D1201): those of
//! [ISO/IEC 9075-2:2023 §4.26] that hold of rows as §4.26.1 defines a
//! dependency, two rows not distinct in the determinant not distinct in
//! what it determines, whatever tells two values that are not distinct
//! apart. A grouping may then group by the determinant alone and make the
//! same groups.
//!
//! Two character strings equal but for trailing spaces or by a collation,
//! two approximate zeros and two datetimes of one instant in two zones
//! are not distinct, and an expression may tell them apart, their lengths
//! say. So a dependency of a column on the columns an expression reads
//! [§4.26.12 note 69], or a generated column's on its parametric columns
//! [§4.26.3], is kept only of exact numbers, truth values and datetimes
//! of no zone, which are not distinct only as one value. An equality of
//! two columns [§4.26.12] makes each determine the other only where it
//! compares both by their own values and by one relation: two exact
//! numbers, two character strings of no collation or each by its own, or
//! two values of one such type. An outer join's condition [§4.26.7] adds
//! what it adds only if each AND-component of it that reads a column of
//! the preserved operand is such an equality or reads such types alone. A
//! collection derived table's ordinality column determines nothing, nor
//! is anything known of a grouping by a collation.

use super::conditions::by_sort_key;
use super::{Attrs, Known, Walk, known};
use crate::db::State;
use crate::logical::{Cond, Expr, LogicalPlan, cond_columns};
use crate::value::collation::CollationRef;
use crate::value::{CompOp, DataType};

/// Whether two values of the type `t` not distinct are one value to
/// every expression.
fn plain(t: &DataType) -> bool {
    matches!(t, DataType::Exact { .. } | DataType::Boolean | DataType::Date)
        || matches!(t, DataType::Time { zoned: false, .. } | DataType::Timestamp { zoned: false, .. })
}

/// Whether an equality of a value of `a` and one of `b` is one relation
/// with the one each is not distinct from itself by.
fn one_relation(a: &DataType, b: &DataType) -> bool {
    let string = |t: &DataType| matches!(t, DataType::Character { .. } | DataType::Varying { .. });
    let exact = |t: &DataType| matches!(t, DataType::Exact { .. });
    (exact(a) && exact(b)) || (string(a) && string(b)) || (plain(a) && a == b)
}

impl Known {
    /// What is known strictly of the rows of `plan`, of the database
    /// `db`, as the module says.
    pub(crate) fn strict(db: &State, plan: &LogicalPlan) -> Known {
        known(&Walk { db, next: std::cell::Cell::new(0), strict: true }, plan)
    }

    /// The collation of its column `c`, if it is a base table's column
    /// that has one.
    pub(crate) fn collation_of(&self, c: usize) -> Option<&CollationRef> {
        let attribute = self.attribute_of(c)?;
        self.collations.iter().find(|(at, _)| *at == attribute).map(|(_, own)| own)
    }

    /// The declared type of the attribute `a`, if a base table's column.
    fn type_of(&self, a: usize) -> Option<&DataType> {
        self.types.iter().find(|(at, _)| *at == a).map(|(_, t)| t)
    }

    /// Whether what an expression makes of the attributes `read` is one
    /// value of rows not distinct in them: always but strictly, and then
    /// of plain types alone.
    pub(super) fn told_alike(&self, read: &Attrs) -> bool {
        !self.strict || read.iter().all(|&a| self.type_of(a).is_some_and(plain))
    }

    /// Whether an equality of `left` and `right`, each a column compared
    /// by its own values, makes each determine the other: always but
    /// strictly, and then as the module says.
    pub(super) fn compared_alike(&self, left: &Expr, right: &Expr) -> bool {
        if !self.strict {
            return true;
        }
        let typed = |e: &Expr| {
            let attribute = self.attribute_of(self.compared_column(e)?)?;
            let own = by_sort_key(e) || !self.collations.iter().any(|(at, _)| *at == attribute);
            self.type_of(attribute).filter(|_| own)
        };
        typed(left).zip(typed(right)).is_some_and(|(a, b)| one_relation(a, b))
    }

    /// Whether the AND-component `c` of an outer join's condition matches
    /// alike two rows of the preserved operand, whose columns `preserved`
    /// tells, that are not distinct in the columns the condition reads:
    /// always but strictly, and then as the module says.
    pub(super) fn matches_alike(&self, c: &Cond, preserved: &dyn Fn(usize) -> bool) -> bool {
        let mut read = Vec::new();
        cond_columns(c, &mut read);
        read.retain(|&column| preserved(column));
        if self.attributes_of(&read).is_some_and(|attributes| self.told_alike(&attributes)) {
            return true;
        }
        let Cond::Compare(l, CompOp::Eq, r) = c else { return false };
        self.compared_column(l).is_some() && self.compared_column(r).is_some() && self.compared_alike(l, r)
    }
}
