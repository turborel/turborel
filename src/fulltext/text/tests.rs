//! Unit tests of the tokenizer: words, sentences, paragraphs, places,
//! normalization, stop words and streaming.

mod english;
mod languages;
mod vocabulary;

use std::cell::Cell;

use super::*;
use crate::error::{Code, SqlError, SqlState};

/// The tokens of `text` in `language`.
fn tokens_in(text: &str, language: Language) -> Vec<Token> {
    tokenize(text.chars().map(Ok), language).map(Result::unwrap).collect()
}

/// The tokens of `text` in English.
fn tokens(text: &str) -> Vec<Token> {
    tokens_in(text, Language::English)
}

/// The words of `text`.
fn words(text: &str) -> Vec<String> {
    tokens(text).into_iter().map(|t| t.word).collect()
}

/// The sentence and paragraph of each word of `text`.
fn places(text: &str) -> Vec<(u32, u32)> {
    tokens(text).iter().map(|t| (t.sentence, t.paragraph)).collect()
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.9] a language is named in any
/// case, and its name names it again.
#[test]
fn language_named_in_any_case() {
    assert_eq!(Language::named("english").unwrap(), Language::English);
    assert_eq!(Language::named("None").unwrap(), Language::None);
    for language in [Language::English, Language::None] {
        assert_eq!(Language::named(language.name()).unwrap(), language);
    }
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.9] an unknown language is
/// SQLSTATE 2FF02.
#[test]
fn unknown_language_is_2ff02() {
    let e = Language::named("klingon").unwrap_err();
    assert_eq!(e.sqlstate(), SqlState::INVALID_LANGUAGE_SPECIFICATION);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] a word is a run of letters,
