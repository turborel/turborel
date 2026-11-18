//! [X3.135-1986 §5.7] A `<column specification>` resolved: the column of
//! the FROM row it names, qualified or not, and its uses noted.

use super::{Binder, FromItem, Purpose, no_column, require, rule, underlying, views};
use crate::ast::{ColumnSpecification, TableName};
use crate::catalog::{Actions, ColumnDesc};
use crate::error::{At, Result};
use crate::value::DataType;

impl Binder<'_> {
    /// [X3.135-1986 §5.7] `<column specification>`: its position in the FROM row and data type.
    pub(super) fn resolve(&self, cs: &ColumnSpecification) -> Result<(usize, DataType)> {
        self.resolve_column(cs).map(|c| (c.0, c.1)).or_at(cs.at)
    }

    /// [X3.135-1986 §5.7 SR3] Also reports whether the column is constrained
    /// NOT NULL, which a named result column inherits [X3.135-1986 §5.25 SR10].
    pub(super) fn resolve_column(&self, cs: &ColumnSpecification) -> Result<(usize, DataType, bool)> {
        let (item, i, col) = match &cs.qualifier {
            Some(q) => self.qualified(q, &cs.column)?,
            None => self.unqualified(&cs.column)?,
        };
        Ok((self.note(item, i)?, col.data_type.clone(), col.not_null && !item.nullable))
    }

    /// Whether `c` is a column the FROM clause's results have.
    pub(super) fn visible_has(&self, c: &str) -> bool {
        self.visible.iter().any(|&(i, k)| self.items.get(i).and_then(|it| it.desc.columns.get(k)).is_some_and(|col| col.name == c))
    }

    /// The position in the FROM row of column `i` of `item`, which the
    /// query uses: in a constraint, of a named table or view, one it holds
    /// REFERENCES on [ISO/IEC 9075:1992 §11.9 AR1a], a derived table's
    /// columns being of no `<table name>`, and its query's checked as they
    /// are bound. Noted among its uses, if it is of a named table or view,
    /// under the name it has there.
    pub(super) fn note(&self, item: &FromItem, i: usize) -> Result<usize> {
        if self.purpose == Purpose::Constraint {
            if !item.table.auth.is_empty() {
                let needed = Actions { references: std::iter::once(i).collect(), ..Actions::default() };
                require(self.db, self.who(), &item.desc, &needed, "ISO/IEC 9075:1992 §11.9 AR1a")?;
            }
            if let Some(r) = self.referencing {
                r.column_named(&item.table);
            }
        }
        if let Some(uses) = self.uses.filter(|_| !item.table.auth.is_empty()) {
            let column = views::column_name(self.db, &item.table, i).or_else(|| item.desc.columns.get(i).map(|c| c.name.clone()));
            uses.borrow_mut().extend(column.map(|c| (item.table.clone(), c)));
        }
        // [ISO/IEC 9075-2:2023 §7.17 SR23b] a column of a query a <with
        // clause> names is drawn from what its query's column is.
        if let (Some(uses), Some(drawn)) = (self.uses, &item.drawn) {
            uses.borrow_mut().extend(drawn.columns.get(i).cloned().unwrap_or_default());
        }
        if let (Some(uses), views::Source::Common) = (self.uses, &item.source) {
            underlying::note_common(self, item, i, uses);
        }
        let pos = item.offset.saturating_add(item.position(i));
        self.used.borrow_mut().insert(pos);
        Ok(pos)
    }

    /// [X3.135-1986 §5.7 SR3a] column `c` of the table that `<qualifier>` `q` designates.
    pub(super) fn qualified(&self, q: &TableName, c: &str) -> Result<(&FromItem, usize, &ColumnDesc)> {
        let name = q.auth.as_ref().map_or_else(|| q.ident.clone(), |a| format!("{a}.{}", q.ident));
        let it = self
            .items
            .iter()
            .find(|it| it.designated_by(q, self.schema))
            .ok_or_else(|| rule(format!("[X3.135-1986 §5.7 SR3a] no table or correlation name {name} is in scope")))?;
        let (i, col) =
            it.desc.column_desc(c).ok_or_else(|| no_column(&it.table, format!("[X3.135-1986 §5.7 SR3a] {name} has no column {c}"), c))?;
        if it.common.contains(&i) {
            return Err(rule(format!(
                "[ISO/IEC 9075:1992 §6.4 SR2b] {c} is a common column of a joined table {name} is in, so it is not qualified"
            )));
        }
        Ok((it, i, col))
    }

    /// [X3.135-1986 §5.7 SR3b] column `c` of the one table in scope that
    /// has it: of the columns the FROM clause's results have, which for a
    /// NATURAL or USING join are its common columns, not its operands'
    /// [ISO/IEC 9075:1992 §7.5 SR6f].
    pub(super) fn unqualified(&self, c: &str) -> Result<(&FromItem, usize, &ColumnDesc)> {
        let possible: Vec<_> = self
            .visible
            .iter()
            .filter_map(|&(i, k)| {
                let it = self.items.get(i)?;
                it.desc.columns.get(k).filter(|col| col.name == c).map(|col| (it, k, col))
            })
            .collect();
        match possible.as_slice() {
            [only] => Ok(*only),
            [] => Err(rule(format!("[X3.135-1986 §5.7 SR3b] no table in the FROM clause has a column {c}"))),
            [_, _, ..] => Err(rule(format!("[X3.135-1986 §5.7 SR3b] column {c} is ambiguous; qualify it"))),
        }
    }
}
