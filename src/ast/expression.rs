//! Abstract syntax of `<search condition>` [X3.135-1986 §5.18; ISO/IEC
//! 9075:1992 §8.12], its predicates [ISO/IEC 9075:1992 §8], and `<value
//! expression>` [X3.135-1986 §5.9; ISO/IEC 9075:1992 §6.11].
//!
//! Passes over them reach the expressions they are made of through
//! `ValueExpression::for_each_child` and `SearchCondition::for_each_value`,
//! which stop at a subquery, a query of its own.

use super::{
    Composite, Navigation, NestedWindow, QueryExpression, SetFunctionSpecification, TableName, TypeName, ValueSpecification, WindowFunction,
};
use crate::catalog::MatchType;
use crate::value::datetime::{Field, Moment};
use crate::value::interval::Qualifier;
use crate::value::{ArithOp, CompOp};

/// [ISO/IEC 9075:1992 §6.8] A `<datetime value function>`: CURRENT_DATE,
/// CURRENT_TIME or CURRENT_TIMESTAMP, or [ISO/IEC 9075-2:2023 §6.43]
/// LOCALTIME or LOCALTIMESTAMP, of a precision if written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DatetimeFunction {
    /// Which: of a date, a time or a timestamp.
    pub moment: Moment,
    /// The `<time precision>` or `<timestamp precision>`, if written.
    pub precision: Option<u32>,
    /// Whether it is LOCALTIME or LOCALTIMESTAMP, WITHOUT TIME ZONE
    /// [§6.43 SR2, SR3].
    pub local: bool,
}

/// [ISO/IEC 9075:1992 §6.6] An `<extract field>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExtractField {
    /// A `<datetime field>`.
    Datetime(Field),
    /// TIMEZONE_HOUR.
    TimezoneHour,
    /// TIMEZONE_MINUTE.
    TimezoneMinute,
}

// ---------------------------------------------------------------------------
// [X3.135-1986 §5.18] <search condition>
// ---------------------------------------------------------------------------

/// [X3.135-1986 §5.18] `<search condition>` ::= `<boolean term>` | `<search condition>` OR `<boolean term>`
/// `<boolean term>`   ::= `<boolean factor>` | `<boolean term>` AND `<boolean factor>`
/// `<boolean factor>` ::= \[NOT\] `<boolean test>`
/// `<boolean test>` ::= `<boolean primary>` \[IS \[NOT\] `<truth value>`\] [ISO/IEC 9075:1992 §8.12]
/// `<boolean primary>` ::= `<predicate>` | (`<search condition>`)
#[derive(Debug, Clone)]
pub enum SearchCondition {
    /// OR.
    Or(Box<SearchCondition>, Box<SearchCondition>),
    /// AND.
    And(Box<SearchCondition>, Box<SearchCondition>),
    /// NOT.
    Not(Box<SearchCondition>),
    /// IS \[NOT\] TRUE, FALSE or UNKNOWN: the truth value, and whether NOT
    /// was written.
    Is(Box<SearchCondition>, TruthValue, bool),
    /// A `<predicate>`, and where it begins.
    Predicate(Box<Predicate>, usize),
    /// [ISO/IEC 9075-2:2023 §6.46] A `<boolean predicand>`: a value
    /// expression of type BOOLEAN \[SR3\], and where it begins.
    Predicand(Box<ValueExpression>, usize),
}

/// [ISO/IEC 9075:1992 §8.12] `<truth value>` ::= TRUE | FALSE | UNKNOWN
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TruthValue {
    /// TRUE.
    True,
    /// FALSE.
    False,
    /// UNKNOWN.
    Unknown,
}

/// [X3.135-1986 §5.24; ISO/IEC 9075:1992 §7.11] `<subquery>` ::= (`<query
/// expression>`): of one column, or of a row's degree.
#[derive(Debug, Clone)]
pub struct Subquery {
    /// The query.
    pub query: QueryExpression,
    /// Where it begins in the statement: a character offset, for the
    /// positions of errors (`SqlError::or_at`).
    pub at: usize,
}

