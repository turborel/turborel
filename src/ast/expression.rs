//! Abstract syntax of `<search condition>` [X3.135-1986 §5.18; ISO/IEC
//! 9075:1992 §8.12], its predicates [ISO/IEC 9075:1992 §8], and `<value
//! expression>` [X3.135-1986 §5.9; ISO/IEC 9075:1992 §6.11].
//!
//! Passes over them reach the expressions they are made of through
//! `ValueExpression::for_each_child` and `SearchCondition::for_each_value`,
//! which stop at a subquery, a query of its own.

use super::{
    Composite, DatetimeFunction, ExtractField, QueryExpression, SetArg, SetFunctionSpecification, TableName, TypeName, ValueSpecification,
};
use crate::catalog::MatchType;
use crate::value::interval::Qualifier;
use crate::value::{ArithOp, CompOp};

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
        /// Where it begins in the statement.
        at: usize,
    },
    /// [ISO/IEC 9075:1992 §7.11] A `<scalar subquery>`.
    Subquery(Box<Subquery>),
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

impl ValueExpression {
    /// Calls `f` with each value expression this one is made of: operands,
    /// arguments, a set function's argument, the parts of a CASE and the
    /// values its conditions compare; not those of a subquery.
    pub fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match self {
            ValueExpression::Value(_)
            | ValueExpression::Column(_)
            | ValueExpression::Subquery(_)
            | ValueExpression::Datetime(_)
            | ValueExpression::NextValue { .. } => {}
            ValueExpression::Extract { source, .. } => f(source),
            ValueExpression::Difference { minuend, subtrahend, .. } => {
                f(minuend);
                f(subtrahend);
            }
            ValueExpression::AtZone { source, zone, .. } => {
                f(source);
                if let Some(z) = zone {
                    f(z);
                }
            }
            ValueExpression::Dyadic(a, _, b) => {
                f(a);
                f(b);
            }
            ValueExpression::Monadic(_, x)
            | ValueExpression::Nested(x)
            | ValueExpression::Collated(x, _)
            | ValueExpression::CollationFor(x) => f(x),
            ValueExpression::SetFunction(sf) => {
                match &sf.arg {
                    SetArg::CountStar => {}
                    SetArg::Distinct(x) | SetArg::All(x) => f(x),
                }
                sf.order_by.iter().for_each(|o| f(&o.key));
            }
            ValueExpression::Cast { operand, .. } => operand.iter().for_each(|x| f(x)),
            ValueExpression::Function { args, .. } | ValueExpression::Call { args, .. } => args.iter().for_each(f),
            ValueExpression::Case(case) => case.for_each_child(f),
            ValueExpression::Boolean(c) => c.for_each_value(f),
            ValueExpression::Composite(c, _) => c.for_each_child(f),
        }
    }

    /// Calls `f` with each subquery in it, not those nested in them: its
    /// scalar subqueries, and those of the conditions of its CASEs.
    pub fn for_each_subquery<'a>(&'a self, f: &mut dyn FnMut(&'a Subquery)) {
        if let ValueExpression::Subquery(s) = self {
            f(s);
        }
        if let ValueExpression::Composite(c, _) = self
            && let Some(s) = c.subquery()
        {
            f(s);
        }
        if let ValueExpression::Case(case) = self
            && let CaseExpression::Searched { whens, .. } = &**case
        {
            for (c, _) in whens {
                c.for_each_own_subquery(f);
            }
        }
        if let ValueExpression::Boolean(c) = self {
            c.for_each_own_subquery(f);
        }
        self.for_each_child(&mut |child| child.for_each_subquery(f));
    }

    /// Whether `pred` holds of this expression or of one it is made of
    /// (`for_each_child`), at any depth.
    pub fn any(&self, pred: &dyn Fn(&ValueExpression) -> bool) -> bool {
        let mut found = pred(self);
        self.for_each_child(&mut |child| found = found || child.any(pred));
        found
    }
}

