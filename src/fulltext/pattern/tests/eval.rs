//! Evaluating each construct over small documents, true and false.

use super::*;
use crate::error::Code;

/// Verifies: [ISO/IEC 13249-2:2002 §6.4.2, §4.2.1, §4.5] a word matches a
/// token of the document by its normalized characters, in any case, with
/// or without accents; a word missing, or a prefix of one, does not.
#[test]
fn words() {
    assert!(holds(r#""fox""#, "The quick brown Fox."));
    assert!(holds(r#""FOX""#, "a fox"));
    assert!(holds(r#""Résumé""#, "my RESUME"));
    assert!(!holds(r#""cat""#, "The quick brown fox."));
    assert!(!holds(r#""qui""#, "The quick brown fox."));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.4.2, §4.2.3.1, NOTE 12] `_` matches
/// one character and `%` any, none too; an escaped one matches itself.
#[test]
fn masks_and_escapes() {
    assert!(holds(r#""qu%""#, "the quick fox"));
    assert!(holds(r#""%ick""#, "the quick fox"));
    assert!(holds(r#""qu%ck%""#, "the quick fox"));
    assert!(holds(r#""qu_ck""#, "the quick fox"));
    assert!(!holds(r#""q_ck""#, "the quick fox"));
    assert!(!holds(r#""fox_""#, "the quick fox"));
    assert!(holds(r#""snake_case""#, "a snakeXcase word"));
    assert!(holds(r#""snake!_case" ESCAPE "!""#, "a snake_case word"));
    assert!(!holds(r#""snake!_case" ESCAPE "!""#, "a snakeXcase word"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.5.2, §4.2.3.4] FORM OF a word
/// matches a token of the same stem; the word without it does not.
#[test]
fn stemmed_words() {
    assert!(holds(r#"FORM OF "jumping""#, "he jumped high"));
    assert!(holds(r#"STEMMED FORM OF "Standards""#, "an International Standard"));
    assert!(!holds(r#""jumping""#, "he jumped high"));
    assert!(!holds(r#"FORM OF "jumping""#, "he leapt high"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.6.2, §4.2.2, §4.2.4.1] a phrase
/// matches consecutive tokens in order, and an optional word one token or
/// none, not two; a phrase may cross a sentence end.
#[test]
fn phrases_and_optional_words() {
    let doc = "As assumed by this International Standard, every text value.";
    assert!(holds(r#""International Standard""#, doc));
    assert!(!holds(r#""Standard International""#, doc));
    assert!(!holds(r#""this Standard""#, doc));
    assert!(holds(r#""this % Standard""#, doc));
    assert!(holds(r#""this % Standard%""#, doc));
    assert!(holds(r#""International % Standard""#, doc));
    assert!(!holds(r#""assumed % Standard""#, doc));
    assert!(holds(r#""%% by % International""#, doc));
    assert!(holds(r#""value two""#, "a text value. Two more"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.4.2, §6.6.2 Description 3, 4] a stop
/// word of a phrase matches any stop word of the document and nothing
/// else; a stop word by itself matches nothing, so a pattern ORing it
/// rests on its other words.
#[test]
fn stop_words() {
    assert!(holds(r#""sentence or paragraph""#, "of word, sentence and paragraph boundaries"));
    assert!(!holds(r#""sentence or paragraph""#, "of word, sentence big paragraph boundaries"));
    assert!(holds(r#""the" | "fox""#, "the fox"));
    assert!(!holds(r#""the" | "zebra""#, "the fox"));
    assert!(holds(r#"NOT "the" & "fox""#, "the fox"));
    assert!(!holds(r#""th%""#, "the fox"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.7.2, §4.2.4.4] FORM OF a phrase
/// matches consecutive tokens of the same stems.
#[test]
fn stemmed_phrases() {
    assert!(holds(r#"FORM OF "jumping foxes""#, "the jumped fox ran"));
    assert!(!holds(r#"FORM OF "jumping foxes""#, "the fox jumped"));
    assert!(!holds(r#""jumping foxes""#, "the jumped fox ran"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 5a-5c, §4.2.6] OR,
/// AND and NOT, each true and false, and their precedence, as the
/// examples of §4.2.6.4 give it.
#[test]
fn boolean_operators() {
    let second = "The test succeeds since the word International is contained in this text sample.";
    assert!(holds(r#""Standard" | "International" | "language""#, second));
    assert!(!holds(r#""Standard" | "language""#, second));
    assert!(holds(r#""test" & "International""#, second));
    assert!(!holds(r#""test" & "Standard""#, second));
    assert!(holds(r#"NOT "International Standard""#, second));
    assert!(!holds(r#"NOT "International""#, second));
    assert!(holds(r#"NOT "International Standard" & "test""#, second));
    assert!(holds(r#"NOT ("International Standard" & "test")"#, second));
    assert!(!holds(r#"NOT ("International" & "test")"#, second));
    assert!(holds(r#""test" | "nothing" & "nowhere""#, second));
    assert!(!holds(r#"("test" | "nothing") & "nowhere""#, second));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.8.2, §5.1.5] WORDS: the distance is
/// the difference of ordinals, at most the distance given; IN ORDER takes
/// the first list's word first, ANY ORDER either; a word may match both
/// lists; a stop word of a list is ignored.
#[test]
fn proximity_in_words() {
    let doc = "alpha beta gamma delta";
    assert!(holds(r#"("alpha") NEAR ("gamma") WITHIN 2 WORDS IN ORDER"#, doc));
    assert!(!holds(r#"("alpha") NEAR ("gamma") WITHIN 1 WORDS IN ORDER"#, doc));
    assert!(!holds(r#"("gamma") NEAR ("alpha") WITHIN 2 WORDS IN ORDER"#, doc));
    assert!(holds(r#"("gamma") NEAR ("alpha") WITHIN 2 WORDS ANY ORDER"#, doc));
    assert!(!holds(r#"("gamma") NEAR ("alpha") WITHIN 1 WORDS ANY ORDER"#, doc));
    assert!(holds(r#""alpha" NEAR "alp%" WITHIN 0 WORDS IN ORDER"#, doc));
    assert!(holds(r#"("zeta", "beta") NEAR (FORM OF "deltas") WITHIN 2 WORDS IN ORDER"#, doc));
    assert!(!holds(r#"("the", "zeta") NEAR ("alpha") WITHIN 9 WORDS ANY ORDER"#, "the alpha"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.8.2, §5.1.5 Description 4c]
/// CHARACTERS: the distance is the characters between the end of one word
/// and the start of the other, in either order for ANY ORDER.
#[test]
fn proximity_in_characters() {
    let doc = "alpha beta gamma";
    assert!(holds(r#""alpha" NEAR "gamma" WITHIN 6 CHARACTERS IN ORDER"#, doc));
    assert!(!holds(r#""alpha" NEAR "gamma" WITHIN 5 CHARACTERS IN ORDER"#, doc));
    assert!(!holds(r#""gamma" NEAR "alpha" WITHIN 6 CHARACTERS IN ORDER"#, doc));
    assert!(holds(r#""gamma" NEAR "alpha" WITHIN 6 CHARACTERS ANY ORDER"#, doc));
    assert!(!holds(r#""gamma" NEAR "alpha" WITHIN 5 CHARACTERS ANY ORDER"#, doc));
    assert!(holds(r#""alpha" NEAR "beta" WITHIN 1 CHARACTERS IN ORDER"#, doc));
    assert!(!holds(r#""alpha" NEAR "beta" WITHIN 0 CHARACTERS IN ORDER"#, doc));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.8.2, §4.2.5.1] SENTENCES: the
/// distance is the difference of sentence ordinals, 0 the same sentence,
/// so IN ORDER does not order words of one sentence; as the example of
/// §4.2.5.1 gives it.
#[test]
fn proximity_in_sentences() {
    let doc = "Alpha one. Beta two. Gamma three.";
    assert!(holds(r#""alpha" NEAR "gamma" WITHIN 2 SENTENCES IN ORDER"#, doc));
    assert!(!holds(r#""alpha" NEAR "gamma" WITHIN 1 SENTENCES IN ORDER"#, doc));
    assert!(!holds(r#""gamma" NEAR "alpha" WITHIN 2 SENTENCES IN ORDER"#, doc));
    assert!(holds(r#""gamma" NEAR "alpha" WITHIN 2 SENTENCES ANY ORDER"#, doc));
    assert!(holds(r#""three" NEAR "gamma" WITHIN 0 SENTENCE ANY ORDER"#, doc));
    assert!(holds(r#""three" NEAR "gamma" WITHIN 0 SENTENCE IN ORDER"#, doc), "the order inside one unit does not count");
    let first = "As assumed by this International Standard, every text value is associated with a specific language. The recognition.";
    assert!(holds(r#"("Standards", "International") NEAR "language" WITHIN 0 SENTENCES IN ORDER"#, first));
    assert!(!holds(r#"("Standards", "International") NEAR "recognition" WITHIN 0 SENTENCES IN ORDER"#, first));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.8.2] PARAGRAPHS: the distance is the
/// difference of paragraph ordinals, in order and in either.
#[test]
fn proximity_in_paragraphs() {
    let doc = "alpha one.\n\nbeta two.\n\ngamma three.";
    assert!(holds(r#""alpha" NEAR "gamma" WITHIN 2 PARAGRAPHS IN ORDER"#, doc));
    assert!(!holds(r#""alpha" NEAR "gamma" WITHIN 1 PARAGRAPHS IN ORDER"#, doc));
    assert!(!holds(r#""gamma" NEAR "alpha" WITHIN 2 PARAGRAPHS IN ORDER"#, doc));
    assert!(holds(r#""gamma" NEAR "alpha" WITHIN 2 PARAGRAPH ANY ORDER"#, doc));
    assert!(holds(r#""beta" NEAR "two" WITHIN 0 PARAGRAPHS IN ORDER"#, doc));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 5f] a sound or a
/// spelling alike stands as a list on either side of NEAR.
#[test]
fn proximity_of_expansions() {
    assert!(holds(r#"SOUNDS LIKE "Robert" NEAR "smith" WITHIN 1 WORDS IN ORDER"#, "rupert smith"));
    assert!(!holds(r#"SOUNDS LIKE "Robert" NEAR "smith" WITHIN 1 WORDS IN ORDER"#, "rubin smith"));
    assert!(holds(r#""smith" NEAR FUZZY FORM OF "colour" WITHIN 2 WORDS IN ORDER"#, "smith likes color"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.18.2, §4.2.5.2] a context holds
/// when one sentence (paragraph) holds a match of every argument, lists by
/// any member, and a phrase only wholly inside it.
#[test]
fn context() {
    let doc = "The cat sat. The dog ran.\n\nA cow mooed.";
    assert!(holds(r#""cat" IN SAME SENTENCE AS "sat""#, doc));
    assert!(!holds(r#""cat" IN SAME SENTENCE AS "dog""#, doc));
    assert!(holds(r#""cat" IN SAME PARAGRAPH AS "dog""#, doc));
    assert!(!holds(r#""cat" IN SAME PARAGRAPH AS "cow""#, doc));
    assert!(holds(r#""the" | "dog" IN SAME SENTENCE AS "ran" AND FORM OF "dogs""#, doc));
    assert!(!holds(r#""dog" IN SAME SENTENCE AS "ran" AND "sat""#, doc));
    assert!(holds(r#"("cow", "dog") IN SAME SENTENCE AS "ran""#, doc));
    assert!(holds(r#""dog ran" IN SAME SENTENCE AS "dog""#, doc));
    assert!(!holds(r#""dog ran" IN SAME SENTENCE AS "the""#, doc), "a stop word by itself matches nothing");
    assert!(holds(r#""sat the""#, doc));
    assert!(!holds(r#""sat the" IN SAME PARAGRAPH AS "cow""#, doc));
    assert!(!holds(r#""sat the" IN SAME SENTENCE AS "cat""#, doc));
    assert!(holds(r#""sat the" IN SAME PARAGRAPH AS "cat""#, doc));
    assert!(holds(r#"SOUNDS LIKE "cou" IN SAME SENTENCE AS "mooed""#, doc));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.9.2, §6.9.5] SOUNDS LIKE matches a
/// word of the same American Soundex code, or the word itself, and not a
/// word of another code.
#[test]
fn soundex() {
    assert!(holds(r#"SOUNDS LIKE "Robert""#, "Rupert was here"));
    assert!(!holds(r#"SOUNDS LIKE "Robert""#, "Rubin was here"));
    assert!(holds(r#"SOUNDS LIKE "Tymczak""#, "Tymczak"));
    assert!(holds(r#"SOUNDS LIKE "Pfister""#, "Pfyster"));
    assert!(holds(r#"SOUNDS LIKE "Honeyman""#, "honeymoon"));
    assert!(holds(r#"SOUNDS LIKE "1984""#, "in 1984"));
    assert!(!holds(r#"SOUNDS LIKE "1984""#, "in 1985"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.10.2, §6.10.5] FUZZY FORM OF a word
/// of fewer than 8 characters matches within 1 edit, and of 8 or more
/// within 2; one edit more does not.
#[test]
fn fuzzy() {
    assert!(holds(r#"FUZZY FORM OF "color""#, "the colour red"));
    assert!(holds(r#"FUZZY FORM OF "seventy""#, "sevnety"));
    assert!(!holds(r#"FUZZY FORM OF "seventy""#, "sevnetyy"));
    assert!(holds(r#"FUZZY FORM OF "color""#, "the coulor red"));
    assert!(!holds(r#"FUZZY FORM OF "color""#, "the kolar red"));
    assert!(holds(r#"FUZZY FORM OF "standard""#, "standrads"));
    assert!(!holds(r#"FUZZY FORM OF "standard""#, "stnadrads"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.17.2] IS ABOUT, as implemented: a
/// document holding any word of the phrase that is not a stop word.
#[test]
fn about() {
    assert!(holds(r#"IS ABOUT "International Standard on text search""#, "a standard"));
    assert!(!holds(r#"IS ABOUT "International Standard on text search""#, "on the"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.4.2, §6.5.2, §4.4.1] a literal of
/// another language than the document's matches by the same comparison,
/// its stop words its own and the document's the document's: a word that
/// is a stop word of the document's language is matched by nothing, and
/// a stop word of the phrase's language only by one of the document's.
#[test]
fn languages() {
    let none = |pattern: &str, doc: &str| contains(&p(pattern), doc.chars().map(Ok), Language::None).unwrap();
    assert!(!holds(r#"NONE "the" | "zebra""#, "the fox"));
    assert!(none(r#"NONE "the""#, "the fox"));
    assert!(holds(r#""the fox""#, "the fox"));
    assert!(!none(r#""the fox""#, "the fox"));
    assert!(none(r#"FORM OF ENGLISH "jumping""#, "jump"));
    assert!(!none(r#"FORM OF ENGLISH "jumping""#, "jumped"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.1.2 Description 3c ii, §6.4.2] a
/// document of no characters matches no pattern, NOT ones too; one of
/// characters but no word matches NOT ones.
#[test]
fn empty_documents() {
    assert!(!holds(r#""fox""#, ""));
    assert!(!holds(r#"NOT "fox""#, ""));
    assert!(holds(r#"NOT "fox""#, " ,. "));
    assert!(!holds(r#""fox""#, " ,. "));
}

/// Verifies: [DESIGN] a thesaurus expansion not yet expanded is refused
/// with SQLCODE -910, as a primary, a list of NEAR and a context argument;
/// an error reading the document is returned.
#[test]
fn errors() {
    for text in [
        r#"THESAURUS "t" EXPAND SYNONYM TERM OF "x""#,
        r#"THESAURUS "t" EXPAND SYNONYM TERM OF "x" NEAR "b" WITHIN 1 WORDS IN ORDER"#,
        r#""b" IN SAME SENTENCE AS THESAURUS "t" EXPAND SYNONYM TERM OF "x""#,
    ] {
        let e = contains(&p(text), "x b".chars().map(Ok), Language::English).unwrap_err();
        assert_eq!(e.code, Code::Api, "{text}");
    }
    let failing = "fox".chars().map(Ok).chain(std::iter::once(Err(SqlError::new(Code::Io, "[DESIGN] gone"))));
    assert_eq!(contains(&p(r#""fox""#), failing, Language::English).unwrap_err().code, Code::Io);
}

/// Verifies: [DESIGN] a long document is read streaming and a word found
/// near its end; a common word under a proximity is matched at every
/// place it occurs.
#[test]
fn long_documents() {
    let long = format!("{} needle in hay", "hay ".repeat(100_000));
    assert!(holds(r#""needle in hay""#, &long));
    assert!(holds(r#""hay" NEAR "needle" WITHIN 1 WORDS IN ORDER"#, &long));
    assert!(!holds(r#""needle" NEAR "hay" WITHIN 1 WORDS IN ORDER & "pin""#, &long));
}
