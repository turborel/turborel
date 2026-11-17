//! [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §9] The SQL/MM Full-Text
//! Information Schema, FT_INFORMATION_SCHEMA: FT_FEATURES, the features
//! supported and the implementation-defined constants, and FT_SCHEMATA,
//! the schemata in which its types and routines are found (RULES.md D87).
//!
//! Clause 10's base tables are "only a model" [§10.1], so each view is a
//! system table of the view's own columns, as SELECT on it is PUBLIC's and
//! nothing else is granted, "so they cannot be updated" [§9.1].

use super::definition::{information, schemas};
use super::{Seen, SystemTable, catalog_name, identifier, text, text_column};
use crate::catalog::ColumnDesc;
use crate::catalog::owner_of;
use crate::error::Result;
use crate::fulltext::thesaurus::{NAME_LENGTH, TERM_LENGTH};
use crate::fulltext::types::{FT_DEFAULT_LANGUAGE, FT_MAX_LANGUAGE_LENGTH, FT_MAX_TEXT_LENGTH};
use crate::storage::Row;

/// The schema of the SQL/MM Full-Text Information Schema [§9.1].
pub const FT_INFORMATION_SCHEMA: &str = "FT_INFORMATION_SCHEMA";

/// The length of a feature's name and value [§10.2].
const FEATURE_LENGTH: u32 = 256;

/// The character set of the full-text types' and routines' names
/// [§9.3 Description 2].
const SQL_TEXT: &str = "SQL_TEXT";

/// FT_MinRankValue: a BM25 score is a sum of term weights none of
/// which is negative, its inverse document frequency being the logarithm
/// of more than one (RULES.md D87).
const MIN_RANK: f64 = 0.0;

/// FT_MaxRankValue: the greatest DOUBLE PRECISION, as a score's bound is
/// the corpus's.
const MAX_RANK: f64 = f64::MAX;

/// A YES of FT_FEATURES.
const YES: &str = "YES";

/// [§9.2] FT_FEATURES.
pub(super) struct Features;

/// [§9.3] FT_SCHEMATA.
pub(super) struct Schemata;

/// The tables of the SQL/MM Full-Text Information Schema.
pub(super) fn tables() -> [std::sync::Arc<dyn SystemTable>; 2] {
    [std::sync::Arc::new(Features), std::sync::Arc::new(Schemata)]
}

/// Each feature of §10.2 and its value here: the limits of the types,
/// languages and thesauri, the ranks Rank gives, and every unit of
/// proximity and of context supported.
pub(crate) fn features() -> Vec<(&'static str, String)> {
    vec![
        ("FT_MaxTextLength", FT_MAX_TEXT_LENGTH.to_string()),
        ("FT_MaxLanguageLength", FT_MAX_LANGUAGE_LENGTH.to_string()),
        ("FT_DefaultLanguage", FT_DEFAULT_LANGUAGE.name().to_owned()),
        ("FT_ThesNameLength", NAME_LENGTH.to_string()),
        ("FT_ThesTermLength", TERM_LENGTH.to_string()),
        ("FT_MinRankValue", format!("{MIN_RANK:E}")),
        ("FT_MaxRankValue", format!("{MAX_RANK:E}")),
        ("FT_ProximityCharacters", YES.to_owned()),
        ("FT_ProximityWords", YES.to_owned()),
        ("FT_ProximitySentences", YES.to_owned()),
        ("FT_ProximityParagraphs", YES.to_owned()),
        ("FT_ContextSentence", YES.to_owned()),
        ("FT_ContextParagraph", YES.to_owned()),
    ]
}

impl SystemTable for Features {
    fn schema(&self) -> &'static str {
        FT_INFORMATION_SCHEMA
    }

    fn name(&self) -> &'static str {
        "FT_FEATURES"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        vec![text_column(("FT_FEATURENAME", FEATURE_LENGTH), true), text_column(("FT_FEATUREVALUE", FEATURE_LENGTH), true)]
    }

    fn rows(&self, _seen: &Seen<'_>) -> Result<Vec<Row>> {
        Ok(features().into_iter().map(|(name, value)| vec![text(name), text(&value)]).collect())
    }
}

impl SystemTable for Schemata {
    fn schema(&self) -> &'static str {
        FT_INFORMATION_SCHEMA
    }

    fn name(&self) -> &'static str {
        "FT_SCHEMATA"
    }

    fn columns(&self) -> Vec<ColumnDesc> {
        let names = ["CATALOG_NAME", "SCHEMA_NAME", "SCHEMA_OWNER"];
        let sets = ["DEFAULT_CHARACTER_SET_CATALOG", "DEFAULT_CHARACTER_SET_SCHEMA", "DEFAULT_CHARACTER_SET_NAME"];
        names.iter().map(|n| identifier(n, true)).chain(sets.iter().map(|n| identifier(n, false))).collect()
    }

    /// Each schema of the catalog, as INFORMATION_SCHEMA.SCHEMATA shows it
    /// [§9.3]: the full-text types and routines are built in, in no schema,
    /// so each schema holds the descriptors of a complete set of them
    /// [§9.3 Description 3], of the character set SQL_TEXT.
    fn rows(&self, seen: &Seen<'_>) -> Result<Vec<Row>> {
        let catalog = catalog_name(seen);
        let set = information(SQL_TEXT);
        let owner = |s: &str| text(owner_of(&*seen.state.catalog, s));
        let row = |s: &String| vec![catalog.clone(), text(s), owner(s), catalog.clone(), text(&set.auth), text(&set.ident)];
        Ok(schemas(seen).iter().map(row).collect())
    }
}
