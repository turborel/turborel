//! \[EXTENSION\] HYBRID_SEARCH (the vector design document §11.2, §11.3),
//! bound as the derived table it stands for: of the hybrid index it names
//! (`index/vector/hybrid.rs`), the table's rows nearest QUERY_VECTOR by the
//! index's metric, FETCH APPROX FIRST CANDIDATES of them, and those
//! ranked highest for QUERY_TEXT by the index's full-text part, FETCH
//! FIRST CANDIDATES of them, fused and the first TOP_N kept.
//!
//! Each list is read through its part of the index, as an ordinary query
//! reads it, so EXPLAIN shows both reads. A row of each list has its rank
//! in it, 1 for the nearest or the highest ranked, and its score: of the
//! vector list 1 minus its distance, of the text list its rank score
//! (RULES.md D87), above 0. The lists are joined by the table's primary
//! key, which is DOC_ID, a row found by one list alone given no rank or
//! score of the other. Its SCORE is, by FUSION:
//!
//! - RRF, the sum over the lists holding it of the list's weight over
//!   RRF_K and its rank in the list [§11.3];
//! - WEIGHTED, the sum over the lists holding it of the list's weight
//!   times its score normalized min-max over the list's candidates, 1 if
//!   they all score alike [§11.3].
//!
//! Its rows are ordered by SCORE, highest first, then by DOC_ID, which
//! keeps ties stable. Its columns are DOC_ID, SCORE, VECTOR_SCORE,
//! TEXT_SCORE, VECTOR_RANK, TEXT_RANK and CHUNK_TEXT, the row's text
//! [R11.2]: a row is one chunk.

use super::from::{Bounds, FromItem, derived};
use crate::ast::{Fusion, HybridSearch};
use crate::catalog::{IndexDesc, TableDesc};
use crate::db::State;
use crate::error::{Code, Result, SqlError};
use crate::index::vector::hybrid::HYBRID_METHOD;
use crate::parser::Parser;

/// [R11.1] Each list's candidates, of TOP_N, by default.
const CANDIDATES_PER_ROW: u32 = 4;
/// The correlation name of the table in each list's query.
const DOCUMENTS: &str = "HYBRID_SEARCH_DOCUMENTS";
/// The correlation names of the lists.
const VECTOR_SIDE: &str = "HYBRID_SEARCH_VECTOR_SIDE";
const TEXT_SIDE: &str = "HYBRID_SEARCH_TEXT_SIDE";

fn refused(message: impl Into<String>) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {}", message.into()))
}

/// `name` as a `<delimited identifier>`.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('"', "\"\""))
}

/// `text` as a `<character string literal>`.
fn string(text: &str) -> String {
    format!("'{}'", text.replace('\'', "''"))
}

/// `x` as an `<approximate numeric literal>`.
fn approximate(x: f64) -> String {
    format!("{x:E}")
}

/// The item of the FROM clause HYBRID_SEARCH `search` is, its first
/// column at `offset` of the FROM row. An error is placed where it
/// begins: the query it stands for is not the statement's text.
pub(super) fn hybrid_item(b: Bounds<'_>, search: &HybridSearch, offset: usize) -> Result<FromItem> {
    let placed = |e: SqlError| SqlError { pos: Some(search.at), ..e };
    let text = query_of(b.cx.db, search).map_err(placed)?;
    let table = Parser::hybrid_expansion(&text, search).map_err(placed)?;
    derived(b, &table, offset).map_err(placed)
}

/// What a list's query reads: the table, its key, text and vector
/// columns, and the index's name and metric.
struct Read {
    table: String,
    table_ident: String,
    key: String,
    text: String,
    vector: String,
    index: String,
    metric: Option<String>,
}

impl Read {
    /// What `search` reads, of the hybrid index it names.
    fn of(db: &State, search: &HybridSearch) -> Result<Read> {
        let ix = db.catalog.index(&search.index).filter(|ix| ix.method == HYBRID_METHOD);
        let ix = ix.ok_or_else(|| refused(format!("HYBRID_SEARCH's INDEX {} is no hybrid index", search.index)))?;
        let desc = db.catalog.table(&ix.table).ok_or_else(|| refused(format!("there is no table {}", ix.table)))?;
        let key = match desc.primary_key() {
            Some([key]) => *key,
            Some(_) | None => {
                return Err(refused(format!("HYBRID_SEARCH's DOC_ID is a primary key of one column, which {} has not", ix.table)));
            }
        };
        let column = |at: usize| column_name(desc, ix, at);
        Ok(Read {
            table: format!("{}.{}", quoted(&ix.table.auth), quoted(&ix.table.ident)),
            table_ident: quoted(&ix.table.ident),
            key: desc.columns.get(key).map(|c| quoted(&c.name)).ok_or_else(|| refused("its key is no column"))?,
            text: column(0)?,
            vector: column(1)?,
            index: quoted(&ix.name),
            metric: db.methods.get(HYBRID_METHOD).and_then(|m| m.metric(ix)),
        })
    }
}

/// The name of the column of `ix`'s key at `at`, quoted.
fn column_name(desc: &TableDesc, ix: &IndexDesc, at: usize) -> Result<String> {
    let column = ix.columns.get(at).and_then(|c| desc.columns.get(*c));
    column.map(|c| quoted(&c.name)).ok_or_else(|| refused(format!("the index {} has no column {at}", ix.name)))
}