impl CaseExpression {
    /// Calls `f` with each value expression of it, those its searched WHEN
    /// conditions compare among them.
    fn for_each_child<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match self {
            CaseExpression::NullIf(a, b) => {
                f(a);
                f(b);
            }
            CaseExpression::Coalesce(values) => values.iter().for_each(f),
            CaseExpression::Simple { operand, whens, otherwise } => {
                operand.iter().for_each(&mut *f);
                for (list, r) in whens {
                    for when in list {
                        match when {
                            WhenOperand::Value(row, _) => row.iter().for_each(&mut *f),
                            WhenOperand::Condition(c) => c.for_each_value(f),
                        }
                    }
                    if let Some(x) = r {
                        f(x);
                    }
                }
                if let Some(x) = otherwise {
                    f(x);
                }
            }
            CaseExpression::Searched { whens, otherwise } => {
                for (c, r) in whens {
                    c.for_each_value(f);
                    if let Some(x) = r {
                        f(x);
                    }
                }
                if let Some(x) = otherwise {
                    f(x);
                }
            }
        }
    }
}

impl SearchCondition {
    /// Calls `f` with each value expression the condition compares, not
    /// those of its subqueries.
    pub fn for_each_value<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match self {
            SearchCondition::Or(a, b) | SearchCondition::And(a, b) => {
                a.for_each_value(f);
                b.for_each_value(f);
            }
            SearchCondition::Not(a) | SearchCondition::Is(a, ..) => a.for_each_value(f),
            SearchCondition::Predicate(p, _) => p.for_each_value(f),
            SearchCondition::Predicand(v, _) => f(v),
        }
    }

    /// Calls `f` with each subquery in it, not those nested in them: those
    /// its predicates apply to, and those in the values it compares.
    pub fn for_each_subquery<'a>(&'a self, f: &mut dyn FnMut(&'a Subquery)) {
        self.for_each_own_subquery(f);
        self.for_each_value(&mut |v| v.for_each_subquery(f));
    }

    /// Calls `f` with each subquery its predicates apply to.
    fn for_each_own_subquery<'a>(&'a self, f: &mut dyn FnMut(&'a Subquery)) {
        match self {
            SearchCondition::Or(a, b) | SearchCondition::And(a, b) => {
                a.for_each_own_subquery(f);
                b.for_each_own_subquery(f);
            }
            SearchCondition::Not(a) | SearchCondition::Is(a, ..) => a.for_each_own_subquery(f),
            SearchCondition::Predicate(p, _) => p.subquery().into_iter().for_each(f),
            SearchCondition::Predicand(..) => {}
        }
    }
}

impl Predicate {
    /// Calls `f` with each value expression the predicate compares, not
    /// those of its subqueries.
    pub fn for_each_value<'a>(&'a self, f: &mut dyn FnMut(&'a ValueExpression)) {
        match self {
            Predicate::Comparison(a, _, b) | Predicate::Overlaps(a, b) | Predicate::Distinct { left: a, right: b, .. } => {
                a.iter().chain(b).for_each(f);
            }
            Predicate::Between { value, low, high, .. } => value.iter().chain(low).chain(high).for_each(f),
            Predicate::InList { value, list, .. } => value.iter().chain(list).for_each(f),
            Predicate::Like { value, pattern, escape: third, .. } | Predicate::LikeRegex { value, pattern, flag: third, .. } => {
                f(value);
                f(pattern);
                if let Some(e) = third {
                    f(e);
                }
            }
            Predicate::InSubquery { value, .. }
            | Predicate::Null { value, .. }
            | Predicate::Quantified { value, .. }
            | Predicate::Match { value, .. } => {
                value.iter().for_each(f);
            }
            Predicate::Exists(_) | Predicate::Unique { .. } => {}
        }
    }

    /// The `<table subquery>` it applies to, if it has one.
    pub fn subquery(&self) -> Option<&Subquery> {
        match self {
            Predicate::InSubquery { subquery, .. }
            | Predicate::Quantified { subquery, .. }
            | Predicate::Match { subquery, .. }
            | Predicate::Unique { subquery, .. }
            | Predicate::Exists(subquery) => Some(subquery),
            Predicate::Comparison(..)
            | Predicate::Overlaps(..)
            | Predicate::Between { .. }
            | Predicate::InList { .. }
            | Predicate::Like { .. }
            | Predicate::LikeRegex { .. }
            | Predicate::Distinct { .. }
            | Predicate::Null { .. } => None,
        }
    }
}
