//! Abstract syntax of `<JSON table>` [ISO/IEC 9075-2:2023 §7.11], a
//! `<table primary>` [§7.6]: its `<JSON API common syntax>` with the path
//! name of its row pattern, its columns, nested to any depth, its plan
//! and its `<JSON table error behavior>`.
//!
//! A clause not written is none here; the binder applies the Syntax Rules
//! that make one implicit. A column with a `<JSON representation>`, a
//! wrapper, a quotes behavior or EMPTY ARRAY or EMPTY OBJECT is read as a
//! `<JSON table formatted column definition>`, any other as a regular one,
//! and the binder checks each by its rules.

use super::{JsonCommon, JsonFormat, JsonValueBehavior, TypeName, ValueExpression};
use crate::json::{QueryBehavior, Quotes, TableErrorBehavior, Wrapper};

/// [§7.11] `<JSON table>` ::= JSON_TABLE ( `<JSON API common syntax>`
/// `<JSON table columns clause>` \[`<JSON table plan clause>`\]
/// \[`<JSON table error behavior>` ON ERROR\] ), then its `<correlation or
/// recognition>` [§7.6]: \[AS\] `<correlation name>` \[(`<derived column
/// list>`)\].
#[derive(Debug, Clone)]
pub struct JsonTable {
    /// The `<JSON API common syntax>`.
    pub common: JsonCommon,
    /// The `<JSON table path name>` after AS of the common syntax, and
    /// where it is written, if one is [§10.14 SR2b].
    pub name: Option<(String, usize)>,
    /// The `<JSON table column definition>`s of the `<JSON table columns
    /// clause>`, in order.
    pub columns: Vec<JsonTableColumn>,
    /// The `<JSON table plan clause>`, if written.
    pub plan: Option<JsonTablePlanClause>,
    /// The `<JSON table error behavior>`, if written.
    pub on_error: Option<TableErrorBehavior>,
    /// The `<correlation name>`.
    pub correlation: String,
    /// The `<derived column list>`, if written.
    pub derived: Option<Vec<String>>,
    /// Where JSON_TABLE is written.
    pub at: usize,
}

/// [§7.11] A `<JSON table column definition>`.
#[derive(Debug, Clone)]
pub enum JsonTableColumn {
    /// An ordinality, regular or formatted column.
    Column(JsonTableNamed),
    /// `<JSON table nested columns>`.
    Nested(Box<JsonTableNested>),
}

/// [§7.11] A `<JSON table column definition>` that names a column.
#[derive(Debug, Clone)]
pub struct JsonTableNamed {
    /// The `<column name>`.
    pub name: String,
    /// Where the `<column name>` is written.
    pub at: usize,
    /// What the column is.
    pub kind: JsonTableColumnKind,
}

/// [§7.11] What a named column of a `<JSON table>` is.
#[derive(Debug, Clone)]
pub enum JsonTableColumnKind {
    /// `<JSON table ordinality column definition>`: FOR ORDINALITY.
    Ordinality,
    /// `<JSON table regular column definition>`.
    Regular {
        /// The `<data type>`.
        data_type: TypeName,
        /// The `<JSON table column path specification>`, if written.
        path: Option<JsonTablePath>,
        /// The `<JSON table column empty behavior>`, if written.
        on_empty: Option<JsonValueBehavior>,
        /// The `<JSON table column error behavior>`, if written.
        on_error: Option<JsonValueBehavior>,
    },
    /// `<JSON table formatted column definition>`.
    Formatted(Box<JsonTableFormatted>),
}

/// [§7.11] A `<JSON table formatted column definition>`'s clauses.
#[derive(Debug, Clone)]
pub struct JsonTableFormatted {
    /// The `<data type>`.
    pub data_type: TypeName,
    /// The `<JSON representation>` after FORMAT, if written.
    pub format: Option<JsonFormat>,
    /// The `<JSON table column path specification>`, if written.
    pub path: Option<JsonTablePath>,
    /// The `<JSON table formatted column wrapper behavior>`, WITH without
    /// CONDITIONAL or UNCONDITIONAL read as UNCONDITIONAL \[SR1f vi\].
    pub wrapper: Option<Wrapper>,
    /// The `<JSON table formatted column quotes behavior>`, and where it
    /// is written.
    pub quotes: Option<(Quotes, usize)>,
    /// The `<JSON table formatted column empty behavior>`, if written.
    pub on_empty: Option<QueryBehavior>,
    /// The `<JSON table formatted column error behavior>`, if written.
    pub on_error: Option<QueryBehavior>,
}

