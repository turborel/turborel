//! \[DESIGN\] The plan form, format 1 (RULES.md D275): the logical plan as
//! an optimizer rule of a plug-in reads and writes it, apart from the
//! planner's own types, so a program built against one release rewrites
//! another's plans.
//!
//! A plan is a tree of nodes. Each node has a stable operator code, the
//! planner's id of it (0 for a node the rule made), whether it is kept,
//! its output columns with their types, the planner's estimates of its
//! rows and of a row's width, a body, and its inputs. A kept node is
//! rebuilt from the planner's node of its id, its body a label for people,
//! its inputs open (each must keep the columns it had). A node the form
//! cannot say whole is shown kept, so a rule may move it but not open it.
//!
//! ```text
//! plan:        format u32 (1) | node
//! node:        op u8 | id u32 | kept u8 | columns: count u32 | (name str | type str)...
//!              | rows f64 | width f64 | body | inputs: count u32 | node...
//! body, kept:  label str
//! 1  SCAN      table str | statistics: count u32 | (distinct f64 | nulls f64 | u8 0, or u8 1 | low value | high value)...
//! 2  FILTER    condition expr
//! 3  PROJECT   count u32 | expr...
//! 4  JOIN      kind u8 (0 cross, 1 left, 2 right, 3 full) | u8 0, or u8 1 | condition expr
//! 6  SORT      count u32 | (column u32 | descending u8 | nulls first u8)...
//! 8  DISTINCT  (nothing)
//! 9  SET       op u8 (1 union, 2 except, 3 intersect) | all u8
//! 10 VALUES    rows: count u32 | (count u32 | expr...)...
//! 5 AGGREGATE, 7 LIMIT, 11 SYSTEM, 12 RECURSIVE, 13 WORK TABLE, 14 TRANSITION,
//! 15 UNNEST, 16 WINDOW, 17 RECOGNIZE, 18 POLYMORPHIC: kept alone
//! expr:        1 COLUMN   index u32
//!              2 LITERAL  type str | value
//!              3 CALL     name str | count u32 | expr...
//!              4 OPERATOR op u8 | count u32 | expr...
//!              5 CAST     type str | expr
//!              6 OPAQUE   id u32 | count u32 | column u32...
//! operator:    1 AND, 2 OR, 3 NOT, 4 =, 5 <>, 6 <, 7 >, 8 <=, 9 >=, 10 IS NULL,
//!              11 IS NOT NULL, 12 +, 13 -, 14 *, 15 /, 16 negation
//! ```
//!
//! A type is a `<data type>` as SQL writes it, empty where the planner
//! does not know it. A number not known is negative. A value is one of
//! protocol 1 (`values.rs`). An opaque expression is one of the planner's
//! the form cannot say, over the columns it names, which a rule may name
//! again as other columns of the same row but never open. `from.rs`
//! makes the form of a plan, and `back.rs` and `exprs.rs` the plan of a
//! form, checking it whole.

mod back;
mod codec;
mod estimate;
mod exprs;
mod from;
mod typed;

pub use back::plan_of;
pub use codec::{decode, encode};
pub use from::{Kept, form_of};

use crate::value::Value;

/// The plan form's format, version 1 (VERSIONING.md).
pub const FORMAT: u32 = 1;

/// \[DESIGN\] The deepest a plan form nests, nodes and expressions: past
/// it a form is refused (RULES.md R-CF-6), as a plan nested so deep is
/// one no rule needs.
pub const MAX_DEPTH: usize = 400;

/// A plan form: its root.
#[derive(Debug, Clone, PartialEq)]
pub struct Form {
    /// The root node.
    pub root: Node,
}

/// An output column: its name, and its type, empty if not known.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    /// Its name, for people.
    pub name: String,
    /// Its `<data type>`, as SQL writes it; empty if not known.
    pub data_type: String,
}

