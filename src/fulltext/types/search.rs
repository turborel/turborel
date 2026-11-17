//! The Contains [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.1.2] and Rank
//! [§5.1.3] methods of FullText, as CONTAINS(SELF, pattern), INTEGER 1 or
//! 0, and RANK(SELF, pattern), DOUBLE PRECISION. Either takes an FT_Pattern
//! value or a character string, which is cast to one [§5.1.2, §5.1.3
//! Definition]. The binder appends to RANK the name of the full-text index
//! of the column its document is read from, if it has one, and to either,
//! of a statement that reads thesauri, the relations of the FT_THESAURUS
//! tables (`fulltext/thesaurus.rs`), after their own arguments.
//!
//! \[EXTENSION\] Rank is BM25 (`fulltext/rank.rs`), or the ranking the
//! index names (FULLTEXT.RANKING), over the statistics of that index as
//! its statement first reads them (`CallContext::ranking_statistics`); of
//! no index, every inverse document frequency is 1 (RULES.md D87).
//!
//! The pattern is read as a `<search expression>` in the document's
//! language (`pattern/parse.rs`), its thesaurus expansions replaced by the
//! terms those relations give (`Pattern::expanded`), and matched against
//! the document's tokens (`pattern/eval.rs`). \[DESIGN\] A null document
//! gives null before its pattern is read, so a call of a null column
//! raises nothing (RULES.md D87); a null pattern gives null [§5.1.2
//! Description 3c i, §5.1.3 Description 3b i].
//!
//! \[EXTENSION\] SELF may also be a character string or a character large
//! object, read a chunk at a time, in FT_DefaultLanguage.
//!
//! Neither is an index predicate (RULES.md D34): each gives a number, and
//! each raises for a pattern that is no `<search expression>` (2FF01) or
//! has nothing to look for (2FF03). A full-text index finds rows for
//! CONTAINS compared with 1 nonetheless (`predicate_truth`), the WHERE
//! check raising as a scan would (`rechecked_errors`, D87).

use std::sync::Arc;

use super::ft_pattern::pattern_text;
use super::{FT_DEFAULT_LANGUAGE, FT_MAX_PATTERN_LENGTH, FtPatternType, FullTextType, is_type};
use crate::error::{Code, Result, SqlError};
use crate::fulltext::Language;
use crate::fulltext::pattern::{CONTAINS, Pattern, RANK, contains, parse};
use crate::fulltext::rank::{BM25, NoIndex, Ranking, Statistics, built_in, named};
use crate::fulltext::thesaurus::{relations_query, terms_of};
use crate::fulltext::value::{Document, FULLTEXT, document};
use crate::functions::{AppendedQuery, CallContext, ScalarFunction};
use crate::index::fulltext::{DEFAULT_B, DEFAULT_K1, score};
use crate::params::ParameterType;
use crate::value::{DOUBLE_PRECISION, DataType, Decimal, INTEGER_PRECISION, Spelled, Value};

/// The type of Contains's result [§5.1.2 Definition].
const INTEGER: DataType = DataType::Exact { precision: INTEGER_PRECISION, scale: 0, spelled: Spelled::Integer };

/// The type of Rank's result [§5.1.3 Definition].
const DOUBLE: DataType = DataType::Approximate { precision: DOUBLE_PRECISION, spelled: Spelled::DoublePrecision };

/// The error of a call of `name` not of a document and a pattern.
fn signature(name: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 13249-2:2002 §5.1.2] {name} takes a FullText value and a pattern"))
}

/// Whether `t` is the type of a document: FullText, or \[EXTENSION\] a
/// character string.
fn is_document(t: &DataType) -> bool {
    is_type(t, FULLTEXT) || t.is_character()
}

/// Whether `t` is the type of a pattern: FT_Pattern, or a character
/// string, cast to one.
fn is_pattern(t: &DataType) -> bool {
    is_type(t, super::FT_PATTERN) || t.is_character()
}

/// Whether the document `v` has no characters [§5.1.2 Description 3c ii].
fn is_empty(v: &Value) -> bool {
    match v {
        Value::Char(text) => text.is_empty(),
        Value::Lob(lob) => lob.octets() == 0,
        Value::Extension(_) => super::fulltext::parts(v).is_ok_and(|(_, text)| text.is_empty()),
        Value::Null
        | Value::Exact(_)
        | Value::Approx(_)
        | Value::DecFloat(_)
        | Value::Binary(_)
        | Value::Bool(_)
        | Value::Datetime(_)
        | Value::Interval(_)
        | Value::Array(_)
        | Value::Multiset(_)
        | Value::Structured(_)
        | Value::Reference(_)
        | Value::Row(_) => false,
    }
}

