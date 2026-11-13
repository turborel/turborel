//! Arrays and rows bound [ISO/IEC 9075-2:2023 §6.16, §6.25, §6.31, §6.47,
//! §6.48, §6.49, §7.1]: each expression a call of a function of
//! `functions/arrays.rs` or `functions/rows.rs` (RULES.md D68); the
//! identifier chain whose basis is a column of a row type [§6.6]; and the
//! rules of the operations arrays and rows take part in [§9.12, §9.14].

use std::sync::Arc;

use super::case::{brought, set_type};
use super::statements::assignable;
use super::{Binder, Scope, rule};
use crate::ast::{ColumnSpecification, Composite, SelectItem, SetClause, TableName, UpdateSource, ValueExpression};
use crate::catalog::ColumnDesc;
use crate::error::{At, Code, Result, SqlError};
use crate::functions::{
    ARRAY_MAX_CARDINALITY, ArrayConcatenation, ArrayOf, AssignElement, CARDINALITY, CardinalityOf, ElementOf, FieldOf, RowOf, ScalarRef,
    TrimArrayOf,
};
use crate::logical::Expr;
use crate::params::ParameterType;
use crate::value::types::composite::{array_ordered, nesting};
use crate::value::types::{Field, MAX_ARRAY_CARDINALITY, MAX_TYPE_NESTING};
use crate::value::{DataType, Decimal, INTEGER_PRECISION, Spelled, Value};

/// [ISO/IEC 9075-2:2023 §6.31 SR9, SR10] The type of CARDINALITY and
/// ARRAY_MAX_CARDINALITY (IV152, IV153): INTEGER.
pub(super) fn integer() -> DataType {
    DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer }
}

/// The call of `f`, of `args`.
fn call(f: impl crate::functions::ScalarFunction + 'static, args: Vec<Expr>) -> Expr {
    Expr::Call(ScalarRef(Arc::new(f)), args)
}

/// [ISO/IEC 9075-2:2023 §9.14 SR3] An operand of an ordering operation,
/// `what`, of type `t`, is not array-ordered.
///
/// # Errors
///
/// SQLCODE -103 if it is.
pub(super) fn ordering_operand(t: &DataType, what: &str) -> Result<()> {
    if array_ordered(t) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §9.14 SR3] {what} orders values of {t}, and arrays are compared for equality alone"
        )));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §9.12 SR3] An operand of a grouping operation,
