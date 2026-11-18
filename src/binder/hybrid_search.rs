//! \[EXTENSION\] HYBRID_SEARCH (the vector design document §11.2, §11.3),
//! bound as the derived table it stands for: of the hybrid index it names
//! (`index/vector/hybrid.rs`), the table's rows nearest QUERY_VECTOR by the
//! index's metric, FETCH APPROX FIRST CANDIDATES of them, and those
//! ranked highest for QUERY_TEXT by the index's full-text part, FETCH
//! FIRST CANDIDATES of them, fused and the first TOP_N kept. Of no
//! QUERY_VECTOR, the index's MODEL embeds QUERY_TEXT as a query, and that
//! is the vector searched for [§11.2].
//!
//! Each list is read through its part of the index, as an ordinary query
//! reads it, so EXPLAIN shows both reads (`read.rs`). Of an index that
//! embeds its text (`db/index_statements/hybrid_embedded.rs`), the text
//! list ranks the documents, and the vector list's rows are chunks, each
//! document kept once, at its nearest chunk. A document of the text list
//! alone is given its chunk ranked highest for QUERY_TEXT.
//!
//! A value argument is written into the query as often as it is read
//! there, QUERY_TEXT more than once of an index that embeds it, and each
//! copy noted, so its dynamic parameters keep their numbers
//! (`Parser::hybrid_expansion`).
//!
//! A row of each list has its rank in it, 1 for the nearest or the
//! highest ranked, and its score. Of the vector list that is 1 minus its
//! distance, of the text list its rank score (RULES.md D87), above 0. The
//! lists are joined by DOC_ID, a row found by one list alone given no
//! rank or score of the other. Its SCORE is, by FUSION:
//!
//! - RRF, the sum over the lists holding it of the list's weight over
//!   RRF_K and its rank in the list [§11.3];
//! - WEIGHTED, the sum over the lists holding it of the list's weight
//!   times its score normalized min-max over the list's candidates, 1 if
//!   they all score alike [§11.3].
//!
//! Its rows are ordered by SCORE, highest first, then by DOC_ID, which
//! keeps ties stable. Its columns are DOC_ID, SCORE, VECTOR_SCORE,
//! TEXT_SCORE, VECTOR_RANK, TEXT_RANK and CHUNK_TEXT, the row's text, or
//! the document's best chunk [R11.2].

mod read;

use self::read::{Read, chunk_id};
use super::from::{Bounds, FromItem, derived};
use crate::ast::{Fusion, HybridSearch, SearchArgument};
use crate::error::{Code, Result, SqlError};
use crate::parser::Parser;

/// [R11.1] Each list's candidates, of TOP_N, by default.
const CANDIDATES_PER_ROW: u32 = 4;
/// The correlation name of the table in each list's query.
const DOCUMENTS: &str = "HYBRID_SEARCH_DOCUMENTS";
/// The correlation name of a document's chunks, of its best chunk.
const CHUNKS: &str = "HYBRID_SEARCH_CHUNKS";
/// The correlation name of a list's best chunk of each document.
const BEST: &str = "HYBRID_SEARCH_BEST";
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
    let written = query_of(b.cx.db, search).map_err(placed)?;
    let table = Parser::hybrid_expansion(&written.sql, search, &written.copies).map_err(placed)?;
    derived(b, &table, offset).map_err(placed)
}

/// A query being written: its text, and the value argument each copy of
/// one in it is, in order.
#[derive(Default)]
struct Written {
    sql: String,
    copies: Vec<SearchArgument>,
}

impl Written {
    /// `text` written.
    fn put(&mut self, text: &str) -> &mut Written {
        self.sql.push_str(text);
        self
    }

    /// The text of the argument `argument`, `text`, written, a copy noted.
    fn copy(&mut self, argument: SearchArgument, text: &str) -> &mut Written {
        self.copies.push(argument);
        self.put(text)
    }

    /// \[§11.2\] The vector searched for written: QUERY_VECTOR, or of none,
    /// of an index that embeds its text, QUERY_TEXT embedded as a query by
    /// its model.
    fn query_vector(&mut self, read: &Read, search: &HybridSearch) -> Result<&mut Written> {
        if let Some(vector) = &search.vector {
            return Ok(self.copy(SearchArgument::Vector, &vector.text));
        }
        let model = read
            .model
            .as_ref()
            .ok_or_else(|| refused(format!("HYBRID_SEARCH names its QUERY_VECTOR, as {} embeds no text by a MODEL", search.index)))?;
        // Cast, so a dynamic parameter of it takes its type here too.
        Ok(self
            .put(&format!("EMBED({model}, CAST("))
            .copy(SearchArgument::Text, &search.text.text)
            .put(" AS CHARACTER VARYING(32767)), 'query')"))
    }

    /// The full-text pattern of QUERY_TEXT written: IS ABOUT its words,
    /// which ranks a text by any of them (ISO/IEC 13249-2 §5.3), each `"`
    /// of them doubled.
    fn pattern(&mut self, search: &HybridSearch) -> &mut Written {
        match &search.text.literal {
            Some(words) => self.put(&string(&format!("IS ABOUT \"{}\"", words.replace('"', "\"\"")))),
            None => self
                .put("'IS ABOUT \"' || TRANSLATE_REGEX('\"' IN (")
                .copy(SearchArgument::Text, &search.text.text)
                .put(") WITH '\"\"' OCCURRENCE ALL) || '\"'"),
        }
    }
}