/// [§7.11] A `<JSON path specification>` of a column or of nested
/// columns: its text and where it is written.
#[derive(Debug, Clone)]
pub struct JsonTablePath {
    /// The value of the `<character string literal>`.
    pub text: String,
    /// Where it is written.
    pub at: usize,
}

/// [§7.11] `<JSON table nested columns>` ::= NESTED \[PATH\] `<JSON table
/// nested path specification>` \[AS `<JSON table nested path name>`\]
/// `<JSON table columns clause>`.
#[derive(Debug, Clone)]
pub struct JsonTableNested {
    /// The `<JSON table nested path specification>`.
    pub path: JsonTablePath,
    /// The `<JSON table nested path name>` and where it is written, if
    /// one is.
    pub name: Option<(String, usize)>,
    /// The column definitions of its `<JSON table columns clause>`.
    pub columns: Vec<JsonTableColumn>,
}

/// [§7.11] `<JSON table plan clause>`.
#[derive(Debug, Clone)]
pub enum JsonTablePlanClause {
    /// `<JSON table specific plan>`: PLAN ( `<JSON table plan>` ).
    Specific(JsonTablePlan),
    /// `<JSON table default plan>`: PLAN DEFAULT ( ... ), its choices.
    Default {
        /// INNER, else OUTER, written or not.
        inner: bool,
        /// CROSS, else UNION, written or not.
        cross: bool,
    },
}

/// [§7.11] `<JSON table plan>`, and a `<JSON table plan primary>`, which
/// parentheses only group.
#[derive(Debug, Clone)]
pub enum JsonTablePlan {
    /// A `<JSON table path name>`, and where it is written.
    Name(String, usize),
    /// `<JSON table plan outer>` or `<JSON table plan inner>`: the path
    /// name of the parent, where it is written, INNER if written, and the
    /// `<JSON table plan primary>` of the children.
    Parent {
        /// The `<JSON table path name>`, the first operand.
        name: String,
        /// Where it is written.
        at: usize,
        /// INNER, else OUTER.
        inner: bool,
        /// The second operand.
        child: Box<JsonTablePlan>,
    },
    /// `<JSON table plan sibling>`: the `<JSON table plan primary>`s, and
    /// whether they are of CROSS, else of UNION.
    Siblings {
        /// CROSS, else UNION.
        cross: bool,
        /// The operands, two or more, in order.
        operands: Vec<JsonTablePlan>,
    },
}

impl JsonTable {
    /// Its operands, each a `<value expression>`, in the order the binder
    /// binds them: the context item's, each PASSING argument's, then the
    /// DEFAULT of each regular column, in the order the columns are
    /// written at every depth, its ON EMPTY's before its ON ERROR's.
    pub fn operands(&self) -> Vec<&ValueExpression> {
        let mut out = vec![&self.common.context.value];
        out.extend(self.common.passing.iter().map(|argument| &argument.input.value));
        let mut todo: Vec<&JsonTableColumn> = self.columns.iter().rev().collect();
        while let Some(column) = todo.pop() {
            match column {
                JsonTableColumn::Nested(nested) => todo.extend(nested.columns.iter().rev()),
                JsonTableColumn::Column(named) => out.extend(named.defaults()),
            }
        }
        out
    }
}

impl JsonTableNamed {
    /// The `<value expression>`s of its DEFAULT behaviors, ON EMPTY's
    /// first.
    pub fn defaults(&self) -> Vec<&ValueExpression> {
        let JsonTableColumnKind::Regular { on_empty, on_error, .. } = &self.kind else { return Vec::new() };
        [on_empty, on_error]
            .into_iter()
            .filter_map(|behavior| match behavior {
                Some(JsonValueBehavior::Default(value)) => Some(&**value),
                Some(JsonValueBehavior::Error | JsonValueBehavior::Null) | None => None,
            })
            .collect()
    }
}