/// `what`, of type `t`, is not array-ordered.
///
/// # Errors
///
/// SQLCODE -103 if it is.
pub(crate) fn grouping_operand(t: &DataType, what: &str) -> Result<()> {
    if array_ordered(t) {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §9.12 SR3] {what} groups values of {t}, and arrays are grouped by no operation")));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §6.13 SR5-SR8] Whether CAST takes a value of
/// `from` to `to`, either an array, a row or a distinct type: an array to an array
/// whose element type its elements cast to; a row to a row of its degree,
/// field by field; neither from nor to another type. Of others, true, as
/// their own rules say.
pub(super) fn castable(from: &DataType, to: &DataType) -> bool {
    // [§6.13 SR12] By a user-defined cast whose source's designator is in
    // `from`'s type precedence list, to its target: as an appropriate one
    // assigns, implicitly invocable as a distinct type's casts are.
    if let Some(cast) = super::statements::distinct_assignable(to, from) {
        return cast;
    }
    let part = |x: &DataType, y: &DataType| castable(x, y) && element_castable(x, y);
    if let (DataType::Array(a), DataType::Array(b)) = (from, to) {
        return part(&a.element, &b.element);
    }
    if let (DataType::Row(a), DataType::Row(b)) = (from, to) {
        return a.fields.len() == b.fields.len() && a.fields.iter().zip(&b.fields).all(|(x, y)| part(&x.data_type, &y.data_type));
    }
    !(is_composite(from) || is_composite(to))
}

/// Whether `t` is an array or a row type.
fn is_composite(t: &DataType) -> bool {
    matches!(t, DataType::Array(_) | DataType::Row(_))
}

/// `t`, a type an expression makes, nested no deeper than
/// `MAX_TYPE_NESTING` (RULES.md R-CF-6), as a type written is
/// (`parser/composite.rs`).
///
/// # Errors
///
/// SQLCODE -900 if it is nested deeper.
pub(super) fn bounded(t: DataType) -> Result<DataType> {
    if nesting(&t) > MAX_TYPE_NESTING {
        return Err(SqlError::new(
            Code::Unsupported,
            format!("[DESIGN] {t} nests arrays and rows deeper than this implementation's limit of {MAX_TYPE_NESTING}"),
        ));
    }
    Ok(t)
}

/// Whether a part of `from` casts to `to`, as the rules of predefined
/// types say: a binary string with a binary string alone [§6.13 SR7].
fn element_castable(from: &DataType, to: &DataType) -> bool {
    super::datetime::binary_castable(from, to)
}

/// [ISO/IEC 9075-2:2023 §9.5 SR3g, SR3h] The result type of `types`, one
/// of them a row or an array type: a row of the result types of the
/// fields in each place, named as all name it there, else F and its place
/// (UV094); an array of the result type of the element types, of the
/// greatest maximum cardinality. None if none is a row or an array type.
pub(super) fn composite_set_type(types: &[DataType], clause: &str) -> Option<Result<DataType>> {
    let rows: Vec<_> = types.iter().filter_map(|t| if let DataType::Row(r) = t { Some(r) } else { None }).collect();
    let arrays: Vec<_> = types.iter().filter_map(|t| if let DataType::Array(a) = t { Some(a) } else { None }).collect();
    if rows.is_empty() && arrays.is_empty() {
        return None;
    }
    if rows.len() != types.len() && arrays.len() != types.len() {
        return Some(Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §9.5 SR3g, SR3h] {clause}: rows or arrays are brought together with other values"
        ))));
    }
    if let Some(max) = arrays.iter().map(|a| a.max).max() {
        let elements: Vec<DataType> = arrays.iter().map(|a| a.element.clone()).collect();
        return Some(set_type(&elements, clause).map(|element| DataType::array(element, max)));
    }
    let degree = rows.first().map_or(0, |r| r.fields.len());
    if rows.iter().any(|r| r.fields.len() != degree) {
        return Some(Err(rule(format!("[ISO/IEC 9075-2:2023 §9.5 SR3g] {clause}: rows of different degrees are brought together"))));
    }
    let field = |i: usize| -> Result<Field> {
        let named: Vec<&Field> = rows.iter().filter_map(|r| r.fields.get(i)).collect();
        let field_types: Vec<DataType> = named.iter().map(|f| f.data_type.clone()).collect();
        let first = named.first().map(|f| f.name.clone()).unwrap_or_default();
        let (name, unnamed) =
            if named.iter().all(|f| f.name == first) { (first, named.iter().all(|f| f.unnamed)) } else { (anonymous(i), true) };
        Ok(Field { name, data_type: set_type(&field_types, clause)?, unnamed })
    };
    Some((0..degree).map(field).collect::<Result<Vec<_>>>().map(DataType::row))
}

/// [ISO/IEC 9075-2:2023 §7.1 SR3b] The name of the field at `i`, from 0,
/// of a row made of values (UV077): F and its place, as [§14.15 NOTE 688]
/// names them.
pub(super) fn anonymous(i: usize) -> String {
    format!("F{}", i.saturating_add(1))
}

/// Whether `v` is `ARRAY[]`, an `<empty specification>` [ISO/IEC
/// 9075-2:2023 §6.5].
pub(super) fn is_empty_array(v: &ValueExpression) -> bool {
    matches!(v, ValueExpression::Composite(c, _) if matches!(&**c, Composite::Array(e) if e.is_empty()))
}

