//! \[EXTENSION\] A full-text index's options (RULES.md D41, D86): the
//! weights of Okapi BM25 ranking, after Robertson and Zaragoza, "The
//! Probabilistic Relevance Framework: BM25 and Beyond", 2009, §3.4, which
//! change in place; and how its documents are read, its language and stop
//! words [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §4.4], which its postings
//! and hashes depend on, so a change makes the index anew. How documents
//! are read is kept in its locator too, so an index opened reads them as
//! it was made to.

use std::collections::BTreeSet;
use std::sync::Arc;

use super::document::Reading;
use crate::bytes::{Put, Reader};
use crate::catalog::IndexDesc;
use crate::disk::pager::PageId;
use crate::error::{Code, Result, SqlError};
use crate::fulltext::Language;
use crate::fulltext::rank::{BM25, Ranking, Rankings, named};
use crate::fulltext::text::StopWords;
use crate::settings::Kind;
use crate::storage::{Rebuild, TableOption, option_value};
use crate::value::{MAX_CHARACTER_LENGTH, Value};

/// The option of BM25's term frequency saturation, k1.
pub const K1_OPTION: &str = "FULLTEXT.K1";
/// The option of BM25's length normalization, b.
pub const B_OPTION: &str = "FULLTEXT.B";
/// The option of the language an index reads its documents in.
pub const LANGUAGE_OPTION: &str = "FULLTEXT.LANGUAGE";
/// The option of the ranking an index's Rank uses, by its name
/// (`crate::fulltext::rank`).
pub const RANKING_OPTION: &str = "FULLTEXT.RANKING";
/// The option of an index's stop words: `DEFAULT_STOP_WORDS`,
/// `NO_STOP_WORDS`, or words by commas.
pub const STOP_WORDS_OPTION: &str = "FULLTEXT.STOP_WORDS";
/// The stop words option's value for the language's own list, its default.
pub const DEFAULT_STOP_WORDS: &str = "DEFAULT";
/// The stop words option's value for none.
pub const NO_STOP_WORDS: &str = "NONE";
/// k1's default, as Robertson and Zaragoza suggest (§3.4.1).
pub const DEFAULT_K1: f64 = 1.2;
/// b's default, as Robertson and Zaragoza suggest (§3.4.1).
pub const DEFAULT_B: f64 = 0.75;
/// \[DESIGN\] k1's greatest: far past where its saturation means anything.
const MOST_K1: f64 = 1_000.0;
/// The format of a locator: the format, the tree's meta page, the
/// language, the stop words.
const LOCATOR_VERSION: u8 = 1;

/// How a locator writes each kind of stop words.
const STOP_LANGUAGE: u8 = 0;
/// No stop words.
const STOP_NONE: u8 = 1;
/// A list of stop words, its count then each.
const STOP_LIST: u8 = 2;

/// \[EXTENSION\] The weights an index ranks by (Okapi BM25).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    /// k1: how fast a term's frequency saturates.
    pub k1: f64,
    /// b: how much a document's length is normalized, from 0 to 1.
    pub b: f64,
}

impl Default for Weights {
    fn default() -> Weights {
        Weights { k1: DEFAULT_K1, b: DEFAULT_B }
    }
}

/// An index's options, as its descriptor sets them.
pub(super) struct Options {
    /// How it reads its documents.
    pub reading: Reading,
    /// The weights it ranks by.
    pub weights: Weights,
    /// The name of its ranking, as set.
    pub ranking: String,
}

/// The number option `name` of `declared` `set` gives.
fn number(declared: &[TableOption], set: &[(String, Value)], name: &str, default: f64) -> f64 {
    option_value(declared, set, name).and_then(Value::as_f64).unwrap_or(default)
}

/// The stop words of the option's value `text`: the language's for
/// `DEFAULT`, none for `NONE`, else the words it lists by commas, none if
/// it lists none, either word's case not counting; each word normalized
/// as `language` normalizes its words.
pub(super) fn stop_words_of(text: &str, language: Language) -> StopWords {
    let text = text.trim();
    if text.eq_ignore_ascii_case(DEFAULT_STOP_WORDS) {
        StopWords::Language
    } else if text.eq_ignore_ascii_case(NO_STOP_WORDS) {
        StopWords::None
    } else {
        StopWords::parse(text, language)
    }
}

/// The options `desc` sets, the others their defaults.
///
/// # Errors
///
/// SQLCODE -103 for a language the tokenizer does not know, or a weight
/// out of its range.
pub(super) fn options_of(desc: &IndexDesc) -> Result<Options> {
    let declared = options();
    let weights =
        Weights { k1: number(&declared, &desc.options, K1_OPTION, DEFAULT_K1), b: number(&declared, &desc.options, B_OPTION, DEFAULT_B) };
    if !(0.0_f64..=MOST_K1).contains(&weights.k1) || !(0.0_f64..=1.0_f64).contains(&weights.b) {
        return Err(SqlError::new(Code::SyntaxRule, "[EXTENSION] FULLTEXT.K1 is from 0 to 1000 and FULLTEXT.B from 0 to 1"));
    }
    let language = match option_value(&declared, &desc.options, LANGUAGE_OPTION) {
        Some(Value::Char(name)) => language_named(name)?,
        Some(_) | None => Language::English,
    };
    let stop = match option_value(&declared, &desc.options, STOP_WORDS_OPTION) {
        Some(Value::Char(text)) => stop_words_of(text, language),
        Some(_) | None => StopWords::Language,
    };
    let ranking = match option_value(&declared, &desc.options, RANKING_OPTION) {
        Some(Value::Char(name)) => name.to_ascii_uppercase(),
        Some(_) | None => BM25.to_owned(),
    };
    Ok(Options { reading: Reading { language, stop }, weights, ranking })
}

