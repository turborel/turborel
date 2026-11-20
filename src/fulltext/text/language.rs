//! The languages the tokenizer knows [ISO/IEC 13249-2:2002 (CD, SC32
//! N0640) §4.4], each with how its words are found, normalized, stemmed
//! and marked as stop words (RULES.md D321). A language is named by its
//! English name in upper case, which a pattern's `<language
//! specification>` and FULLTEXT(text, language) write [§5.3.1 Description
//! 3c, §5.1.9].
//!
//! - NONE has no stemmer and no stop words, and ENGLISH Snowball's
//!   English stemmer and stop words. Both take accents off.
//! - Every other language of Snowball 3.2.0 has its Snowball stemmer, and
//!   its Snowball stop words where Snowball has a list. Their words keep
//!   their accents, which their stemmers read, and Turkish folds `I` to
//!   dotless `ı` and `İ` to `i`.
//! - French, Italian, Catalan and Irish take an elided article or
//!   pronoun off the word it is written on, `l'homme` read as `homme`, and
//!   Catalan joins the letters around a middle dot, `col·lecció`.
//! - Chinese, Japanese, Korean, Thai, Lao, Khmer and Burmese, written
//!   with no spaces between words, are read as character bigrams (D323).

use std::sync::OnceLock;

use super::snowball::Stemmer;
use crate::error::{Code, Result, SqlError, SqlState};

/// A language the tokenizer knows [§4.4]: how it finds, normalizes and
/// stems words, and which it takes for stop words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Language {
    /// No language: no stem but the word, and no stop words.
    None,
    /// English: Snowball's English stems and stop words.
    English,
    /// Arabic, by Snowball's stemmer.
    Arabic,
    /// Armenian, by Snowball's stemmer.
    Armenian,
    /// Basque, by Snowball's stemmer.
    Basque,
    /// Catalan, by Snowball's stemmer.
    Catalan,
    /// Czech, by Snowball's stemmer.
    Czech,
    /// Danish, by Snowball's stemmer and stop words.
    Danish,
    /// Dutch, by Snowball's stemmer and stop words.
    Dutch,
    /// Esperanto, by Snowball's stemmer.
    Esperanto,
    /// Estonian, by Snowball's stemmer.
    Estonian,
    /// Finnish, by Snowball's stemmer and stop words.
    Finnish,
    /// French, by Snowball's stemmer and stop words.
    French,
    /// German, by Snowball's stemmer and stop words.
    German,
    /// Greek, by Snowball's stemmer.
    Greek,
    /// Hindi, by Snowball's stemmer.
    Hindi,
    /// Hungarian, by Snowball's stemmer and stop words.
    Hungarian,
    /// Indonesian, by Snowball's stemmer and stop words.
    Indonesian,
    /// Irish, by Snowball's stemmer and stop words.
    Irish,
    /// Italian, by Snowball's stemmer and stop words.
    Italian,
    /// Lithuanian, by Snowball's stemmer.
    Lithuanian,
    /// Nepali, by Snowball's stemmer.
    Nepali,
    /// Norwegian, by Snowball's stemmer and stop words.
    Norwegian,
    /// Persian, by Snowball's stemmer.
    Persian,
    /// Polish, by Snowball's stemmer.
    Polish,
    /// Portuguese, by Snowball's stemmer and stop words.
    Portuguese,
    /// Romanian, by Snowball's stemmer.
    Romanian,
    /// Russian, by Snowball's stemmer and stop words.
    Russian,
    /// Serbian, by Snowball's stemmer.
    Serbian,
    /// Sesotho, by Snowball's stemmer and stop words.
    Sesotho,
    /// Spanish, by Snowball's stemmer and stop words.
    Spanish,
    /// Swedish, by Snowball's stemmer and stop words.
    Swedish,
    /// Tamil, by Snowball's stemmer.
    Tamil,
    /// Turkish, by Snowball's stemmer.
    Turkish,
    /// Ukrainian, by Snowball's stemmer and stop words.
    Ukrainian,
    /// Yiddish, by Snowball's stemmer.
    Yiddish,
    /// Chinese, read as character bigrams.
    Chinese,
    /// Japanese, read as character bigrams.
    Japanese,
    /// Korean, read as character bigrams.
    Korean,
    /// Thai, read as character bigrams.
    Thai,
    /// Lao, read as character bigrams.
    Lao,
    /// Khmer, read as character bigrams.
    Khmer,
    /// Burmese, in the Myanmar script, read as character bigrams.
    Burmese,
}