/// The text of the derived table `search` stands for.
fn query_of(db: &State, search: &HybridSearch) -> Result<String> {
    let read = Read::of(db, search)?;
    let candidates = match search.candidates {
        Some(n) => n,
        None => search.top_n.checked_mul(CANDIDATES_PER_ROW).ok_or_else(|| refused("HYBRID_SEARCH's TOP_N is too large"))?,
    };
    let (vector, text) = (vector_side(&read, search, candidates), text_side(&read, search, candidates));
    let sides = if search.vector_first {
        format!("( {vector} ) AS {VECTOR_SIDE} FULL JOIN ( {text} ) AS {TEXT_SIDE}")
    } else {
        format!("( {text} ) AS {TEXT_SIDE} FULL JOIN ( {vector} ) AS {VECTOR_SIDE}")
    };
    let (v, t) = (VECTOR_SIDE, TEXT_SIDE);
    let columns = search.columns.as_ref().map(|c| format!(" ({})", c.iter().map(|n| quoted(n)).collect::<Vec<_>>().join(", ")));
    Ok(format!(
        "( SELECT COALESCE({v}.K, {t}.K) AS DOC_ID, {score} AS SCORE, 1E0 - {v}.D AS VECTOR_SCORE, {t}.S AS TEXT_SCORE, \
         {v}.R AS VECTOR_RANK, {t}.R AS TEXT_RANK, COALESCE({v}.C, {t}.C) AS CHUNK_TEXT \
         FROM {sides} ON {v}.K = {t}.K ORDER BY SCORE DESC, DOC_ID FETCH FIRST {n} ROWS ONLY ) AS {correlation}{columns}",
        score = score(search),
        n = search.top_n,
        correlation = quoted(&search.correlation),
        columns = columns.unwrap_or_default(),
    ))
}

/// The query of the vector list: the candidates nearest QUERY_VECTOR,
/// each its key, distance D, text C, rank R, and the least and greatest
/// distance of the list, LO and HI.
fn vector_side(read: &Read, search: &HybridSearch, candidates: u32) -> String {
    let metric = read.metric.as_deref().map(|m| format!(", {}", string(m))).unwrap_or_default();
    format!(
        "SELECT K, D, C, ROW_NUMBER() OVER (ORDER BY D, K) AS R, MIN(D) OVER () AS LO, MAX(D) OVER () AS HI \
         FROM ( SELECT {DOCUMENTS}.{key} AS K, VECTOR_DISTANCE({DOCUMENTS}.{vector}, ({query}){metric}) AS D, \
         {DOCUMENTS}.{text} AS C FROM {table} AS {DOCUMENTS} ORDER BY D FETCH APPROX FIRST {candidates} ROWS ONLY \
         HINT (VECTOR_INDEX({table_ident} {index})) ) AS HYBRID_SEARCH_NEAREST WHERE D IS NOT NULL",
        key = read.key,
        vector = read.vector,
        query = search.vector.text,
        text = read.text,
        table = read.table,
        table_ident = read.table_ident,
        index = read.index,
    )
}

/// The query of the text list: the candidates ranked highest for
/// QUERY_TEXT, each its key, score S, text C, rank R, and the least and
/// greatest score of the list, LO and HI.
fn text_side(read: &Read, search: &HybridSearch, candidates: u32) -> String {
    format!(
        "SELECT K, S, C, ROW_NUMBER() OVER (ORDER BY S DESC, K) AS R, MIN(S) OVER () AS LO, MAX(S) OVER () AS HI \
         FROM ( SELECT {DOCUMENTS}.{key} AS K, RANK({DOCUMENTS}.{text}, {pattern}) AS S, {DOCUMENTS}.{text} AS C \
         FROM {table} AS {DOCUMENTS} ORDER BY S DESC NULLS LAST FETCH FIRST {candidates} ROWS ONLY ) AS HYBRID_SEARCH_RANKED WHERE S > 0",
        key = read.key,
        text = read.text,
        pattern = pattern(search),
        table = read.table,
    )
}

/// The full-text pattern of QUERY_TEXT: IS ABOUT its words, which ranks a
/// text by any of them (ISO/IEC 13249-2 §5.3), each `"` of them doubled.
fn pattern(search: &HybridSearch) -> String {
    match &search.text.literal {
        Some(words) => string(&format!("IS ABOUT \"{}\"", words.replace('"', "\"\""))),
        None => format!("'IS ABOUT \"' || TRANSLATE_REGEX('\"' IN ({}) WITH '\"\"' OCCURRENCE ALL) || '\"'", search.text.text),
    }
}

/// [§11.3] The fused score of a row, by FUSION.
fn score(search: &HybridSearch) -> String {
    let (wv, wt) = (approximate(search.weights.0), approximate(search.weights.1));
    let (v, t) = (VECTOR_SIDE, TEXT_SIDE);
    match search.fusion {
        Fusion::Rrf => {
            let k = approximate(f64::from(search.rrf_k));
            format!("COALESCE({wv} / ({k} + {v}.R), 0E0) + COALESCE({wt} / ({k} + {t}.R), 0E0)")
        }
        Fusion::Weighted => format!(
            "COALESCE({wv} * CASE WHEN {v}.HI = {v}.LO THEN 1E0 ELSE ({v}.HI - {v}.D) / ({v}.HI - {v}.LO) END, 0E0) + \
             COALESCE({wt} * CASE WHEN {t}.HI = {t}.LO THEN 1E0 ELSE ({t}.S - {t}.LO) / ({t}.HI - {t}.LO) END, 0E0)"
        ),
    }
}
