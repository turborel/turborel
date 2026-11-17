//! [ISO/IEC 9075-2:2023 §11.3 SR11g, SR11d v, SR14] The `<column options>`
//! of a typed table's definition applied to the columns it stands for:
//! each of an originally-defined column, its scope replacing the scope of
//! the column's reference type, its default replacing the column's, its
//! constraints added to the column's.

use super::TypedColumn;
use crate::ast::{ColumnConstraintKind, ColumnOptions, TableDefinition, TableElement, TableName, TypeName};
use crate::binder::qualify;
use crate::catalog::QualifiedName;
use crate::db::{State, rule};
use crate::error::Result;
use crate::value::DataType;

impl State {
    /// [ISO/IEC 9075-2:2023 §11.3 SR11g] `columns`, of the typed table
    /// `table` defined by `t`, its names of schema `names`, each named by
    /// `<column options>` of `t` changed as they say \[SR11g iv to vi\],
    /// those deleted from its elements \[SR11g vii\].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one names no originally-defined column \[SR11g i\],
    /// a scope of a column of no reference type \[SR11g iv\] or of no
    /// persistent base table \[SR14\], or, of a subtable, a PRIMARY KEY
    /// \[SR11d v\].
    pub(super) fn optioned(&self, (table, names): (&QualifiedName, &str), columns: &mut [TypedColumn], t: &TableDefinition) -> Result<()> {
        let subtable = t.typed.as_ref().is_some_and(|typed| typed.under.is_some());
        for o in t.elements.iter().filter_map(|e| if let TableElement::Options(o) = e { Some(o) } else { None }) {
            let (name, at) = (&o.column.0, o.column.1);
            let Some(c) = columns.iter_mut().find(|c| c.def.name == *name) else {
                return Err(
                    rule(format!("[ISO/IEC 9075-2:2023 §11.3 SR11g i] {table} has no column {name} to take <column options>")).or_at(at)
                );
            };
            if !c.original {
                let msg = format!(
                    "[ISO/IEC 9075-2:2023 §11.3 SR11g i] column {name} of {table} is no originally-defined column, so it takes no <column options>"
                );
                return Err(rule(msg).or_at(at));
            }
            let primary = o.constraints.iter().find(|k| matches!(k.kind, ColumnConstraintKind::Unique { primary: true }));
            if let (true, Some(k)) = (subtable, primary) {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.3 SR11d v] subtable {table} has no PRIMARY KEY")).or_at(k.at));
            }
            if let Some(scope) = &o.scope {
                self.scoped(c, (scope, names), t)?;
            }
            apply(c, o);
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.3 SR11g iv, SR14] Column `c` of the table
    /// defined by `t` given the scope `scope`, its names of schema
    /// `names`: its reference type's scope replaced.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if `c` is of no reference type, or `scope` is a view
    /// or a table of another kind than `t`'s.
    fn scoped(&self, c: &mut TypedColumn, (scope, names): (&TableName, &str), t: &TableDefinition) -> Result<()> {
        let Some(TypeName::Data(DataType::Reference(rt))) = &c.def.data_type else {
            let msg = format!(
                "[ISO/IEC 9075-2:2023 §11.3 SR11g iv] column {} is of no reference type, so it takes no <scope clause>",
                c.def.name
            );
            return Err(rule(msg).or_at(scope.at));
        };
        let sq = qualify(scope, names);
        let kind = |temporary: Option<crate::ast::Temporary>| temporary.map(|k| k.kind);
        let other = self.catalog.table(&sq).is_some_and(|d| kind(d.temporary) != kind(t.temporary));
        if self.catalog.view(&sq).is_some() || other {
            let wanted = if t.temporary.is_some() { "a temporary table of its table's kind" } else { "a persistent base table" };
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §11.3 SR14] the scope {sq} of column {} is {wanted}, as it is not",
                c.def.name
            ))
            .or_at(scope.at));
        }
        let referenced = TableName { auth: Some(rt.referenced.name.schema.clone()), ident: rt.referenced.name.name.clone(), at: scope.at };
        // Resolved as the column is described, as its owner may name it
        // [§6.1 SR45, AR1].
        c.def.data_type = Some(TypeName::Reference(referenced, Some(scope.clone())));
        Ok(())
    }
}

/// [ISO/IEC 9075-2:2023 §11.3 SR11g v, vi] Column `c` given the default
/// and the constraints of `o`.
fn apply(c: &mut TypedColumn, o: &ColumnOptions) {
    if o.default.is_some() {
        c.def.default.clone_from(&o.default);
    }
    c.def.constraints.extend(o.constraints.iter().cloned());
}