/// How a language's words are normalized [§4.5].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fold {
    /// Case folded, canonically decomposed and folded again, nonspacing
    /// marks taken out.
    Strip,
    /// Case folded and canonically composed (NFC), marks kept.
    Keep,
    /// As `Keep`, `I` folded first to `ı` and `İ` to `i`.
    Turkish,
    /// Case folded and composed for compatibility (NFKC), so full-width
    /// and half-width forms are one.
    Width,
}

/// How a language's text is cut into words [§4.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Cut {
    /// Runs of letters, numbers and marks, each ideograph or kana a word.
    Words,
    /// As `Words`, but a run of characters of the scripts written without
    /// spaces is read as its overlapping bigrams (D323).
    Bigrams,
}

/// What the tokenizer does in one language.
#[derive(Debug)]
pub(super) struct Profile {
    pub language: Language,
    pub name: &'static str,
    /// The Snowball program of its stemmer, if it stems.
    pub program: Option<&'static str>,
    /// Its stop words in Snowball's list format, if it has a list.
    pub stop: Option<&'static str>,
    pub fold: Fold,
    pub cut: Cut,
    /// The articles and pronouns elided before an apostrophe that are
    /// taken off the word they are written on.
    pub elisions: &'static [&'static str],
    /// Whether a middle dot between letters joins them.
    pub middle_dot: bool,
}

/// A language of a Snowball stemmer and the list of its stop words.
const fn snowball(language: Language, name: &'static str, program: &'static str, stop: Option<&'static str>) -> Profile {
    Profile { language, name, program: Some(program), stop, fold: Fold::Keep, cut: Cut::Words, elisions: &[], middle_dot: false }
}

/// A language read as character bigrams.
const fn bigrams(language: Language, name: &'static str) -> Profile {
    Profile { language, name, program: None, stop: None, fold: Fold::Width, cut: Cut::Bigrams, elisions: &[], middle_dot: false }
}

/// The French elisions, Snowball's French stemmer's: a single letter of
/// `cdjlmnstz`, or `qu`.
const FRENCH_ELISIONS: [&str; 10] = ["c", "d", "j", "l", "m", "n", "s", "t", "z", "qu"];
/// The Italian elisions, Snowball's Italian stemmer's.
const ITALIAN_ELISIONS: [&str; 17] =
    ["d", "l", "m", "s", "t", "v", "all", "dall", "dell", "gl", "nell", "quell", "quest", "sull", "tutt", "un", "c"];
/// The Catalan elisions: the articles, preposition and pronouns written
/// with an apostrophe.
const CATALAN_ELISIONS: [&str; 6] = ["d", "l", "m", "n", "s", "t"];
/// The Irish elisions, Snowball's Irish stemmer's.
const IRISH_ELISIONS: [&str; 3] = ["b", "d", "m"];