/// The search a call of `name` asks: its pattern read in the document's
/// language, `fallback` of a character string, and expanded through the thesaurus relations of its third
/// argument, if it has one, the document and that language; none if the
/// document or the pattern is null, the document checked first, so a
/// call of a null column raises nothing (RULES.md D87).
///
/// # Errors
///
/// 2FF01 if the pattern is no `<search expression>`; as reading the
/// document's payload, the relations and expanding the pattern.
fn search<'v>(name: &str, args: &'v [Value], fallback: Language) -> Result<Option<(Pattern, Document<'v>, Language)>> {
    let [self_, pattern, extras @ ..] = args else { return Err(signature(name)) };
    let (_, relations) = given(name, extras)?;
    let Some(document) = document(self_)? else { return Ok(None) };
    if pattern.is_null() {
        return Ok(None);
    }
    let text = if let Value::Char(text) = pattern { text.as_str() } else { pattern_text(pattern).ok_or_else(|| signature(name))? };
    let language = document.language.unwrap_or(fallback);
    let expanded = parse(text, language)?.expanded(&terms_of(relations)?)?;
    Ok(Some((expanded, document, language)))
}

/// The arguments the binder appends, `extras`: the name of the index that
/// ranks the document's column, a character string, and the thesaurus
/// relations, of any other type, each if given (RULES.md D87).
///
/// # Errors
///
/// SQLCODE -103 of more than one of either.
fn given<'v>(name: &str, extras: &'v [Value]) -> Result<(Option<&'v str>, Option<&'v Value>)> {
    let mut given = (None, None);
    for extra in extras {
        let doubled =
            if let Value::Char(index) = extra { given.0.replace(index.as_str()).is_some() } else { given.1.replace(extra).is_some() };
        if doubled {
            return Err(signature(name));
        }
    }
    Ok(given)
}

/// The declared type of a document argument, its own if it is one, else
/// FullText; of a pattern argument, FT_Pattern, to which a character
/// string is assigned; the thesaurus relations are given as they are.
fn argument(position: usize, known: &[Option<DataType>]) -> Option<DataType> {
    match position {
        0 => Some(known.first().and_then(Option::as_ref).filter(|t| is_document(t)).cloned().unwrap_or_else(FullTextType::data_type)),
        1 => Some(FtPatternType::data_type()),
        _ => None,
    }
}

/// The type a dynamic parameter takes: FullText as the document, a
/// character string as the pattern.
fn parameter(position: usize) -> Option<ParameterType> {
    match position {
        0 => Some(ParameterType::Of(FullTextType::data_type())),
        1 => Some(ParameterType::Of(DataType::varying(FT_MAX_PATTERN_LENGTH))),
        _ => None,
    }
}

/// The result type of a call of `name` of `args`.
fn result(name: &str, args: &[DataType], output: DataType) -> Result<DataType> {
    let [document, pattern, extras @ ..] = args else { return Err(signature(name)) };
    let appended = extras.len() <= 2 && extras.iter().all(|t| t.is_character() || matches!(t, DataType::Array(_)));
    if is_document(document) && is_pattern(pattern) && appended { Ok(output) } else { Err(signature(name)) }
}

/// CONTAINS(SELF, pattern), the Contains methods [§5.1.2]: 1 if the
/// document holds the pattern, else 0; 0 of a document of no characters.
#[derive(Debug, Clone, Copy)]
pub struct Contains;

impl Contains {
    /// 1 if the document of `args` holds their pattern, a character string
    /// read in `fallback`; 0 of a document of no characters; null of a null.
    ///
    /// # Errors
    ///
    /// As `search` and `contains`.
    fn held(args: &[Value], fallback: Language) -> Result<Value> {
        let Some((pattern, document, language)) = search(CONTAINS, args, fallback)? else { return Ok(Value::Null) };
        let empty = args.first().is_some_and(is_empty);
        let holds = !empty && contains(&pattern, document.chars, language)?;
        Ok(Value::Exact(Decimal::new(i128::from(holds), 0)))
    }
}

/// The arguments a full-text index reads of a CONTAINS call: the column
/// and the pattern, not the thesaurus relations (RULES.md D87).
const PROBED: usize = 2;