/// [ISO/IEC 9075:1992 §7.1] `<row value constructor>`: one `<row value
/// constructor element>`, or a list of them in parentheses; a `<row
/// subquery>` [§7.11] is one element, a `<scalar subquery>`, of the
/// subquery's degree.
pub type RowValue = Vec<ValueExpression>;

/// [X3.135-1986 §5.10; ISO/IEC 9075:1992 §8.1] `<predicate>`.
#[derive(Debug, Clone)]
pub enum Predicate {
    /// [§8.2] `<comparison predicate>` ::= `<row value constructor>` `<comp op>` `<row value constructor>`
    Comparison(RowValue, CompOp, RowValue),
    /// [§8.11] `<overlaps predicate>` ::= `<row value constructor>`
    /// OVERLAPS `<row value constructor>`
    Overlaps(RowValue, RowValue),
    /// [ISO/IEC 9075-2:2023 §8.20] `<period predicate>`: its operator, and
    /// its `<period predicand 1>` and second operand.
    Period(super::PeriodOperator, super::PeriodOperand, super::PeriodOperand),
    /// [§8.3] `<between predicate>` ::= `<row value constructor>` \[NOT\]
    /// BETWEEN `<row value constructor>` AND `<row value constructor>`
    Between {
        /// The row tested.
        value: RowValue,
        /// The least it may be.
        low: RowValue,
        /// The greatest it may be.
        high: RowValue,
        /// NOT BETWEEN was written.
        not: bool,
    },
    /// [§8.4] `<in predicate>` with an `<in value list>`.
    InList {
        /// The row tested.
        value: RowValue,
        /// The `<value expression>`s it may be.
        list: Vec<ValueExpression>,
        /// NOT IN was written.
        not: bool,
    },
    /// [§8.4] `<in predicate>` with a `<table subquery>`.
    InSubquery {
        /// The row tested.
        value: RowValue,
        /// The subquery.
        subquery: Box<Subquery>,
        /// NOT IN was written.
        not: bool,
    },
    /// [§8.5] `<like predicate>` ::= `<match value>` \[NOT\] LIKE `<pattern>`
    /// \[ESCAPE `<escape character>`\], each a `<character value expression>`
    Like {
        /// The `<match value>`.
        value: Box<ValueExpression>,
        /// The `<pattern>`.
        pattern: Box<ValueExpression>,
        /// The `<escape character>`, if one was given.
        escape: Option<Box<ValueExpression>>,
        /// NOT LIKE was written.
        not: bool,
    },
    /// [ISO/IEC 9075-2:2023 §8.7] `<regex like predicate>` ::= `<row value
    /// predicand>` \[NOT\] LIKE_REGEX `<XQuery pattern>` \[FLAG `<XQuery
    /// option flag>`\], each a `<character value expression>`
    LikeRegex {
        /// The value matched, CVE.
        value: Box<ValueExpression>,
        /// The `<XQuery pattern>`.
        pattern: Box<ValueExpression>,
        /// The `<XQuery option flag>`, if one was given.
        flag: Option<Box<ValueExpression>>,
        /// NOT LIKE_REGEX was written.
        not: bool,
    },
    /// [ISO/IEC 9075-2:2023 §8.23] `<JSON exists predicate>` ::=
    /// JSON_EXISTS ( `<JSON API common syntax>` \[`<JSON exists error
    /// behavior>` ON ERROR\] ).
    JsonExists(Box<super::JsonOperator>),
    /// [ISO/IEC 9075-2:2023 §8.22] `<JSON predicate>` ::= `<JSON predicate
    /// input>` \[`<JSON input clause>`\] IS \[NOT\] JSON \[`<JSON predicate
    /// type constraint>`\] \[`<JSON key uniqueness constraint>`\].
    IsJson(Box<super::IsJson>),
    /// [§8.6] `<null predicate>` ::= `<row value constructor>` IS \[NOT\] NULL
    Null {
        /// The row tested.
        value: RowValue,
        /// IS NOT NULL was written.
        not: bool,
    },
    /// [ISO/IEC 9075-2:2023 §8.15] `<distinct predicate>` ::= `<row value
    /// predicand 3>` IS \[NOT\] DISTINCT FROM `<row value predicand 4>`
    Distinct {
        /// The first row, X.
        left: RowValue,
        /// The second row, Y.
        right: RowValue,
        /// IS NOT DISTINCT FROM was written \[SR6\].
        not: bool,
    },
    /// [§8.7] `<quantified comparison predicate>` ::= `<row value
    /// constructor>` `<comp op>` `<quantifier>` `<table subquery>`
    Quantified {
        /// The row compared.
        value: RowValue,
        /// The `<comp op>`.
        op: CompOp,
        /// ALL, rather than SOME or ANY.
        all: bool,
        /// The `<table subquery>`.
        subquery: Box<Subquery>,
    },
    /// [§8.8] `<exists predicate>` ::= EXISTS `<table subquery>`
    Exists(Box<Subquery>),
    /// [ISO/IEC 9075-2:2023 §8.11] `<unique predicate>` ::= UNIQUE
    /// \[`<unique null treatment>`\] `<table subquery>`
    Unique {
        /// The `<table subquery>`.
        subquery: Box<Subquery>,
        /// NULLS DISTINCT, written or implicit, rather than NULLS NOT
        /// DISTINCT.
        nulls_distinct: bool,
    },
    /// [§8.10] `<match predicate>` ::= `<row value constructor>` MATCH
    /// \[UNIQUE\] \[PARTIAL | FULL\] `<table subquery>`
    Match {
        /// The row matched.
        value: RowValue,
        /// UNIQUE was written.
        unique: bool,
        /// The match type.
        match_type: MatchType,
        /// The `<table subquery>`.
        subquery: Box<Subquery>,
    },
    /// [ISO/IEC 9075-2:2023 §8.16] `<member predicate>` ::= `<row value
    /// predicand>` \[NOT\] MEMBER \[OF\] `<multiset value expression>`
    Member {
        /// The row tested, X.
        value: RowValue,
        /// The multiset, MVE.
        multiset: Box<ValueExpression>,
        /// NOT MEMBER was written \[SR5\].
        not: bool,
    },
    /// [ISO/IEC 9075-2:2023 §8.17] `<submultiset predicate>` ::= `<row value
    /// predicand>` \[NOT\] SUBMULTISET \[OF\] `<multiset value expression>`
    Submultiset {
        /// The multiset tested, CVE: a row of one value \[SR1\].
        value: RowValue,
        /// The multiset, MVE.
        multiset: Box<ValueExpression>,
        /// NOT SUBMULTISET was written \[SR5\].
        not: bool,
    },
    /// [ISO/IEC 9075-2:2023 §8.18] `<set predicate>` ::= `<row value
    /// predicand>` IS \[NOT\] A SET
    Set {
        /// The multiset tested, CVE: a row of one value \[SR1\].
        value: RowValue,
        /// IS NOT A SET was written \[SR3\].
        not: bool,
    },
    /// \[EXTENSION\] [P1 §4.7] A predicate of graph elements, which only
    /// a GRAPH_TABLE's conditions and columns may have.
    Graph(Box<super::GraphPredicate>),
    /// \[EXTENSION\] [GS §10.1] MATCH ( `<graph search pattern>` ) over
    /// the tables of the FROM clause, which only a WHERE clause may have.
    GraphMatch(Box<super::MatchClause>),
    /// [ISO/IEC 9075-2:2023 §8.19] `<type predicate>` ::= `<row value
    /// predicand>` IS \[NOT\] OF ( `<type list>` )
    Type {
        /// The value tested: a row of one value \[SR1\].
        value: RowValue,
        /// The types.
        types: super::TypeList,
        /// IS NOT OF was written.
        not: bool,
    },
    /// [ISO/IEC 9075-14:2006 draft §8.2, §8.3] `<XML content predicate>`
    /// ::= `<row value predicand>` IS \[NOT\] CONTENT, and `<XML document
    /// predicate>` ::= `<row value predicand>` IS \[NOT\] DOCUMENT.
    Xml {
        /// The XML value tested, XVE: a row of one value \[SR1\].
        value: RowValue,
        /// DOCUMENT or CONTENT.
        syntax: crate::xml::Syntax,
        /// IS NOT was written \[SR2\].
        not: bool,
    },
    /// [ISO/IEC 9075-14:2006 draft §8.5] `<XML valid predicate>` ::= `<row
    /// value predicand>` IS \[NOT\] VALID `<document or content or
    /// sequence>` \[`<XML valid according to clause>`\].
    XmlValid {
        /// The XML value tested, XVE: a row of one value \[SR1\].
        value: RowValue,
        /// DOCUMENT, CONTENT or SEQUENCE, DCS \[SR2\].
        dcs: super::Dcs,
        /// The `<XML valid according to clause>`, if written.
        according: Option<Box<super::XmlValidAccording>>,
        /// IS NOT was written \[SR3\].
        not: bool,
    },
}