/// The Snowball programs (`snowball/algorithms`).
mod programs {
    pub const ARABIC: &str = include_str!("snowball/algorithms/arabic.sbl");
    pub const ARMENIAN: &str = include_str!("snowball/algorithms/armenian.sbl");
    pub const BASQUE: &str = include_str!("snowball/algorithms/basque.sbl");
    pub const CATALAN: &str = include_str!("snowball/algorithms/catalan.sbl");
    pub const CZECH: &str = include_str!("snowball/algorithms/czech.sbl");
    pub const DANISH: &str = include_str!("snowball/algorithms/danish.sbl");
    pub const DUTCH: &str = include_str!("snowball/algorithms/dutch.sbl");
    pub const ENGLISH: &str = include_str!("snowball/algorithms/english.sbl");
    pub const ESPERANTO: &str = include_str!("snowball/algorithms/esperanto.sbl");
    pub const ESTONIAN: &str = include_str!("snowball/algorithms/estonian.sbl");
    pub const FINNISH: &str = include_str!("snowball/algorithms/finnish.sbl");
    pub const FRENCH: &str = include_str!("snowball/algorithms/french.sbl");
    pub const GERMAN: &str = include_str!("snowball/algorithms/german.sbl");
    pub const GREEK: &str = include_str!("snowball/algorithms/greek.sbl");
    pub const HINDI: &str = include_str!("snowball/algorithms/hindi.sbl");
    pub const HUNGARIAN: &str = include_str!("snowball/algorithms/hungarian.sbl");
    pub const INDONESIAN: &str = include_str!("snowball/algorithms/indonesian.sbl");
    pub const IRISH: &str = include_str!("snowball/algorithms/irish.sbl");
    pub const ITALIAN: &str = include_str!("snowball/algorithms/italian.sbl");
    pub const LITHUANIAN: &str = include_str!("snowball/algorithms/lithuanian.sbl");
    pub const NEPALI: &str = include_str!("snowball/algorithms/nepali.sbl");
    pub const NORWEGIAN: &str = include_str!("snowball/algorithms/norwegian.sbl");
    pub const PERSIAN: &str = include_str!("snowball/algorithms/persian.sbl");
    pub const POLISH: &str = include_str!("snowball/algorithms/polish.sbl");
    pub const PORTUGUESE: &str = include_str!("snowball/algorithms/portuguese.sbl");
    pub const ROMANIAN: &str = include_str!("snowball/algorithms/romanian.sbl");
    pub const RUSSIAN: &str = include_str!("snowball/algorithms/russian.sbl");
    pub const SERBIAN: &str = include_str!("snowball/algorithms/serbian.sbl");
    pub const SESOTHO: &str = include_str!("snowball/algorithms/sesotho.sbl");
    pub const SPANISH: &str = include_str!("snowball/algorithms/spanish.sbl");
    pub const SWEDISH: &str = include_str!("snowball/algorithms/swedish.sbl");
    pub const TAMIL: &str = include_str!("snowball/algorithms/tamil.sbl");
    pub const TURKISH: &str = include_str!("snowball/algorithms/turkish.sbl");
    pub const UKRAINIAN: &str = include_str!("snowball/algorithms/ukrainian.sbl");
    pub const YIDDISH: &str = include_str!("snowball/algorithms/yiddish.sbl");
}

/// The Snowball stop-word lists (`stop`).
mod stops {
    pub const DANISH: &str = include_str!("stop/danish.stop");
    pub const DUTCH: &str = include_str!("stop/dutch.stop");
    pub const ENGLISH: &str = include_str!("stop/english.stop");
    pub const FINNISH: &str = include_str!("stop/finnish.stop");
    pub const FRENCH: &str = include_str!("stop/french.stop");
    pub const GERMAN: &str = include_str!("stop/german.stop");
    pub const HUNGARIAN: &str = include_str!("stop/hungarian.stop");
    pub const INDONESIAN: &str = include_str!("stop/indonesian.stop");
    pub const IRISH: &str = include_str!("stop/irish.stop");
    pub const ITALIAN: &str = include_str!("stop/italian.stop");
    pub const NORWEGIAN: &str = include_str!("stop/norwegian.stop");
    pub const PORTUGUESE: &str = include_str!("stop/portuguese.stop");
    pub const RUSSIAN: &str = include_str!("stop/russian.stop");
    pub const SESOTHO: &str = include_str!("stop/sesotho.stop");
    pub const SPANISH: &str = include_str!("stop/spanish.stop");
    pub const SWEDISH: &str = include_str!("stop/swedish.stop");
    pub const UKRAINIAN: &str = include_str!("stop/ukrainian.stop");
}

/// How many languages there are.
const LANGUAGES: usize = 43;

