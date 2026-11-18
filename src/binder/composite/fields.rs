//! The fields of a row spread into a select list [ISO/IEC 9075-2:2023
//! §7.16 SR10, SR11]: an `<all fields reference>` and the `q`.* whose
//! basis is a column of a row type.

use super::super::{Binder, Scope, rule};
use crate::ast::{ColumnSpecification, Composite, SelectItem, TableName, ValueExpression};
use crate::error::{At, Result};
use crate::value::DataType;
use crate::value::types::Field;

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §7.16 SR11] `<all fields reference>` `value`.*
    /// \[AS ( `names` )\]: VEP.F1 AS N1, ..., one item for each field of its
    /// row type, as many names as fields \[SR11a\], each else named as its
    /// field (UV087).
    pub(in crate::binder) fn all_fields(&self, value: &ValueExpression, names: Option<&[String]>, at: usize) -> Result<Vec<SelectItem>> {
        let scope = Scope::Rows("[ISO/IEC 9075-2:2023 §7.16 SR11] the row an <all fields reference> spreads is found of each row");
        let (_, t) = self.value_expression(value, scope).or_at(at)?;
        let DataType::Row(r) = &t else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.16 SR11] .* spreads a row, not a value of {t}")).or_at(at));
        };
        if let Some(n) = names.filter(|n| n.len() != r.fields.len()) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §7.16 SR11a] {} names are given the {} fields of {t}",
                n.len(),
                r.fields.len()
            ))
            .or_at(at));
        }
        let item = |(i, f): (usize, &Field)| {
            let field = Composite::Field(Box::new(value.clone()), f.name.clone());
            let alias = names.and_then(|n| n.get(i)).cloned().unwrap_or_else(|| f.name.clone());
            SelectItem { value: ValueExpression::Composite(Box::new(field), at), alias: Some(alias), at }
        };
        Ok(r.fields.iter().enumerate().map(item).collect())
    }

    /// [ISO/IEC 9075-2:2023 §7.16 SR10] `q`.* of two identifiers whose
    /// basis is the second, a column of a row type of the table the first
    /// names \[SR10f iv\]: its `<all fields reference>` \[SR10g ii 1\]; none
    /// if it is not, `table` saying whether a table is the basis \[SR10f
    /// vi\], as exactly one is \[SR10f vii\].
    pub(in crate::binder) fn row_asterisk(&self, q: &TableName, table: bool) -> Result<Option<Vec<SelectItem>>> {
        let Some(first) = &q.auth else { return Ok(None) };
        let column = ColumnSpecification {
            qualifier: Some(TableName { auth: None, ident: first.clone(), at: q.at }),
            column: q.ident.clone(),
            catalog: None,
            at: q.at,
        };
        if !matches!(self.local(&column), Ok(Some((_, DataType::Row(_), _)))) {
            return Ok(None);
        }
        if table {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §7.16 SR10f vii] {first}.{} is both a table and a column of a row type",
                q.ident
            ))
            .or_at(q.at));
        }
        self.all_fields(&ValueExpression::Column(column), None, q.at).map(Some)
    }
}