/// The derived table `search` stands for, written.
fn query_of(db: &crate::db::State, search: &HybridSearch) -> Result<Written> {
    let read = Read::of(db, search)?;
    let candidates = match search.candidates {
        Some(n) => n,
        None => search.top_n.checked_mul(CANDIDATES_PER_ROW).ok_or_else(|| refused("HYBRID_SEARCH's TOP_N is too large"))?,
    };
    let (v, t) = (VECTOR_SIDE, TEXT_SIDE);
    let mut w = Written::default();
    w.put(&format!(
        "( SELECT COALESCE({v}.K, {t}.K) AS DOC_ID, {score} AS SCORE, 1E0 - {v}.D AS VECTOR_SCORE, {t}.S AS TEXT_SCORE, \
         {v}.R AS VECTOR_RANK, {t}.R AS TEXT_RANK, COALESCE({v}.C, {t}.C) AS CHUNK_TEXT FROM ( ",
        score = score(search),
    ));
    text_side(&mut w, &read, search, candidates);
    w.put(&format!(" ) AS {t} FULL JOIN ( "));
    vector_side(&mut w, &read, search, candidates)?;
    let columns = search.columns.as_ref().map(|c| format!(" ({})", c.iter().map(|n| quoted(n)).collect::<Vec<_>>().join(", ")));
    w.put(&format!(
        " ) AS {v} ON {v}.K = {t}.K ORDER BY SCORE DESC, DOC_ID FETCH FIRST {n} ROWS ONLY ) AS {correlation}{columns}",
        n = search.top_n,
        correlation = quoted(&search.correlation),
        columns = columns.unwrap_or_default(),
    ));
    Ok(w)
}

/// The query of the vector list written: the candidates nearest the
/// vector searched for, each its key, distance D, text C, rank R, and the
/// least and greatest distance of the list, LO and HI.
fn vector_side(w: &mut Written, read: &Read, search: &HybridSearch, candidates: u32) -> Result<()> {
    let n = &read.nearest;
    let metric = n.metric.as_deref().map(|m| format!(", {}", string(m))).unwrap_or_default();
    w.put(&format!(
        "SELECT K, D, C, ROW_NUMBER() OVER (ORDER BY D, K) AS R, MIN(D) OVER () AS LO, MAX(D) OVER () AS HI \
         FROM ( SELECT K, D, C, ROW_NUMBER() OVER (PARTITION BY K ORDER BY D, C) AS B \
         FROM ( SELECT {DOCUMENTS}.{key} AS K, VECTOR_DISTANCE({DOCUMENTS}.{vector}, (",
        key = n.key,
        vector = n.vector,
    ));
    w.query_vector(read, search)?;
    w.put(&format!(
        "){metric}) AS D, {DOCUMENTS}.{text} AS C FROM {table} AS {DOCUMENTS} ORDER BY D FETCH APPROX FIRST {candidates} ROWS ONLY \
         HINT (VECTOR_INDEX({ident} {index})) ) AS HYBRID_SEARCH_NEAREST WHERE D IS NOT NULL ) AS {BEST} WHERE B = 1",
        text = n.text,
        table = n.table,
        ident = n.ident,
        index = n.index,
    ));
    Ok(())
}

/// The query of the text list written: the candidates ranked highest for
/// QUERY_TEXT, each its key, score S, text C, rank R, and the least and
/// greatest score of the list, LO and HI. Of an index that embeds its
/// text, C is the document's chunk ranked highest.
fn text_side(w: &mut Written, read: &Read, search: &HybridSearch, candidates: u32) {
    let r = &read.ranked;
    w.put(&format!(
        "SELECT K, S, C, ROW_NUMBER() OVER (ORDER BY S DESC, K) AS R, MIN(S) OVER () AS LO, MAX(S) OVER () AS HI \
         FROM ( SELECT K, S, C, ROW_NUMBER() OVER (PARTITION BY K ORDER BY S DESC, C) AS B \
         FROM ( SELECT {DOCUMENTS}.{key} AS K, RANK({DOCUMENTS}.{text}, ",
        key = r.key,
        text = r.text,
    ));
    w.pattern(search).put(") AS S, ");
    match &read.chunks {
        Some(chunks) => {
            let (id, text, key) = (chunk_id(), &read.nearest.text, &read.nearest.key);
            w.put(&format!(
                "( SELECT {CHUNKS}.{text} FROM {chunks} AS {CHUNKS} WHERE {CHUNKS}.{key} = {DOCUMENTS}.{doc} \
                 ORDER BY RANK({CHUNKS}.{text}, ",
                doc = r.key,
            ));
            w.pattern(search).put(&format!(") DESC, {CHUNKS}.{id} FETCH FIRST 1 ROW ONLY ) AS C"));
        }
        None => {
            w.put(&format!("{DOCUMENTS}.{} AS C", r.text));
        }
    }
    w.put(&format!(
        " FROM {table} AS {DOCUMENTS} ORDER BY S DESC NULLS LAST FETCH FIRST {candidates} ROWS ONLY ) AS HYBRID_SEARCH_RANKED \
         WHERE S > 0 ) AS {BEST} WHERE B = 1",
        table = r.table,
    ));
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
