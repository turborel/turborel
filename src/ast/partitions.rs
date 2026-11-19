//! \[EXTENSION\] Abstract syntax of partitioning (RULES.md D153): a table
//! definition's `<partition by clause>` and `<partition of clause>`, and
//! the bound of a partition, which ALTER TABLE ... ATTACH PARTITION
//! writes too.

use super::{Literal, TableName};

/// `<partition key part>` ::= `<column name>` | ( `<value expression>` )
#[derive(Debug, Clone)]
pub enum PartitionKeyPart {
    /// A column, and where its name begins.
    Column(String, usize),
    /// An expression, as written, and where it begins.
    Expression(String, usize),
}

/// `<partition by clause>` ::= PARTITION BY `<partition strategy name>` (
/// `<partition key part>` \[{, `<partition key part>`}...\] )
#[derive(Debug, Clone)]
pub struct PartitionBy {
    /// The strategy's name: RANGE, LIST, HASH or an `<identifier>`.
    pub strategy: String,
    /// Where the strategy's name begins.
    pub at: usize,
    /// The key's parts, in order.
    pub parts: Vec<PartitionKeyPart>,
}

/// A value of a partition bound: `<literal>`, NULL, MINVALUE or MAXVALUE.
#[derive(Debug, Clone)]
pub enum BoundDatum {
    /// A `<literal>`.
    Literal(Literal),
    /// NULL.
    Null,
    /// MINVALUE.
    MinValue,
    /// MAXVALUE.
    MaxValue,
}

/// `<partition bound specification>` ::= FOR VALUES `<partition bound>` |
/// DEFAULT
#[derive(Debug, Clone)]
pub enum BoundSpec {
    /// DEFAULT.
    Default,
    /// FROM ( `<range datum>`... ) TO ( `<range datum>`... ).
    Range {
        /// The lower bound's values.
        from: Vec<BoundDatum>,
        /// The upper bound's values.
        to: Vec<BoundDatum>,
    },
    /// IN ( `<list datum>`... ): each key listed, one value per key part.
    List(Vec<Vec<BoundDatum>>),
    /// WITH ( `<bound option>`... ): each option's name and value.
    With(Vec<(String, BoundDatum)>),
}

/// `<partition of clause>` ::= PARTITION OF `<parent table name>`
/// `<partition bound specification>`
#[derive(Debug, Clone)]
pub struct PartitionOfClause {
    /// The partitioned table it is a partition of.
    pub parent: TableName,
    /// The rows it holds.
    pub bound: BoundSpec,
    /// Where the bound begins.
    pub at: usize,
}

/// A table definition's partitioning: of a partitioned table its PARTITION
/// BY, of a partition its PARTITION OF, of a partition partitioned in
/// turn both.
#[derive(Debug, Clone, Default)]
pub struct TablePartitioning {
    /// Its `<partition by clause>`, if written.
    pub by: Option<PartitionBy>,
    /// Its `<partition of clause>`, if written.
    pub of: Option<PartitionOfClause>,
}
