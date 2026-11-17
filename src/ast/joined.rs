//! Abstract syntax of a `<joined table>` [ISO/IEC 9075:1992 §7.5]: its
//! operands, its kind, its join specification and the partitioning of a
//! `<partitioned join table>` [ISO/IEC 9075-2:2023 §7.10].

use super::{ColumnSpecification, SearchCondition, TableReference};

/// [ISO/IEC 9075:1992 §7.5] `<joined table>` ::= `<cross join>` |
/// `<qualified join>` | (`<joined table>`)
#[derive(Debug, Clone)]
pub struct JoinedTable {
    /// The first `<table reference>`.
    pub left: TableReference,
    /// The second `<table reference>`.
    pub right: TableReference,
    /// CROSS, or the `<join type>`: INNER if none is written \[SR3\].
    pub kind: JoinKind,
    /// NATURAL was written.
    pub natural: bool,
    /// The `<join specification>`, if written.
    pub on: Option<JoinSpecification>,
    /// Where its join is written in the statement: the first key word
    /// after `left`.
    pub at: usize,
    /// The `<partitioned join column reference list>` of each operand, the
    /// first's then the second's, if it is a `<partitioned join table>`
    /// [ISO/IEC 9075-2:2023 §7.10].
    pub partitioned: [Option<Partitioning>; 2],
}

/// [ISO/IEC 9075-2:2023 §7.10] PARTITION BY `<partitioned join column
/// reference list>`: the `<column reference>`s an operand of a join is
/// partitioned by, and where PARTITION is written.
#[derive(Debug, Clone)]
pub struct Partitioning {
    /// The `<partitioned join column reference>`s, in order.
    pub columns: Vec<ColumnSpecification>,
    /// Where PARTITION begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075:1992 §7.5] What a join is: CROSS, or a `<join type>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JoinKind {
    /// CROSS JOIN.
    Cross,
    /// INNER.
    Inner,
    /// LEFT \[OUTER\].
    Left,
    /// RIGHT \[OUTER\].
    Right,
    /// FULL \[OUTER\].
    Full,
    /// UNION.
    Union,
}

/// [ISO/IEC 9075:1992 §7.5] `<join specification>` ::= `<join condition>`
/// | `<named columns join>`
#[derive(Debug, Clone)]
pub enum JoinSpecification {
    /// ON `<search condition>`.
    On(SearchCondition),
    /// USING (`<join column list>`).
    Using(Vec<String>),
}
