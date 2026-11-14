//! Abstract syntax of row pattern recognition [ISO/IEC 9075-2:2023 §4.19 to
//! §4.23]: the `<row pattern recognition clause>` of a table reference
//! [§7.6, §7.7], its `<row pattern measures>` [§7.8], the `<row pattern
//! common syntax>` it shares with a window [§7.9, §7.15], and the scalar
//! expressions only row pattern matching reads [§6.27, §6.31, §6.33].

use super::{GroupingColumn, SearchCondition, Subquery, TableReference, ValueExpression, ValueOrder};

/// [ISO/IEC 9075-2:2023 §7.6] A `<table primary>` of a `<row pattern
/// recognition clause and name>`: its input named as it is written, then
/// the clause, then the name and columns of the row pattern output table.
#[derive(Debug, Clone)]
pub struct RecognizedTable {
    /// The row pattern input table: a `<table or query name>` or `<derived
    /// table>`, its `<row pattern input name>` and `<row pattern input
    /// derived column list>` its correlation name and columns [SR6b i 3,
    /// 4].
    pub input: TableReference,
    /// The `<row pattern recognition clause>`.
    pub clause: RowPatternRecognition,
    /// The `<row pattern output name>`, if written [SR6b i 7].
    pub correlation: Option<String>,
    /// The `<row pattern output derived column list>`, if written [SR6b i
    /// 6].
    pub columns: Option<Vec<String>>,
    /// Where MATCH_RECOGNIZE begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §7.7] `<row pattern recognition clause>` ::=
/// MATCH_RECOGNIZE ( \[`<row pattern partition by>`\] \[`<row pattern order
/// by>`\] \[`<row pattern measures>`\] \[`<row pattern rows per match>`\]
/// `<row pattern common syntax>` )
#[derive(Debug, Clone)]
pub struct RowPatternRecognition {
    /// The `<row pattern partition column>`s, none if not written.
    pub partition: Vec<GroupingColumn>,
    /// The `<sort specification list>` of `<row pattern order by>`, none if
    /// not written.
    pub order: Vec<ValueOrder>,
    /// The `<row pattern measure definition>`s, none if not written.
    pub measures: Vec<Measure>,
    /// The `<row pattern rows per match>`, ONE ROW PER MATCH if not written
    /// \[SR5\].
    pub rows: RowsPerMatch,
    /// The `<row pattern common syntax>`.
    pub common: PatternCommon,
}

/// [ISO/IEC 9075-2:2023 §7.8] `<row pattern measure definition>` ::= `<row
/// pattern measure expression>` AS `<measure name>`
#[derive(Debug, Clone)]
pub struct Measure {
    /// The `<row pattern measure expression>`.
    pub expr: ValueExpression,
    /// The `<measure name>`.
    pub name: String,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §7.7] `<row pattern rows per match>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowsPerMatch {
    /// ONE ROW PER MATCH, written or implied.
    One,
    /// ALL ROWS PER MATCH and its `<row pattern empty match handling>`,
    /// SHOW EMPTY MATCHES if not written \[SR6\].
    All(EmptyMatches),
}

/// [ISO/IEC 9075-2:2023 §7.7] `<row pattern empty match handling>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EmptyMatches {
    /// SHOW EMPTY MATCHES.
    Show,
    /// OMIT EMPTY MATCHES.
    Omit,
    /// WITH UNMATCHED ROWS.
    Unmatched,
}

/// [ISO/IEC 9075-2:2023 §7.9] `<row pattern common syntax>` ::= \[AFTER
/// MATCH `<row pattern skip to>`\] \[`<row pattern initial or seek>`\]
/// PATTERN ( `<row pattern>` ) \[`<row pattern subset clause>`\] DEFINE
/// `<row pattern definition list>`
#[derive(Debug, Clone)]
pub struct PatternCommon {
    /// The `<row pattern skip to>`, SKIP PAST LAST ROW if not written
    /// \[SR23a\].
    pub skip: SkipTo,
    /// The `<row pattern initial or seek>`, if written: whether SEEK.
    pub seek: Option<bool>,
    /// The `<row pattern>`.
    pub pattern: RowPattern,
    /// The `<row pattern subset item>`s, none if not written.
    pub subsets: Vec<Subset>,
    /// The `<row pattern definition>`s.
    pub defines: Vec<Definition>,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §7.9] `<row pattern skip to>`, SKIP TO a variable
/// read as SKIP TO LAST \[SR23b\].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipTo {
    /// SKIP TO NEXT ROW.
    NextRow,
    /// SKIP PAST LAST ROW, written or implied.
    PastLastRow,
    /// SKIP TO FIRST `<row pattern skip to variable name>`, and where.
    First(String, usize),
    /// SKIP TO LAST, or SKIP TO, `<row pattern skip to variable name>`,
    /// and where.
    Last(String, usize),
}

/// [ISO/IEC 9075-2:2023 §7.9] A `<row pattern>`, each quantifier read as
/// `{ n, m }` or `{ n, }` with its reluctance \[SR19\].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RowPattern {
    /// A `<row pattern primary variable name>`, and where.
    Variable(String, usize),
    /// `^`: the start of a row pattern partition.
    Start,
    /// `$`: the end of a row pattern partition.
    End,
    /// `( )`: the empty pattern.
    Empty,
    /// A `<row pattern term>` of two or more factors, in order.
    Concat(Vec<RowPattern>),
    /// A `<row pattern alternation>` of two or more terms, in order.
    Alternation(Vec<RowPattern>),
    /// A `<row pattern primary>` and its `<row pattern quantifier>`.
    Quantified(Box<Quantified>),
    /// `{-` `<row pattern>` `-}`: rows matched left out of ALL ROWS PER
    /// MATCH's output.
    Excluded(Box<RowPattern>),
    /// PERMUTE ( `<row pattern>` \[, ...\] ) \[SR21\].
    Permute(Vec<RowPattern>),
}