/// What the database keeps of a column of a table it scans.
#[derive(Debug, Clone, PartialEq)]
pub struct ColumnStatistics {
    /// Its distinct non-null values; negative if not known.
    pub distinct: f64,
    /// The share of its values that are null; negative if not known.
    pub nulls: f64,
    /// Its least and greatest values, if known.
    pub bounds: Option<(Value, Value)>,
}

/// A node of a plan form.
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    /// Its operator.
    pub op: Op,
    /// The planner's id of it; 0 of a node a rule made.
    pub id: u32,
    /// Whether it is rebuilt from the planner's node `id`.
    pub kept: bool,
    /// Its output columns.
    pub columns: Vec<Column>,
    /// The rows it is estimated to give; negative if not known.
    pub rows: f64,
    /// A row's estimated width in bytes; negative if not known.
    pub width: f64,
    /// What it does.
    pub body: Body,
    /// Its inputs, in order.
    pub inputs: Vec<Node>,
}

/// The operators of the plan form, each with its code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    /// A base table's rows.
    Scan,
    /// The rows a condition is true of.
    Filter,
    /// A row of expressions of each row.
    Project,
    /// A join, cross or outer.
    Join,
    /// Groups and their set functions.
    Aggregate,
    /// The rows in an order.
    Sort,
    /// OFFSET and FETCH FIRST.
    Limit,
    /// Duplicates removed.
    Distinct,
    /// UNION, EXCEPT or INTERSECT.
    Set,
    /// A table value constructor.
    Values,
    /// A system table's rows.
    System,
    /// A recursive query's rows.
    Recursive,
    /// A recursive query's rows of the iteration before.
    WorkTable,
    /// A trigger's transition table.
    Transition,
    /// A collection derived table.
    Unnest,
    /// Window functions.
    Window,
    /// Row pattern recognition.
    Recognize,
    /// A polymorphic table function.
    Polymorphic,
}

impl Op {
    /// Every operator, in the order of their codes.
    pub const ALL: [Op; 18] = [
        Op::Scan,
        Op::Filter,
        Op::Project,
        Op::Join,
        Op::Aggregate,
        Op::Sort,
        Op::Limit,
        Op::Distinct,
        Op::Set,
        Op::Values,
        Op::System,
        Op::Recursive,
        Op::WorkTable,
        Op::Transition,
        Op::Unnest,
        Op::Window,
        Op::Recognize,
        Op::Polymorphic,
    ];

    /// Its code: its place in `ALL`, from 1.
    pub fn code(self) -> u8 {
        let place = Op::ALL.iter().position(|o| *o == self).unwrap_or(0);
        u8::try_from(place).unwrap_or(u8::MAX).saturating_add(1)
    }

    /// The operator of code `code`, if any.
    pub fn of_code(code: u8) -> Option<Op> {
        Op::ALL.into_iter().find(|o| o.code() == code)
    }

    /// Whether a rule may make a node of it, its body said whole; else
    /// it is kept alone.
    pub fn made(self) -> bool {
        match self {
            Op::Scan | Op::Filter | Op::Project | Op::Join | Op::Sort | Op::Distinct | Op::Set | Op::Values => true,
            Op::Aggregate
            | Op::Limit
            | Op::System
            | Op::Recursive
            | Op::WorkTable
            | Op::Transition
            | Op::Unnest
            | Op::Window
            | Op::Recognize
            | Op::Polymorphic => false,
        }
    }
}

/// What a node does, as its operator says.
#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    /// Of a kept node: a label, for people.
    Kept(String),
    /// A base table, "SCHEMA.TABLE", and what the database keeps of each
    /// column (none of a table it keeps nothing of).
    Scan {
        /// The table.
        table: String,
        /// Each column's statistics.
        statistics: Vec<ColumnStatistics>,
    },
    /// The condition.
    Filter(Expression),
    /// One expression per output column.
    Project(Vec<Expression>),
    /// A join's kind, and an outer join's condition.
    Join {
        /// Cross, left, right or full.
        kind: JoinKind,
        /// The `<join condition>` of an outer join.
        condition: Option<Expression>,
    },
    /// The sort keys, most significant first.
    Sort(Vec<Key>),
    /// Nothing.
    Distinct,
    /// The set operator, and whether ALL.
    Set {
        /// Union, except or intersect.
        op: SetKind,
        /// ALL, which keeps duplicates.
        all: bool,
    },
    /// The rows' expressions, over no row.
    Values(Vec<Vec<Expression>>),
}