impl ScalarFunction for Contains {
    fn name(&self) -> &'static str {
        CONTAINS
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        result(CONTAINS, args, INTEGER)
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        argument(position, known)
    }

    fn parameter_type(&self, position: usize, _: &[Option<DataType>]) -> Option<ParameterType> {
        parameter(position)
    }

    /// Of no statement: a character string read in FT_DefaultLanguage.
    fn call(&self, args: &[Value]) -> Result<Value> {
        Contains::held(args, FT_DEFAULT_LANGUAGE)
    }

    /// A character string read in the language of the index the binder
    /// names, as Rank reads it (D87).
    fn call_in(&self, args: &[Value], cx: &dyn CallContext) -> Result<Value> {
        let index = args.get(2..).map(|extras| given(CONTAINS, extras)).transpose()?.and_then(|(index, _)| index);
        let statistics = index.map(|i| cx.ranking_statistics(i)).transpose()?.flatten();
        Contains::held(args, statistics.and_then(|s| s.language()).unwrap_or(FT_DEFAULT_LANGUAGE))
    }

    fn ranked_by_index(&self) -> bool {
        true
    }

    fn uses_context(&self) -> bool {
        true
    }

    /// The thesaurus relations are its arguments, so equal arguments give
    /// equal answers.
    fn deterministic(&self) -> bool {
        true
    }

    /// 1: the document holds the pattern [§5.1.2 Description 3c iii A].
    fn predicate_truth(&self) -> Option<Value> {
        Some(Value::Exact(Decimal::new(1, 0)))
    }

    /// It raises only of its pattern and relations, alike for every row,
    /// and a null document gives null before they are read.
    fn rechecked_errors(&self) -> bool {
        true
    }

    fn probe_arguments(&self) -> Option<usize> {
        Some(PROBED)
    }

    fn appended_query(&self, constants: &[Option<&Value>]) -> Option<AppendedQuery> {
        relations_query(constants.get(1).copied().flatten())
    }
}

/// RANK(SELF, pattern), the Rank methods [§5.1.3]: the document's score
/// for the pattern, by the ranking and statistics of the index of its
/// column, or BM25 with every inverse document frequency 1; 0 of a document
/// of no characters.
#[derive(Debug, Clone, Copy)]
pub struct Rank;

impl Rank {
    /// The rank of a call of `args`, of `statistics` and their ranking.
    ///
    /// # Errors
    ///
    /// As `search`; SQLCODE -910 if no ranking of the statistics' name is
    /// registered, or the ranking breaks its contract.
    fn ranked(args: &[Value], statistics: &dyn Statistics, ranking: Option<Arc<dyn Ranking>>) -> Result<Value> {
        // A character string is read in the index's language (D87).
        let fallback = statistics.language().unwrap_or(FT_DEFAULT_LANGUAGE);
        let Some((pattern, document, language)) = search(RANK, args, fallback)? else { return Ok(Value::Null) };
        if args.first().is_some_and(is_empty) {
            return Ok(Value::Approx(0.0));
        }
        let named = statistics.ranking();
        let ranking = ranking.ok_or_else(|| SqlError::new(Code::Api, format!("[DESIGN] no ranking {named} is registered")))?;
        Ok(Value::Approx(score(&pattern, document, language, statistics, &*ranking)?))
    }
}

/// The statistics of no index: BM25's default weights.
const UNINDEXED: NoIndex = NoIndex { k1: DEFAULT_K1, b: DEFAULT_B };

impl ScalarFunction for Rank {
    fn name(&self) -> &'static str {
        RANK
    }

    fn return_type(&self, args: &[DataType]) -> Result<DataType> {
        result(RANK, args, DOUBLE)
    }

    fn argument_type(&self, position: usize, known: &[Option<DataType>]) -> Option<DataType> {
        argument(position, known)
    }

    fn parameter_type(&self, position: usize, _: &[Option<DataType>]) -> Option<ParameterType> {
        parameter(position)
    }

    /// Of no statement: BM25 of no index.
    fn call(&self, args: &[Value]) -> Result<Value> {
        Rank::ranked(args, &UNINDEXED, named(&built_in(), BM25))
    }

    /// Of the statistics of the index the binder names, as the statement
    /// first read them, and the ranking they name.
    fn call_in(&self, args: &[Value], cx: &dyn CallContext) -> Result<Value> {
        let index = args.get(2..).map(|extras| given(RANK, extras)).transpose()?.and_then(|(index, _)| index);
        let Some(statistics) = index.map(|i| cx.ranking_statistics(i)).transpose()?.flatten() else {
            return Rank::ranked(args, &UNINDEXED, cx.ranking(BM25));
        };
        Rank::ranked(args, &*statistics, cx.ranking(statistics.ranking()))
    }

    fn appended_query(&self, constants: &[Option<&Value>]) -> Option<AppendedQuery> {
        relations_query(constants.get(1).copied().flatten())
    }

    fn ranked_by_index(&self) -> bool {
        true
    }

    fn uses_context(&self) -> bool {
        true
    }

    /// A score whose greatest an index may give first, as the same
    /// statistics are read for the bounds and for every call.
    fn ranks(&self) -> bool {
        true
    }

    /// FT_RankDeterminism [§5.1.1 DR6], implementation-defined: NOT
    /// DETERMINISTIC, as a rank moves with its index's statistics, though
    /// within a statement they are read once.
    fn deterministic(&self) -> bool {
        false
    }
}