/// [ISO/IEC 9075-2:2023 §7.9] A `<row pattern factor>` with a quantifier.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Quantified {
    /// The `<row pattern primary>`.
    pub pattern: RowPattern,
    /// The least number of times it repeats.
    pub min: u32,
    /// The most, none if unbounded.
    pub max: Option<u32>,
    /// A reluctant quantifier, its last `?`.
    pub reluctant: bool,
    /// Where the quantifier begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §7.9] `<row pattern subset item>` ::= `<row pattern
/// subset item variable name>` = ( `<row pattern subset rhs>` )
#[derive(Debug, Clone)]
pub struct Subset {
    /// The union row pattern variable it defines.
    pub name: String,
    /// The primary row pattern variables it is the union of, in order.
    pub variables: Vec<String>,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §7.9] `<row pattern definition>` ::= `<row pattern
/// definition variable name>` AS `<row pattern definition search
/// condition>`
#[derive(Debug, Clone)]
pub struct Definition {
    /// The primary row pattern variable it defines.
    pub name: String,
    /// The `<row pattern definition search condition>`.
    pub condition: SearchCondition,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §7.15] A window's row pattern recognition: the
/// `<row pattern measures>` before its frame and the `<row pattern common
/// syntax>` after.
#[derive(Debug, Clone)]
pub struct WindowRecognition {
    /// The `<row pattern measure definition>`s, none if not written.
    pub measures: Vec<Measure>,
    /// The `<row pattern common syntax>`.
    pub common: PatternCommon,
}

/// [ISO/IEC 9075-2:2023 §6.27] A `<row pattern navigation operation>`, read
/// as its `<row pattern navigation: compound>` \[SR7, SR8\]: a logical
/// step to a row mapped to a row pattern variable, then a physical step
/// from it.
#[derive(Debug, Clone)]
pub struct Navigation {
    /// The `<value expression>` evaluated in the row navigated to.
    pub expr: ValueExpression,
    /// FIRST or LAST and its offset, if written; none of PREV or NEXT
    /// alone, read as RUNNING LAST ( VE, 0 ) \[SR7\].
    pub logical: Option<Logical>,
    /// PREV or NEXT and its offset, if written; none of FIRST or LAST alone,
    /// read as PREV ( ..., 0 ) \[SR8\].
    pub physical: Option<Physical>,
    /// Where it begins in the statement.
    pub at: usize,
}

/// [ISO/IEC 9075-2:2023 §6.27] A logical step: \[`<running or final>`\]
/// `<first or last>` and its `<logical offset>`.
#[derive(Debug, Clone)]
pub struct Logical {
    /// FINAL rather than RUNNING, written or implied \[SR5a\].
    pub final_: bool,
    /// LAST rather than FIRST.
    pub last: bool,
    /// The `<logical offset>`, if written; 0 if not \[SR5c ii\].
    pub offset: Option<Box<ValueExpression>>,
}

/// [ISO/IEC 9075-2:2023 §6.27] A physical step: `<prev or next>` and its
/// `<physical offset>`.
#[derive(Debug, Clone)]
pub struct Physical {
    /// NEXT rather than PREV.
    pub next: bool,
    /// The `<physical offset>`, if written; 1 if not \[SR6b\].
    pub offset: Option<Box<ValueExpression>>,
}

/// [ISO/IEC 9075-2:2023 §6.9] `<running or final>` written before an
/// `<aggregate function>` in row pattern matching.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Semantics {
    /// RUNNING.
    Running,
    /// FINAL.
    Final,
}

impl RowPattern {
    /// Whether it contains `{-` `<row pattern>` `-}` [ISO/IEC 9075-2:2023
    /// §7.9 SR22]. Walked with a stack of its own rather than by recursion,
    /// so no pattern is too deep for it.
    #[must_use]
    pub fn excludes(&self) -> bool {
        let mut stack = vec![self];
        while let Some(p) = stack.pop() {
            match p {
                RowPattern::Excluded(_) => return true,
                RowPattern::Variable(..) | RowPattern::Start | RowPattern::End | RowPattern::Empty => {}
                RowPattern::Concat(list) | RowPattern::Alternation(list) | RowPattern::Permute(list) => stack.extend(list),
                RowPattern::Quantified(q) => stack.push(&q.pattern),
            }
        }
        false
    }
}

impl PatternCommon {
    /// Calls `f` with each subquery its definitions hold, not those nested
    /// in them.
    pub fn for_each_subquery<'a>(&'a self, f: &mut dyn FnMut(&'a Subquery)) {
        for d in &self.defines {
            d.condition.for_each_subquery(f);
        }
    }
}

impl RowPatternRecognition {
    /// Calls `f` with each subquery its sort keys, measures and definitions
    /// hold, not those nested in them.
    pub fn for_each_subquery<'a>(&'a self, f: &mut dyn FnMut(&'a Subquery)) {
        self.order.iter().for_each(|o| o.key.for_each_subquery(f));
        self.measures.iter().for_each(|m| m.expr.for_each_subquery(f));
        self.common.for_each_subquery(f);
    }
}

impl WindowRecognition {
    /// Calls `f` with each subquery its measures and definitions hold, not
    /// those nested in them.
    pub fn for_each_subquery<'a>(&'a self, f: &mut dyn FnMut(&'a Subquery)) {
        self.measures.iter().for_each(|m| m.expr.for_each_subquery(f));
        self.common.for_each_subquery(f);
    }
}