/// Every language and what the tokenizer does in it.
const PROFILES: [Profile; LANGUAGES] = [
    Profile {
        language: Language::None,
        name: "NONE",
        program: None,
        stop: None,
        fold: Fold::Strip,
        cut: Cut::Words,
        elisions: &[],
        middle_dot: false,
    },
    Profile { fold: Fold::Strip, ..snowball(Language::English, "ENGLISH", programs::ENGLISH, Some(stops::ENGLISH)) },
    snowball(Language::Arabic, "ARABIC", programs::ARABIC, None),
    snowball(Language::Armenian, "ARMENIAN", programs::ARMENIAN, None),
    snowball(Language::Basque, "BASQUE", programs::BASQUE, None),
    Profile { elisions: &CATALAN_ELISIONS, middle_dot: true, ..snowball(Language::Catalan, "CATALAN", programs::CATALAN, None) },
    snowball(Language::Czech, "CZECH", programs::CZECH, None),
    snowball(Language::Danish, "DANISH", programs::DANISH, Some(stops::DANISH)),
    snowball(Language::Dutch, "DUTCH", programs::DUTCH, Some(stops::DUTCH)),
    snowball(Language::Esperanto, "ESPERANTO", programs::ESPERANTO, None),
    snowball(Language::Estonian, "ESTONIAN", programs::ESTONIAN, None),
    snowball(Language::Finnish, "FINNISH", programs::FINNISH, Some(stops::FINNISH)),
    Profile { elisions: &FRENCH_ELISIONS, ..snowball(Language::French, "FRENCH", programs::FRENCH, Some(stops::FRENCH)) },
    snowball(Language::German, "GERMAN", programs::GERMAN, Some(stops::GERMAN)),
    snowball(Language::Greek, "GREEK", programs::GREEK, None),
    snowball(Language::Hindi, "HINDI", programs::HINDI, None),
    snowball(Language::Hungarian, "HUNGARIAN", programs::HUNGARIAN, Some(stops::HUNGARIAN)),
    snowball(Language::Indonesian, "INDONESIAN", programs::INDONESIAN, Some(stops::INDONESIAN)),
    Profile { elisions: &IRISH_ELISIONS, ..snowball(Language::Irish, "IRISH", programs::IRISH, Some(stops::IRISH)) },
    Profile { elisions: &ITALIAN_ELISIONS, ..snowball(Language::Italian, "ITALIAN", programs::ITALIAN, Some(stops::ITALIAN)) },
    snowball(Language::Lithuanian, "LITHUANIAN", programs::LITHUANIAN, None),
    snowball(Language::Nepali, "NEPALI", programs::NEPALI, None),
    snowball(Language::Norwegian, "NORWEGIAN", programs::NORWEGIAN, Some(stops::NORWEGIAN)),
    snowball(Language::Persian, "PERSIAN", programs::PERSIAN, None),
    snowball(Language::Polish, "POLISH", programs::POLISH, None),
    snowball(Language::Portuguese, "PORTUGUESE", programs::PORTUGUESE, Some(stops::PORTUGUESE)),
    snowball(Language::Romanian, "ROMANIAN", programs::ROMANIAN, None),
    snowball(Language::Russian, "RUSSIAN", programs::RUSSIAN, Some(stops::RUSSIAN)),
    snowball(Language::Serbian, "SERBIAN", programs::SERBIAN, None),
    snowball(Language::Sesotho, "SESOTHO", programs::SESOTHO, Some(stops::SESOTHO)),
    snowball(Language::Spanish, "SPANISH", programs::SPANISH, Some(stops::SPANISH)),
    snowball(Language::Swedish, "SWEDISH", programs::SWEDISH, Some(stops::SWEDISH)),
    snowball(Language::Tamil, "TAMIL", programs::TAMIL, None),
    Profile { fold: Fold::Turkish, ..snowball(Language::Turkish, "TURKISH", programs::TURKISH, None) },
    snowball(Language::Ukrainian, "UKRAINIAN", programs::UKRAINIAN, Some(stops::UKRAINIAN)),
    snowball(Language::Yiddish, "YIDDISH", programs::YIDDISH, None),
    bigrams(Language::Chinese, "CHINESE"),
    bigrams(Language::Japanese, "JAPANESE"),
    bigrams(Language::Korean, "KOREAN"),
    bigrams(Language::Thai, "THAI"),
    bigrams(Language::Lao, "LAO"),
    bigrams(Language::Khmer, "KHMER"),
    bigrams(Language::Burmese, "BURMESE"),
];

/// Each language's stemmer, read the first time it stems a word: none of
/// a language that does not stem, or were its program damaged.
static STEMMERS: [OnceLock<Option<Stemmer>>; LANGUAGES] = [const { OnceLock::new() }; LANGUAGES];

