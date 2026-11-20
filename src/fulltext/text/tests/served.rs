//! Languages a plug-in serves, by a language of the tests in place of a
//! plug-in (RULES.md D328).

use std::sync::Arc;

use crate::error::Result;
use crate::fulltext::text::served::{ServedLanguage, register};
use crate::fulltext::text::{Language, Languages, is_stop, stem, tokenize};

/// Words of two letters each, stems their first letter, `zz` a stop word.
#[derive(Debug)]
struct Pairs;

impl ServedLanguage for Pairs {
    fn base(&self) -> Language {
        Language::English
    }
    fn segments(&self) -> bool {
        true
    }
    fn stems(&self) -> bool {
        true
    }
    fn segment(&self, documents: &[&str]) -> Result<Vec<Vec<(u32, u32)>>> {
        let pairs = |d: &&str| (0..d.chars().count()).step_by(2).map(|k| (u32::try_from(k).unwrap(), 2)).collect();
        Ok(documents.iter().map(pairs).collect())
    }
    fn stem(&self, words: &[&str]) -> Result<Vec<String>> {
        Ok(words.iter().map(|w| w.chars().take(1).collect()).collect())
    }
    fn stop_words(&self) -> Option<&[String]> {
        None
    }
    fn prepare(&self) -> Result<()> {
        Ok(())
    }
}

/// Verifies: [EXTENSION] RULES.md D328: a served language is named as a
/// built-in one while it is registered; its text is cut by it, its words
/// normalized by its base, its sentences the built-in rules', its stems
/// its own and its stop words its base's; it has a bit of a set of
/// languages; a built-in name, or a name another extension serves, is
/// refused; once its registrations are gone, the name is no language.
#[test]
fn a_served_language_is_named_and_cut_by_its_plugin() {
    let served: Arc<dyn ServedLanguage> = Arc::new(Pairs);
    assert!(register("ENGLISH", "X", &served).is_err(), "the built-in language wins");
    let slot = register("pairs_test", "X", &served).unwrap();
    assert!(register("PAIRS_TEST", "Y", &served).is_err(), "another extension's");
    let language = Language::named("Pairs_Test").unwrap();
    assert_eq!((language, language.name()), (Language::Served(u8::try_from(slot).unwrap()), "PAIRS_TEST"));
    assert!(language.stems() && language.has_stop_words() && !language.reads_bigrams());
    let tokens: Vec<_> = tokenize("ABcd. Efgh".chars().map(Ok), language).map(Result::unwrap).collect();
    let words: Vec<(&str, u32, u32)> = tokens.iter().map(|t| (t.word.as_str(), t.offset, t.sentence)).collect();
    assert_eq!(words, [("ab", 0, 1), ("cd", 2, 1), (". ", 4, 1), ("ef", 6, 2), ("gh", 8, 2)], "as it cuts them");
    assert_eq!(stem(language, "ab").unwrap(), "a");
    assert!(is_stop(language, "the"), "English's stop words");
    let set = Languages::of(language).with(Language::French);
    assert_eq!(set.iter().collect::<Vec<_>>(), [Language::French, language]);
    drop(served);
    assert!(Language::named("PAIRS_TEST").is_err(), "no extension serves it now");
}
