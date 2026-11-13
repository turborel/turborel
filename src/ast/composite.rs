//! Abstract syntax of the expressions of arrays and rows [ISO/IEC
//! 9075-2:2023 §6.16, §6.25, §6.49, §7.1], each a `ValueExpression::
//! Composite`, and of the ordering of an array aggregate [§10.9].

use super::{SortSpecification, Subquery, ValueExpression};

/// An expression of arrays and rows.
#[derive(Debug, Clone)]
pub enum Composite {
    /// [ISO/IEC 9075-2:2023 §6.49] `<array value constructor by
    /// enumeration>` ::= ARRAY `[` `<array element list>` `]`, or with no
    /// element, an `<empty specification>` [§6.5], ARRAY `[` `]`.
    Array(Vec<ValueExpression>),
    /// [ISO/IEC 9075-2:2023 §6.49] `<array value constructor by query>`
    /// ::= ARRAY ( `<query expression>` \[`<order by clause>`\] ).
    ArrayQuery(Box<Subquery>, Vec<SortSpecification>),
    /// [ISO/IEC 9075-2:2023 §7.1] `<explicit row value constructor>` ::=
    /// ROW ( `<row value constructor element list>` ) | (
    /// `<row value constructor element>` , `<row value constructor element
    /// list>` ).
    Row(Vec<ValueExpression>),
    /// [ISO/IEC 9075-2:2023 §6.25] `<array element reference>` ::= `<array
    /// value expression>` `[` `<numeric value expression>` `]`.
    Element(Box<ValueExpression>, Box<ValueExpression>),
    /// [ISO/IEC 9075-2:2023 §6.16] `<field reference>` ::= `<value
    /// expression primary>` . `<field name>`.
    Field(Box<ValueExpression>, String),
    /// [ISO/IEC 9075-2:2023 §6.6 SR9b iv 2, SR11] An `<identifier chain>`
    /// of four or more `<identifier>`s whose first is not the catalog's
    /// name: the second a column of a row type of the table the first
    /// names, the rest its fields in turn. Of fewer, or of four whose first
    /// may be the catalog's name, it is a `ValueExpression::Column`.
    Chain(Vec<String>),
}

impl Composite {
    /// Calls `f` with each value expression it is made of; not those of
    /// its subquery.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match self {
            Composite::Array(values) | Composite::Row(values) => values.iter().for_each(f),
            Composite::ArrayQuery(..) | Composite::Chain(_) => {}
            Composite::Element(array, index) => {
                f(array);
                f(index);
            }
            Composite::Field(value, _) => f(value),
        }
    }

    /// Its subquery, if it has one.
    pub fn subquery(&self) -> Option<&Subquery> {
        if let Composite::ArrayQuery(s, _) = self { Some(s) } else { None }
    }
}

/// [ISO/IEC 9075-2:2023 §10.10] A `<sort specification>` of an `<array
/// aggregate function>` [§10.9]: its `<sort key>`, a value expression,
/// its `<collate clause>` among it.
#[derive(Debug, Clone)]
pub struct ValueOrder {
    /// The `<sort key>`.
    pub key: ValueExpression,
    /// DESC was written.
    pub descending: bool,
    /// [ISO/IEC 9075-2:2023 §10.10] Its `<null ordering>`, if written:
    /// whether NULLS FIRST.
    pub nulls_first: Option<bool>,
}