/// A join's kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    /// The extended Cartesian product.
    Cross,
    /// LEFT OUTER.
    Left,
    /// RIGHT OUTER.
    Right,
    /// FULL OUTER.
    Full,
}

/// A set operator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetKind {
    /// UNION.
    Union,
    /// EXCEPT.
    Except,
    /// INTERSECT.
    Intersect,
}

/// A sort key: a column of the input, and its direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Key {
    /// The column, from 0.
    pub column: u32,
    /// DESC.
    pub descending: bool,
    /// NULLS FIRST.
    pub nulls_first: bool,
}

/// An expression of the plan form, over its node's input row.
#[derive(Debug, Clone, PartialEq)]
pub enum Expression {
    /// The column of the input row at this place, from 0.
    Column(u32),
    /// A value of a type, empty if the null value of none.
    Literal {
        /// Its `<data type>`.
        data_type: String,
        /// Its value.
        value: Value,
    },
    /// A scalar function, by its name.
    Call {
        /// Its name.
        name: String,
        /// Its arguments.
        args: Vec<Expression>,
    },
    /// An operator of its operands.
    Operator {
        /// The operator.
        op: Operator,
        /// Its operands.
        args: Vec<Expression>,
    },
    /// CAST to a type.
    Cast {
        /// The `<data type>`.
        data_type: String,
        /// The value cast.
        arg: Box<Expression>,
    },
    /// One of the planner's, over the columns it names.
    Opaque {
        /// The planner's id of it.
        id: u32,
        /// The columns it reads, in the planner's order.
        columns: Vec<u32>,
    },
}

/// The operators of expressions, each with its code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// AND.
    And,
    /// OR.
    Or,
    /// NOT.
    Not,
    /// =.
    Eq,
    /// <>.
    Ne,
    /// <.
    Lt,
    /// >.
    Gt,
    /// <=.
    Le,
    /// >=.
    Ge,
    /// IS NULL.
    IsNull,
    /// IS NOT NULL.
    IsNotNull,
    /// +.
    Add,
    /// -.
    Sub,
    /// *.
    Mul,
    /// /.
    Div,
    /// Monadic minus.
    Negate,
}

impl Operator {
    /// Every operator, in the order of their codes.
    pub const ALL: [Operator; 16] = [
        Operator::And,
        Operator::Or,
        Operator::Not,
        Operator::Eq,
        Operator::Ne,
        Operator::Lt,
        Operator::Gt,
        Operator::Le,
        Operator::Ge,
        Operator::IsNull,
        Operator::IsNotNull,
        Operator::Add,
        Operator::Sub,
        Operator::Mul,
        Operator::Div,
        Operator::Negate,
    ];

    /// Its code: its place in `ALL`, from 1.
    pub fn code(self) -> u8 {
        let place = Operator::ALL.iter().position(|o| *o == self).unwrap_or(0);
        u8::try_from(place).unwrap_or(u8::MAX).saturating_add(1)
    }

    /// The operator of code `code`, if any.
    pub fn of_code(code: u8) -> Option<Operator> {
        Operator::ALL.into_iter().find(|o| o.code() == code)
    }
}

impl Node {
    /// Calls `f` with it and each node beneath it, itself first.
    pub fn walk<'a>(&'a self, f: &mut dyn FnMut(&'a Node)) {
        f(self);
        for input in &self.inputs {
            input.walk(f);
        }
    }
}

#[cfg(test)]
mod tests;