impl Binder<'_> {
    /// An expression of arrays and rows, bound.
    pub(super) fn composite(&self, (c, at): (&Composite, usize), scope: Scope<'_>) -> Result<(Expr, DataType)> {
        match c {
            Composite::Array(values) => self.array_of(values, scope),
            Composite::ArrayQuery(s, order_by) => self.array_query(s, order_by, scope),
            Composite::Row(values) => self.row_of(values, scope),
            Composite::Element(array, index) => self.element(array, index, scope),
            Composite::Field(value, name) => {
                let bound = self.value_expression(value, scope)?;
                field_of(bound, name)
            }
            Composite::Chain(parts) => self.chain(parts, at, scope),
        }
    }

    /// [ISO/IEC 9075-2:2023 §6.49 SR2, GR2] `<array value constructor by
    /// enumeration>`: of the type of all its elements \[§9.5\], each cast to
    /// it, its maximum cardinality their number. ARRAY[] has a type only
    /// where its context gives it one [§6.5 SR1], as `typed` takes it.
    fn array_of(&self, values: &[ValueExpression], scope: Scope<'_>) -> Result<(Expr, DataType)> {
        if values.is_empty() {
            return Err(rule("[ISO/IEC 9075-2:2023 §6.5 SR1] ARRAY[] is of the element type its context gives, and none is given here"));
        }
        let bound = values.iter().map(|v| self.value_expression(v, scope)).collect::<Result<Vec<_>>>()?;
        let max = u32::try_from(bound.len()).ok().filter(|&n| n <= MAX_ARRAY_CARDINALITY).ok_or_else(|| {
            rule(format!(
                "[ISO/IEC 9075-2:2023 §6.49 SR2] ARRAY of {} elements exceeds this implementation's maximum cardinality of {MAX_ARRAY_CARDINALITY}",
                bound.len()
            ))
        })?;
        let types: Vec<DataType> = bound.iter().map(|(_, t)| t.clone()).collect();
        let element = set_type(&types, "§6.49 SR2")?;
        let args = bound.into_iter().map(|(e, t)| brought(e, &t, element.clone())).collect();
        let t = bounded(DataType::array(element, max))?;
        Ok((call(ArrayOf(t.clone()), args), t))
    }

    /// [ISO/IEC 9075-2:2023 §7.1 SR3b, GR3b] `<explicit row value
    /// constructor>`: a row of its values, each field of its value's type,
    /// named as `anonymous` names it.
    pub(super) fn row_of(&self, values: &[ValueExpression], scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let bound = values.iter().map(|v| self.value_expression(v, scope)).collect::<Result<Vec<_>>>()?;
        let fields =
            bound.iter().enumerate().map(|(i, (_, t))| Field { name: anonymous(i), data_type: t.clone(), unnamed: true }).collect();
        let t = bounded(DataType::row(fields))?;
        Ok((call(RowOf(t.clone()), bound.into_iter().map(|(e, _)| e).collect()), t))
    }

    /// [ISO/IEC 9075-2:2023 §6.25] `<array element reference>`: of the
    /// element type \[SR1\], its position exact of scale 0 \[SR2\], a `?`
    /// taking INTEGER.
    fn element(&self, array: &ValueExpression, index: &ValueExpression, scope: Scope<'_>) -> Result<(Expr, DataType)> {
        let (a, t) = self.value_expression(array, scope)?;
        let DataType::Array(of) = &t else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.25 SR1] an element is referenced of an array, not of {t}")));
        };
        let i = self.position(index, "§6.25 SR2", scope)?;
        let element = of.element.clone();
        Ok((call(ElementOf(element.clone()), vec![a, i]), element))
    }

    /// A position in an array, `index`: of an exact numeric type of scale
    /// 0, as `clause` says, a `?` taking INTEGER.
    pub(super) fn position(&self, index: &ValueExpression, clause: &str, scope: Scope<'_>) -> Result<Expr> {
        let (i, t) = self.typed(index, ParameterType::Of(integer()), scope)?;
        if !matches!(t, DataType::Exact { scale: 0, .. }) {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 {clause}] a position in an array is exact numeric of scale 0, not {t}")));
        }
        Ok(i)
    }

    /// [ISO/IEC 9075-2:2023 §6.31, §6.48] CARDINALITY, ARRAY_MAX_CARDINALITY
    /// or TRIM_ARRAY of `args`, if `name` is one of them.
    pub(super) fn array_function(&self, name: &str, args: &[ValueExpression], scope: Scope<'_>) -> Option<Result<(Expr, DataType)>> {
        let array = |p: &Self| -> Result<(Expr, DataType)> {
            let (e, t) =
                p.value_expression(args.first().ok_or_else(|| rule(format!("[ISO/IEC 9075-2:2023 §6.31] {name} has an operand")))?, scope)?;
            if !matches!(t, DataType::Array(_)) {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.31, §6.47 SR2, §6.48] the operand of {name} is an array, not of {t}")));
            }
            Ok((e, t))
        };
        Some(match name {
            CARDINALITY => array(self).map(|(e, _)| (call(CardinalityOf(integer()), vec![e]), integer())),
            // [§6.31 GR1a] the maximum cardinality of the declared type.
            ARRAY_MAX_CARDINALITY => array(self).map(|(_, t)| {
                let max = if let DataType::Array(a) = &t { a.max } else { 0 };
                (Expr::Const(Value::Exact(Decimal::new(i128::from(max), 0))), integer())
            }),
            crate::functions::TRIM_ARRAY => array(self).and_then(|(e, t)| {
                let n = self.position(
                    args.get(1).ok_or_else(|| rule("[ISO/IEC 9075-2:2023 §6.48] TRIM_ARRAY has two operands"))?,
                    "§6.48 SR2a",
                    scope,
                )?;
                Ok((call(TrimArrayOf(t.clone()), vec![e, n]), t))
            }),
            _ => return None,
        })
    }

    /// [ISO/IEC 9075-2:2023 §6.47 SR3, GR2] `<array concatenation>` of
    /// `left` and `right`, if both are arrays: of the type of both \[§9.5\]
    /// whose maximum cardinality is the sum of theirs, up to IL008.
    pub(super) fn array_concatenation((left, right): (&(Expr, DataType), &(Expr, DataType))) -> Option<Result<(Expr, DataType)>> {
        let (DataType::Array(a), DataType::Array(b)) = (&left.1, &right.1) else { return None };
        let max = a.max.saturating_add(b.max).min(MAX_ARRAY_CARDINALITY);
        Some(set_type(&[a.element.clone(), b.element.clone()], "§6.47 SR3a").map(|element| {
            let t = DataType::array(element, max);
            let args = vec![brought(left.0.clone(), &left.1, t.clone()), brought(right.0.clone(), &right.1, t.clone())];
            (call(ArrayConcatenation(t.clone()), args), t)
        }))
    }

    /// [ISO/IEC 9075-2:2023 §6.6 SR9b iv 2, SR9c, SR11] The `<column
    /// reference>` `cs` of three identifiers, if its basis is the second,
    /// a column of a row type of the table its first names, the third a
    /// field: that field reference; none if its basis is not, so it is a
    /// column's.
    pub(super) fn field_chain(&self, cs: &ColumnSpecification, scope: Scope<'_>) -> Result<Option<(Expr, DataType)>> {
        let (Some(TableName { auth: Some(first), ident: second, at }), None) = (&cs.qualifier, &cs.catalog) else { return Ok(None) };
        let basis = ColumnSpecification {
            qualifier: Some(TableName { auth: None, ident: first.clone(), at: *at }),
            column: second.clone(),
            catalog: None,
            at: cs.at,
        };
        let refinable = matches!(self.local(&basis), Ok(Some((_, DataType::Row(_), _))));
        if !refinable {
            return Ok(None);
        }
        // [SR9c] exactly one candidate basis: not the column too of the
        // table the first two name.
        if matches!(self.local(cs), Ok(Some(_))) {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §6.6 SR9c] {first}.{second}.{} is both a column of table {first}.{second} and a field of column {second}",
                cs.column
            )));
        }
        let bound = self.column_expr(&basis, scope)?;
        field_of(bound, &cs.column).map(Some).or_at(cs.at)
    }
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §7.16 SR11] `<all fields reference>` `value`.*
    /// \[AS ( `names` )\]: VEP.F1 AS N1, ..., one item for each field of its
    /// row type, as many names as fields \[SR11a\], each else named as its
    /// field (UV087).
    pub(super) fn all_fields(&self, value: &ValueExpression, names: Option<&[String]>, at: usize) -> Result<Vec<SelectItem>> {
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
    pub(super) fn row_asterisk(&self, q: &TableName, table: bool) -> Result<Option<Vec<SelectItem>>> {
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

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §14.15 SR11a, §15.5 GR3b i] The new value of
    /// the column `col` of `sc`, whose `<update target>` is its element at
    /// `index`: its array with that element its `<update source>`, stored
    /// in the element type [§9.2], at a position from 1 to the maximum
    /// cardinality. DEFAULT is the default of a column, not of an element
    /// [§6.5 SR3].
    pub(super) fn element_assignment(
        &self,
        (sc, col): (&SetClause, &ColumnDesc),
        index: &ValueExpression,
        no_set_function: &'static str,
    ) -> Result<Expr> {
        let DataType::Array(a) = &col.data_type else {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §14.15 SR11a] an element of column {} is set, and it is not an array but {}",
                col.name, col.data_type
            )));
        };
        let scope = Scope::Rows(no_set_function);
        let value = match &sc.value {
            UpdateSource::Null => Expr::Const(Value::Null),
            UpdateSource::Default => {
                return Err(rule(format!(
                    "[ISO/IEC 9075-2:2023 §6.5 SR3] DEFAULT is a column's default, not that of an element of {}",
                    col.name
                )));
            }
            UpdateSource::Value(v) => {
                let (e, t) = self.typed(v, ParameterType::Of(a.element.clone()), scope)?;
                if !assignable(&a.element, &t) {
                    return Err(rule(format!(
                        "[ISO/IEC 9075-2:2023 §14.15 SR11a, §9.2 SR2] a {t} value is not assigned to an element of {}",
                        col.data_type
                    )));
                }
                super::datetime::stored(e, t, &a.element, self.now)
            }
        };
        let i = self.position(index, "§14.15 SR11a, §6.25 SR2", scope)?;
        let column = ColumnSpecification { qualifier: None, column: sc.column.clone(), catalog: None, at: sc.at };
        let (array, _) = self.column_expr(&column, scope)?;
        Ok(call(AssignElement(col.data_type.clone(), a.max), vec![array, i, value]))
    }
}

/// [ISO/IEC 9075-2:2023 §6.16] The field `name` of `bound`, a value of a
/// row type \[SR2\]: of its type \[SR5\], none if the row is null \[GR2a\].
pub(super) fn field_of((row, row_type): (Expr, DataType), name: &str) -> Result<(Expr, DataType)> {
    let DataType::Row(described) = &row_type else {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.16 SR2] field {name} is referenced of a row, not of {row_type}")));
    };
    let Some((place, field)) = described.fields.iter().enumerate().find(|(_, field)| field.name == name) else {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.16 SR4] {row_type} has no field {name}")));
    };
    let data_type = field.data_type.clone();
    Ok((call(FieldOf(data_type.clone(), place), vec![row]), data_type))
}

/// `bound`, a value of a row type, spread into its fields [ISO/IEC
/// 9075-2:2023 §7.2 `<row value special case>`]: each a field reference
/// of it, and its type.
pub(super) fn spread((e, t): (Expr, DataType)) -> Vec<(Expr, DataType)> {
    let DataType::Row(r) = &t else { return vec![(e, t)] };
    r.fields.iter().enumerate().map(|(i, f)| (call(FieldOf(f.data_type.clone(), i), vec![e.clone()]), f.data_type.clone())).collect()
}