/// The ranking of `rankings` named `name`: refused as any option's bad
/// value is, naming the rankings known.
///
/// # Errors
///
/// SQLCODE -103 if there is none of that name.
pub(super) fn ranking_named(rankings: &Rankings, name: &str) -> Result<Arc<dyn Ranking>> {
    named(rankings, name).ok_or_else(|| {
        let known: Vec<String> = rankings().iter().map(|ranking| ranking.name().to_owned()).collect();
        SqlError::new(Code::SyntaxRule, format!("[EXTENSION] FULLTEXT.RANKING is one of {}, not {name}", known.join(", ")))
    })
}

/// The language FULLTEXT.LANGUAGE names: refused as any option's bad
/// value is, saying which are known, as 43 are too many to list;
/// SQLSTATE 2FF02 stays for a language named where a statement runs
/// [§5.1.9 GR].
fn language_named(name: &str) -> Result<Language> {
    Language::named(name).map_err(|_| {
        let built_in = Language::all().count();
        let why =
            format!("[EXTENSION] FULLTEXT.LANGUAGE is one of the {built_in} built-in languages or one an extension serves, not {name}");
        SqlError::new(Code::SyntaxRule, why)
    })
}

/// The options its indexes take (RULES.md D41).
pub(super) fn options() -> Vec<TableOption> {
    let most = usize::try_from(MAX_CHARACTER_LENGTH).unwrap_or(usize::MAX);
    vec![
        TableOption::new(
            (K1_OPTION, Kind::Number { min: 0.0, max: MOST_K1 }, Value::Approx(DEFAULT_K1)),
            Rebuild::No,
            "BM25's k1: how fast a term's frequency in a document saturates its weight.",
        ),
        TableOption::new(
            (B_OPTION, Kind::Number { min: 0.0, max: 1.0 }, Value::Approx(DEFAULT_B)),
            Rebuild::No,
            "BM25's b: how much a document's weight is normalized by its length, from 0 to 1.",
        ),
        TableOption::new(
            (RANKING_OPTION, Kind::Identifier, Value::Char(BM25.to_owned())),
            Rebuild::No,
            "The ranking Rank uses, by its name: BM25, built in, or one registered.",
        ),
        TableOption::new(
            (LANGUAGE_OPTION, Kind::Identifier, Value::Char(Language::English.name().to_owned())),
            Rebuild::Yes,
            "The language documents are read in: how words are stemmed and, by default, which are stop words.",
        ),
        TableOption::new(
            (STOP_WORDS_OPTION, Kind::Text { max: most }, Value::Char(DEFAULT_STOP_WORDS.to_owned())),
            Rebuild::Yes,
            "The words not indexed: DEFAULT for the language's stop words, NONE for none, or the words by commas.",
        ),
    ]
}

/// The locator of an index whose tree's meta page is `meta`, reading its
/// documents as `reading` says.
pub(super) fn locator(meta: PageId, reading: &Reading) -> Vec<u8> {
    let mut out = vec![LOCATOR_VERSION];
    out.put_u64(meta.0);
    out.put_str(reading.language.name());
    match &reading.stop {
        StopWords::Language => out.push(STOP_LANGUAGE),
        StopWords::None => out.push(STOP_NONE),
        StopWords::Words(words) => {
            out.push(STOP_LIST);
            out.put_u32(u32::try_from(words.len()).unwrap_or(u32::MAX));
            for word in words.iter() {
                out.put_str(word);
            }
        }
    }
    out
}

/// The meta page and reading a locator holds.
///
/// # Errors
///
/// SQLCODE -916 if it is not one of this format; SQLSTATE 2FF02 if its
/// language is not known.
pub(super) fn located(locator: &[u8]) -> Result<(PageId, Reading)> {
    let mut reader = Reader::new(locator, "full-text index locator");
    if reader.u8()? != LOCATOR_VERSION {
        return Err(reader.corrupt("its version is not known"));
    }
    let meta = PageId(reader.u64()?);
    let language = Language::named(&reader.str()?)?;
    let stop = match reader.u8()? {
        STOP_LANGUAGE => StopWords::Language,
        STOP_NONE => StopWords::None,
        STOP_LIST => {
            let count = reader.u32()?;
            let words = (0..count).map(|_| reader.str()).collect::<Result<BTreeSet<String>>>()?;
            StopWords::Words(Arc::new(words))
        }
        _ => return Err(reader.corrupt("its stop words are of no known kind")),
    };
    reader.finish()?;
    Ok((meta, Reading { language, stop }))
}