// ---------------------------------------------------------------------------
// [X3.135-1986 §5.9] <value expression>
// ---------------------------------------------------------------------------

/// [X3.135-1986 §5.9] `<value expression>` ::= `<term>` | `<value expression>` {+|-} `<term>`
/// `<term>`    ::= `<factor>` | `<term>` {*|/} `<factor>`
/// `<factor>`  ::= \[+|-\] `<primary>`
/// `<primary>` ::= `<value specification>` | `<column specification>`
///             | `<set function specification>` | (`<value expression>`)
///
/// And the primaries and operators of ISO/IEC 9075:1992 and the \[EXTENSION\] below.
#[derive(Debug, Clone)]
pub enum ValueExpression {
    /// A dyadic operator: +, -, * or /.
    Dyadic(Box<ValueExpression>, ArithOp, Box<ValueExpression>),
    /// A `<factor>` with a sign.
    Monadic(Sign, Box<ValueExpression>),
    /// A `<value specification>`.
    Value(ValueSpecification),
    /// A `<column specification>`.
    Column(ColumnSpecification),
    /// A `<set function specification>`.
    SetFunction(SetFunctionSpecification),
    /// A `<value expression>` in parentheses.
    Nested(Box<ValueExpression>),
    /// [ISO/IEC 9075-2:2023 §6.29, §6.46] A `<boolean value expression>`
    /// as a value: a search condition's truth value, Unknown the null value
    /// [§4.6.1].
    Boolean(Box<SearchCondition>),
    /// [ISO/IEC 9075-2:2023 §10.7] A value of the collation its `<collate
    /// clause>` names.
    Collated(Box<ValueExpression>, TableName),
    /// [ISO/IEC 9075-2:2023 §6.4] `<current collation specification>` ::=
    /// COLLATION FOR ( `<string value expression>` ).
    CollationFor(Box<ValueExpression>),
    /// [ISO/IEC 9075-2:2023 §6.14] `<next value expression>` ::= NEXT
    /// VALUE FOR `<sequence generator name>`.
    NextValue {
        /// The `<sequence generator name>`.
        name: TableName,
        /// Where it begins in the statement.
        at: usize,
    },
    /// [ISO/IEC 9075:1992 §6.9] A `<case expression>`.
    Case(Box<CaseExpression>),
    /// [ISO/IEC 9075-2:2023 §6.13] CAST (`<cast operand>` AS `<cast
    /// target>` \[FORMAT `<cast template>`\]).
    Cast {
        /// The `<value expression>`, or none for NULL.
        operand: Option<Box<ValueExpression>>,
        /// The `<cast target>`: a `<data type>`, or a `<domain name>`.
        target: TypeName,
        /// The `<cast template>`'s string, if written [Feature T839].
        format: Option<String>,
        /// [ISO/IEC 9075-14:2006 draft §6.5] The `<XML passing
        /// mechanism>`, if written.
        passing: Option<super::Passing>,
        /// Where it begins in the statement.
        at: usize,
    },
    /// [ISO/IEC 9075:1992 §7.11] A `<scalar subquery>`.
    Subquery(Box<Subquery>),
    /// [ISO/IEC 9075-14:2006 draft §6.6, §6.8, §6.13, §6.16] An SQL/XML
    /// operator, XMLCAST, XMLDOCUMENT, XMLPARSE or XMLSERIALIZE, and where
    /// it begins.
    Xml(Box<super::XmlOperator>, usize),
    /// [ISO/IEC 9075:1992 §6.6, §6.7, §6.13] A string function, or
    /// concatenation, read with its own syntax: the function registered
    /// under `name`, called with its operands in order.
    Function {
        /// The key word it is registered under.
        name: &'static str,
        /// The operands, in order.
        args: Vec<ValueExpression>,
        /// Where it begins in the statement.
        at: usize,
    },
    /// [ISO/IEC 9075:1992 §6.8] A `<datetime value function>`.
    Datetime(DatetimeFunction),
    /// [ISO/IEC 9075-2:2023 §6.27] A `<row pattern navigation operation>`.
    Navigation(Box<Navigation>),
    /// [ISO/IEC 9075-2:2023 §6.33] `<classifier function>` ::= CLASSIFIER (
    /// \[`<row pattern variable name>`\] ).
    Classifier {
        /// The `<row pattern variable name>`, if written.
        variable: Option<String>,
        /// Where it begins in the statement.
        at: usize,
    },
    /// [ISO/IEC 9075-2:2023 §6.31] `<match number function>` ::=
    /// MATCH_NUMBER ( ).
    MatchNumber {
        /// Where it begins in the statement.
        at: usize,
    },
    /// [ISO/IEC 9075:1992 §6.15] (`<datetime value expression>` -
    /// `<datetime term>`) `<interval qualifier>`.
    Difference {
        /// The datetime subtracted from.
        minuend: Box<ValueExpression>,
        /// The datetime subtracted.
        subtrahend: Box<ValueExpression>,
        /// The interval's fields.
        qualifier: Qualifier,
        /// Where it begins in the statement.
        at: usize,
    },
    /// [ISO/IEC 9075:1992 §6.14] A `<datetime primary>` with its `<time
    /// zone>`: AT LOCAL, none, or AT TIME ZONE an interval.
    AtZone {
        /// The datetime.
        source: Box<ValueExpression>,
        /// The displacement, or none for the session's.
        zone: Option<Box<ValueExpression>>,
        /// Where it begins in the statement.
        at: usize,
    },
    /// [ISO/IEC 9075:1992 §6.6] EXTRACT (`<extract field>` FROM `<extract
    /// source>`).
    Extract {
        /// The field.
        field: ExtractField,
        /// The datetime it is of.
        source: Box<ValueExpression>,
        /// Where it begins in the statement.
        at: usize,
    },
    /// [ISO/IEC 9075-2:2023 §6.16, §6.25, §6.49, §7.1] An expression of
    /// arrays and rows, and where it begins in the statement.
    Composite(Box<Composite>, usize),
    /// [ISO/IEC 9075-2:2023 §10.4] `<routine invocation>` ::= \[`<schema
    /// name>` .\] `<identifier>` ( \[`<value expression>` \[{, `<value
    /// expression>`}...\]\] ): of a routine, or \[EXTENSION\] of a
    /// function registered, taken only when a session enables it.
    Call {
        /// The `<schema name>`, if written.
        schema: Option<String>,
        /// The function's name.
        name: String,
        /// The arguments, in order.
        args: Vec<ValueExpression>,
        /// Where it begins in the statement.
        at: usize,
        /// \[EXTENSION\] Whether the session took function calls as it was
        /// read, so it may call a function registered.
        extension: bool,
    },
    /// [ISO/IEC 9075-2:2023 §6.18 to §6.24] An expression of user-defined
    /// types and references, and where it begins in the statement.
    Structured(Box<super::StructuredExpression>, usize),
    /// [ISO/IEC 9075-2:2023 §6.28, §6.34, §6.35] JSON_VALUE, JSON_QUERY,
    /// JSON_OBJECT or JSON_ARRAY, and where it begins in the statement.
    Json(Box<super::JsonExpression>, usize),
    /// [ISO/IEC 9075-2:2023 §6.10] A `<window function>`.
    Window(Box<WindowFunction>),
    /// [ISO/IEC 9075-2:2023 §6.11] A `<nested window function>`.
    NestedWindow(Box<NestedWindow>),
    /// \[EXTENSION\] A function of a graph variable, which only a
    /// GRAPH_TABLE's conditions and columns may have, and where it begins.
    Graph(Box<super::GraphFunction>, usize),
}