/// Each language's stop words, normalized as its words are and sorted,
/// read the first time one is asked for.
static STOP_WORDS: [OnceLock<Vec<String>>; LANGUAGES] = [const { OnceLock::new() }; LANGUAGES];

/// What begins a comment in a Snowball stop-word list, to the line's end.
const STOP_COMMENT: char = '|';

impl Language {
    /// The language named `name`, its case not counting.
    ///
    /// # Errors
    ///
    /// SQLSTATE 2FF02 if the tokenizer knows none of that name
    /// [§5.1.9 GR].
    pub fn named(name: &str) -> Result<Language> {
        PROFILES.iter().find(|profile| profile.name.eq_ignore_ascii_case(name)).map(|profile| profile.language).ok_or_else(|| {
            SqlError::new(Code::Data, format!("[ISO/IEC 13249-2:2002 §5.1.9] no language is named {}", name.to_ascii_uppercase()))
                .with_state(SqlState::INVALID_LANGUAGE_SPECIFICATION)
        })
    }

    /// Its name.
    pub fn name(self) -> &'static str {
        self.profile().name
    }

    /// Every language the tokenizer knows, NONE and ENGLISH first, then
    /// the stemmed languages and those read as bigrams, each in the order
    /// of its name.
    pub fn all() -> impl Iterator<Item = Language> {
        PROFILES.iter().map(|profile| profile.language)
    }

    /// Whether it has a stemmer.
    pub fn stems(self) -> bool {
        self.profile().program.is_some()
    }

    /// Whether it has stop words of its own.
    pub fn has_stop_words(self) -> bool {
        self.profile().stop.is_some()
    }

    /// Whether its text is read as character bigrams (D323).
    pub fn reads_bigrams(self) -> bool {
        self.profile().cut == Cut::Bigrams
    }

    /// Its place in `PROFILES`.
    fn place(self) -> usize {
        PROFILES.iter().position(|profile| profile.language == self).unwrap_or(0)
    }

    /// What the tokenizer does in it.
    pub(super) fn profile(self) -> &'static Profile {
        PROFILES.get(self.place()).unwrap_or(&PROFILES[0])
    }

    /// Its stemmer, read the first time it is asked for; none if it does
    /// not stem.
    pub(super) fn stemmer(self) -> Option<&'static Stemmer> {
        let program = self.profile().program?;
        STEMMERS.get(self.place())?.get_or_init(|| Stemmer::compile(program).ok()).as_ref()
    }

    /// Its stop words, normalized as `normalize` normalizes its words,
    /// sorted, each once; none if it has no list.
    pub(super) fn stop_list(self, normalize: impl Fn(&str) -> String) -> &'static [String] {
        let Some(list) = self.profile().stop else { return &[] };
        let Some(cell) = STOP_WORDS.get(self.place()) else { return &[] };
        cell.get_or_init(|| {
            let mut words: Vec<String> =
                list.lines().filter_map(|line| line.split(STOP_COMMENT).next()).flat_map(str::split_whitespace).map(normalize).collect();
            words.sort_unstable();
            words.dedup();
            words
        })
    }
}

/// A set of languages, as the languages of the documents an index holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Languages(u64);

impl Languages {
    /// The set of `language` alone.
    pub fn of(language: Language) -> Languages {
        Languages::default().with(language)
    }

    /// The set with `language` too.
    #[must_use]
    pub fn with(self, language: Language) -> Languages {
        Languages(self.0 | Languages::bit(language))
    }

    /// The set of the bits `bits`, as `bits` gives them.
    pub fn from_bits(bits: u64) -> Languages {
        Languages(bits)
    }

    /// Its bits, one a language by its place.
    pub fn bits(self) -> u64 {
        self.0
    }

    /// Whether it holds `language`.
    pub fn contains(self, language: Language) -> bool {
        self.0 & Languages::bit(language) != 0
    }

    /// Its languages, in the order of `Language::all`.
    pub fn iter(self) -> impl Iterator<Item = Language> {
        Language::all().filter(move |language| self.contains(*language))
    }

    /// The bit of `language`.
    fn bit(language: Language) -> u64 {
        u32::try_from(language.place()).ok().and_then(|place| 1_u64.checked_shl(place)).unwrap_or(0)
    }
}