/// numbers and marks; anything else ends it.
#[test]
fn words_are_runs_of_letters_numbers_and_marks() {
    assert_eq!(words("Hello, world-42x; (3rd) a+b"), ["hello", "world", "42x", "3rd", "a", "b"]);
    assert_eq!(words("   "), Vec::<String>::new());
    assert_eq!(words(""), Vec::<String>::new());
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] one connector joins two runs;
/// a connector at either end, or two in a row, joins nothing.
#[test]
fn connector_joins_two_runs() {
    assert_eq!(words("foo_bar snake_case_name"), ["foo_bar", "snake_case_name"]);
    assert_eq!(words("trail_ _lead a__b x_1"), ["trail", "lead", "a", "b", "x_1"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] an apostrophe joins two letters
/// and nothing else: MC/DC of the letter before (`5'a`) and the letter
/// after (`a'5`), and the right quote U+2019 written as U+0027.
#[test]
fn apostrophe_joins_two_letters() {
    assert_eq!(words("don't rock'n'roll"), ["don't", "rock'n'roll"]);
    assert_eq!(words("'quoted' dogs' 'tis"), ["quoted", "dogs", "tis"]);
    assert_eq!(words("5'a a'5"), ["5", "a", "a", "5"]);
    assert_eq!(words("don\u{2019}t"), ["don't"]);
    assert_eq!(words("o''clock"), ["o", "clock"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] each ideograph or kana is a word
/// by itself, with the marks after it, and joins nothing.
#[test]
fn ideographs_are_words_by_themselves() {
    assert_eq!(words("漢字abcカタ"), ["漢", "字", "abc", "カ", "タ"]);
    assert_eq!(words("x漢_y"), ["x", "漢", "y"]);
    let marked = tokens("か\u{3099}た");
    assert_eq!(marked.len(), 2);
    assert_eq!((marked[0].offset, marked[0].length), (0, 2));
    assert_eq!(marked[0].word, "か");
    assert_eq!(words("𠀀𪜀"), ["𠀀", "𪜀"]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.5] offsets and lengths count
/// characters as written, from 0.
#[test]
fn offsets_and_lengths_count_characters() {
    let found = tokens("é Re\u{301}sume\u{301} x");
    let at: Vec<(u32, u32)> = found.iter().map(|t| (t.offset, t.length)).collect();
    assert_eq!(at, [(0, 1), (2, 8), (11, 1)]);
    assert_eq!(found[1].word, "resume");
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.5 DR7-DR9] ordinals count every
/// word from 1, stop words too.
#[test]
fn ordinals_count_stop_words() {
    let found = tokens("The cat and the hat");
    let ordinals: Vec<u32> = found.iter().map(|t| t.ordinal).collect();
    assert_eq!(ordinals, [1, 2, 3, 4, 5]);
    let stops: Vec<bool> = found.iter().map(|t| t.stop).collect();
    assert_eq!(stops, [true, false, true, true, false]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.4.2] a document in NONE has no stop
/// words.
#[test]
fn none_marks_no_stop_words() {
    assert!(tokens_in("the and of", Language::None).iter().all(|t| !t.stop));
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] a terminator then a separator or
/// the end ends a sentence; one without a separator after it does not.
#[test]
fn sentences_end_at_terminator_and_separator() {
    assert_eq!(places("One two. Three! Four? Five"), [(1, 1), (1, 1), (2, 1), (3, 1), (4, 1)]);
    assert_eq!(places("pi is 3.14 e.g.x done."), [(1, 1); 8]);
    assert_eq!(places("a.\tb.\nc"), [(1, 1), (2, 1), (3, 1)]);
    assert_eq!(places("a.\u{A0}b"), [(1, 1), (2, 1)]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] closing quotes, brackets and
/// further terminators stay in the sentence they close.
#[test]
fn closers_stay_in_their_sentence() {
    assert_eq!(places("He said \"go.\" Then (yes.) next"), [(1, 1), (1, 1), (1, 1), (2, 1), (2, 1), (3, 1)]);
    assert_eq!(places("Really?!\u{201D} Yes. 'No.' x"), [(1, 1), (2, 1), (3, 1), (4, 1)]);
    assert_eq!(places("a.)b"), [(1, 1), (1, 1)]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] a sentence holding no word is
/// not counted, before or between words.
#[test]
fn empty_sentences_do_not_count() {
    assert_eq!(places(". ! a. . ? b"), [(1, 1), (2, 1)]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] ideographic and full-width
/// terminators end a sentence by themselves.
#[test]
fn wide_terminators_end_sentences_by_themselves() {
    assert_eq!(places("日本。東京！大阪？京都．奈良"), [(1, 1), (1, 1), (2, 1), (2, 1), (3, 1), (3, 1), (4, 1), (4, 1), (5, 1), (5, 1)]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] two line terminators in a row,
/// blanks between, end a paragraph and its sentence; one does not.
#[test]
fn paragraphs_end_at_two_line_terminators() {
    assert_eq!(places("a\nb"), [(1, 1), (1, 1)]);
    assert_eq!(places("a\n\nb"), [(1, 1), (2, 2)]);
    assert_eq!(places("a\n \t\nb"), [(1, 1), (2, 2)]);
    assert_eq!(places("a\n x\nb"), [(1, 1), (1, 1), (1, 1)]);
    assert_eq!(places("a\r\rb\u{2028}\u{2028}c\u{C}\u{B}d\u{85}\u{85}e"), [(1, 1), (2, 2), (3, 3), (4, 4), (5, 5)]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] CRLF counts once, so one CRLF
/// does not end a paragraph and two do.
#[test]
fn crlf_counts_once() {
    assert_eq!(places("a\r\nb"), [(1, 1), (1, 1)]);
    assert_eq!(places("a\r\n\r\nb"), [(1, 1), (2, 2)]);
    assert_eq!(places("a\r \nb"), [(1, 1), (2, 2)]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] PARAGRAPH SEPARATOR ends a
/// paragraph by itself; leading and repeated ends count no empty one.
#[test]
fn paragraph_separator_ends_a_paragraph() {
    assert_eq!(places("a\u{2029}b"), [(1, 1), (2, 2)]);
    assert_eq!(places("\n\n\u{2029}a\n\n\n\n\u{2029}b"), [(1, 1), (2, 2)]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.1] a terminator before a line end
/// ends the sentence but not the paragraph.
#[test]
fn line_end_separates_a_terminated_sentence() {
    assert_eq!(places("a.\nb\nc.\r\nd"), [(1, 1), (2, 1), (2, 1), (3, 1)]);
}

/// Verifies: [DESIGN] a word keeps its first `MAX_WORD_CHARS` characters
/// and counts all of them.
#[test]
fn long_word_keeps_its_first_characters() {
    let long = "a".repeat(1000);
    let found = tokens(&format!("{long} b"));
    assert_eq!(found[0].word.chars().count(), usize::try_from(MAX_WORD_CHARS).unwrap());
    assert_eq!((found[0].length, found[1].offset), (1000, 1001));
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.5] normalization folds case,
/// decomposes and takes nonspacing marks out; ideographs pass through.
#[test]
fn normalize_folds_and_strips_marks() {
    assert_eq!(normalize("Résumé"), "resume");
    assert_eq!(normalize("ÉCOLE"), "ecole");
    assert_eq!(normalize("Re\u{301}sume\u{301}"), "resume");
    assert_eq!(normalize("\u{212B}ngstro\u{308}m"), "angstrom");
    assert_eq!(normalize("ΣΊΣΥΦΟΣ"), "σισυφοσ");
    assert_eq!(normalize("漢字"), "漢字");
    assert_eq!(normalize("don\u{2019}t"), "don't");
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.4.2] the English stop words are
/// the Snowball list, sorted for the search, matched normalized.
#[test]
fn english_stop_words() {
    let english = stop_list(Language::English);
    assert_eq!(english.len(), 174);
    assert!(english.windows(2).all(|w| w[0] < w[1]));
    for word in ["the", "a", "don't", "yourselves", "very"] {
        assert!(is_stop(Language::English, word), "{word}");
    }
    assert!(!is_stop(Language::English, "The"));
    assert!(!is_stop(Language::English, "database"));
    assert!(english.iter().all(|w| normalize(w) == *w));
    assert!(english.iter().all(|w| !is_stop(Language::None, w)));
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] NONE stems a word to itself.
#[test]
fn none_stems_to_the_word() {
    assert_eq!(stem(Language::None, "running"), "running");
    assert_eq!(stem(Language::English, "running"), "run");
}

/// Characters of a document of `length` characters, made as they are read,
/// counting how many were read in `read`.
fn generated(length: usize, read: &Cell<usize>) -> impl Iterator<Item = Result<char>> + '_ {
    const PATTERN: &str = "Lorem ipsum dolor sit amet. Consectetur adipiscing elit!\n\n";
    PATTERN.chars().cycle().take(length).map(move |c| {
        read.set(read.get().saturating_add(1));
        Ok(c)
    })
}

/// Verifies: [DESIGN] RULES.md D86 a document of 10 MB streams through:
/// the first word comes after reading only its characters and one more,
/// and every word is counted in the end.
#[test]
fn ten_megabytes_stream_through() {
    const LENGTH: usize = 10 * 1024 * 1024;
    let read = Cell::new(0);
    let mut tokens = tokenize(generated(LENGTH, &read), Language::English);
    assert_eq!(tokens.next().unwrap().unwrap().word, "lorem");
    assert_eq!(read.get(), 6);
    let mut count = 1u32;
    let mut last = 1u32;
    for token in tokens {
        let token = token.unwrap();
        count = count.saturating_add(1);
        let second = ["consectetur", "adipiscing", "elit"].contains(&token.word.as_str());
        assert_eq!(token.sentence, token.paragraph.saturating_mul(2).saturating_sub(u32::from(!second)));
        last = token.paragraph;
        assert_eq!(token.ordinal, count);
    }
    assert_eq!(read.get(), LENGTH);
    assert!(last > 170_000);
}

/// The error the tests' documents fail with.
fn failure() -> SqlError {
    SqlError::new(Code::Corrupt, "[DESIGN] the document cannot be read")
}

/// Verifies: [DESIGN] a reading error is passed through as the stream's
/// last item, dropping the word being read.
#[test]
fn reading_error_ends_the_stream() {
    let chars = vec![Ok('a'), Ok('b'), Err(failure()), Ok('c')];
    let mut tokens = tokenize(chars.into_iter(), Language::English);
    assert_eq!(tokens.next().unwrap().unwrap_err().sqlcode(), -916_i32);
    assert!(tokens.next().is_none());
    assert!(tokens.next().is_none());
}

/// Verifies: [DESIGN] the words before a reading error are yielded, then
/// the error, then nothing.
#[test]
fn words_before_a_reading_error_are_kept() {
    let chars = vec![Ok('a'), Ok(' '), Ok('b'), Ok('_'), Err(failure())];
    let found: Vec<Result<Token>> = tokenize(chars.into_iter(), Language::English).collect();
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].as_ref().unwrap().word, "a");
    assert!(found[1].is_err());
}

/// Verifies: [DESIGN] a joiner at the end of the document joins nothing.
#[test]
fn joiner_at_the_end_joins_nothing() {
    assert_eq!(words("ab_"), ["ab"]);
    assert_eq!(words("ab'"), ["ab"]);
}

/// Verifies: [DESIGN] the kinds and normalization known for ASCII are
/// those the tables give.
#[test]
fn ascii_shortcut_agrees_with_the_tables() {
    let tables = tables().unwrap();
    for c in (0u8..0x80).map(char::from) {
        assert_eq!(tables.category_kind(c), tables.table_kind(c), "{c:?}");
        let mut table_normalized = String::new();
        tables.normalize_char(c, &mut Vec::new(), &mut table_normalized);
        assert_eq!(normalize(&c.to_string()), table_normalized, "{c:?}");
    }
}

/// The stop words `words` already normalized.
fn listed(words: &[&str]) -> StopWords {
    StopWords::Words(std::sync::Arc::new(words.iter().map(|w| (*w).to_owned()).collect()))
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.4.2] a stop-word list is split at
/// commas, each word trimmed and normalized, duplicates and empties
/// dropped, and a list of no word is no stop words.
#[test]
fn stop_words_parse_a_list() {
    assert_eq!(StopWords::parse(" The ,RÉSUMÉ,, résumé\t,don\u{2019}t ", Language::English), listed(&["the", "resume", "don't"]));
    assert_eq!(StopWords::parse("a", Language::English), listed(&["a"]));
    assert_eq!(StopWords::parse("", Language::English), StopWords::None);
    assert_eq!(StopWords::parse(" , ,", Language::English), StopWords::None);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.4.2] the language's list, none, or
/// exactly the words given.
#[test]
fn stop_words_contain() {
    assert!(StopWords::Language.contains(Language::English, "the"));
    assert!(!StopWords::Language.contains(Language::None, "the"));
    assert!(!StopWords::None.contains(Language::English, "the"));
    let words = StopWords::parse("foo, Bar", Language::English);
    assert!(words.contains(Language::English, "bar"));
    assert!(words.contains(Language::None, "foo"));
    assert!(!words.contains(Language::English, "the"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.4.2, §5.1.5 DR7] tokens are marked
/// by the stop words given, ordinals unchanged; `tokenize` is the
/// language's list.
#[test]
fn tokenize_with_marks_by_the_given_stop_words() {
    let marks = |stop: &StopWords| -> Vec<(u32, bool)> {
        tokenize_with("The cat and Foo".chars().map(Ok), Language::English, stop).map(|t| t.map(|t| (t.ordinal, t.stop)).unwrap()).collect()
    };
    assert_eq!(marks(&StopWords::Language), [(1, true), (2, false), (3, true), (4, false)]);
    assert_eq!(marks(&StopWords::None), [(1, false), (2, false), (3, false), (4, false)]);
    assert_eq!(marks(&StopWords::parse("foo,cat", Language::English)), [(1, false), (2, true), (3, false), (4, true)]);
    let plain: Vec<bool> = tokens("The cat and Foo").iter().map(|t| t.stop).collect();
    assert_eq!(plain, [true, false, true, false]);
}