/// [ISO/IEC 9075:1992 §6.9] `<case expression>` ::= `<case abbreviation>` |
/// `<case specification>`. A `<result>` that is NULL, and an `<else
/// clause>` not written \[SR6\], are none.
#[derive(Debug, Clone)]
pub enum CaseExpression {
    /// NULLIF (V1, V2) \[SR1\].
    NullIf(ValueExpression, ValueExpression),
    /// COALESCE (V1, V2, ...) \[SR2, SR3\].
    Coalesce(Vec<ValueExpression>),
    /// `<simple case>`: CASE `<case operand>` WHEN `<when operand list>`
    /// THEN `<result>` ... \[ELSE `<result>`\] END.
    Simple {
        /// The `<case operand>`, a `<row value predicand>` [ISO/IEC
        /// 9075-2:2023 §6.12, Feature F262].
        operand: RowValue,
        /// Each `<when operand list>`, one `<when operand>` at least
        /// [ISO/IEC 9075-2:2023 §6.12, Feature F263], and its `<result>`.
        whens: Vec<(Vec<WhenOperand>, Option<ValueExpression>)>,
        /// The `<result>` of the `<else clause>`.
        otherwise: Option<ValueExpression>,
    },
    /// `<searched case>`: CASE WHEN `<search condition>` THEN `<result>`
    /// ... \[ELSE `<result>`\] END.
    Searched {
        /// Each `<search condition>` and its `<result>`.
        whens: Vec<(SearchCondition, Option<ValueExpression>)>,
        /// The `<result>` of the `<else clause>`.
        otherwise: Option<ValueExpression>,
    },
}

/// [ISO/IEC 9075-2:2023 §6.12] A `<when operand>` of a `<simple case>`.
#[derive(Debug, Clone)]
pub enum WhenOperand {
    /// A `<row value predicand>`, which the case operand equals for the
    /// WHEN to be true [SR2f i], and where it begins.
    Value(RowValue, usize),
    /// A predicate's second part, as `> 5` or `IS NULL` [Feature F262],
    /// read as the whole predicate, the case operand its first part
    /// [SR2f ii, SR2i].
    Condition(SearchCondition),
}

/// The sign of a `<factor>` [X3.135-1986 §5.9].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sign {
    /// +
    Plus,
    /// -
    Minus,
}

/// [X3.135-1986 §5.7] `<column specification>` ::= \[`<qualifier>`.\] `<column name>`
#[derive(Debug, Clone)]
pub struct ColumnSpecification {
    /// The `<qualifier>`, if written.
    pub qualifier: Option<TableName>,
    /// The `<column name>`.
    pub column: String,
    /// The `<catalog name>` written before the qualifier, if one is. Of a
    /// value, it may instead name a table whose column of a row type the
    /// qualifier's schema part names, the qualifier's table part and the
    /// column name that column's fields [ISO/IEC 9075-2:2023 §6.6 SR9b iv
    /// 2], as the binder decides \[SR9c\].
    pub catalog: Option<String>,
    /// Where it begins in the statement: a character offset, for the
    /// positions of errors (`SqlError::or_at`).
    pub at: usize,
}

mod children;
