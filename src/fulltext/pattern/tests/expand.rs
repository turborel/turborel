//! Thesaurus expansion through a test double of `Terms`.

use std::cell::RefCell;

use super::*;
use crate::error::{Code, Result, SqlError};

/// A thesaurus in memory: for a thesaurus, a relation and a term, its
/// terms. It records the levels it is asked for.
#[derive(Default)]
struct Double {
    /// (thesaurus, relation, term, terms).
    entries: Vec<(&'static str, Relation, &'static str, Vec<&'static str>)>,
    /// The levels of each call.
    levels: RefCell<Vec<Option<u64>>>,
}

impl Terms for Double {
    fn terms(&self, thesaurus: &str, relation: Relation, term: &str, levels: Option<u64>) -> Result<Vec<String>> {
        self.levels.borrow_mut().push(levels);
        let found = self.entries.iter().find(|(t, r, s, _)| *t == thesaurus && *r == relation && *s == term);
        Ok(found.map(|(_, _, _, terms)| terms.iter().map(|t| (*t).to_owned()).collect()).unwrap_or_default())
    }
}

/// A `Terms` that fails, as a table read may.
struct Failing;

impl Terms for Failing {
    fn terms(&self, _: &str, _: Relation, _: &str, _: Option<u64>) -> Result<Vec<String>> {
        Err(SqlError::new(Code::Io, "[DESIGN] the thesaurus could not be read"))
    }
}

/// The thesaurus "cs" of the examples of §4.2.3.2 and §4.2.4.2.
fn cs() -> Double {
    Double {
        entries: vec![
            ("cs", Relation::Synonym, "list", vec!["sequence", "List", "sequence"]),
            ("cs", Relation::Synonym, "rule of thumb", vec!["heuristics", "Rule-of-Thumb"]),
            ("cs", Relation::Broader, "mouse", vec!["rodent", "mammal"]),
            ("cs", Relation::Preferred, "pc", vec!["personal computer"]),
            ("cs", Relation::Top, "mouse", vec!["animal"]),
        ],
        levels: RefCell::default(),
    }
}

/// `pattern` read and expanded by `terms`.
fn expanded(pattern: &str, terms: &dyn Terms) -> Pattern {
    p(pattern).expanded(terms).unwrap()
}

/// Whether `document` matches `pattern` expanded by the thesaurus "cs".
fn holds_expanded(pattern: &str, document: &str) -> bool {
    contains(&expanded(pattern, &cs()), document.chars().map(Ok), Language::English).unwrap()
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.13.5 DR5, §5.3.1 Description 5m,
/// §4.2.3.2] a synonym expansion is the OR of the starting term and its
/// synonyms, each once, the term not repeated though the thesaurus lists
/// it; it matches a document holding any of them.
#[test]
fn synonyms_or_the_term() {
    let pattern = expanded(r#"THESAURUS "cs" EXPAND SYNONYM TERM OF "list""#, &cs());
    assert_eq!(pattern, Pattern::Or(Box::new(Pattern::Literal(lit("list"))), Box::new(Pattern::Literal(lit("sequence")))));
    assert!(holds_expanded(r#"THESAURUS "cs" EXPAND SYNONYM TERM OF "list""#, "a sequence of words"));
    assert!(holds_expanded(r#"THESAURUS "cs" EXPAND SYNONYM TERM OF "list""#, "a list of words"));
    assert!(!holds_expanded(r#"THESAURUS "cs" EXPAND SYNONYM TERM OF "list""#, "a set of words"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.4.2, §6.13.5] a phrase expands as
/// its words written, and its terms are phrases, normalized and cut as a
/// document is.
#[test]
fn phrases_expand_to_phrases() {
    let pattern = expanded(r#"THESAURUS "cs" EXPAND SYNONYM TERM OF "Rule of Thumb""#, &cs());
    assert_eq!(pattern, Pattern::Or(Box::new(Pattern::Literal(lit("rule of thumb"))), Box::new(Pattern::Literal(lit("heuristics")))));
    assert!(holds_expanded(r#"THESAURUS "cs" EXPAND SYNONYM TERM OF "rule of thumb""#, "conventions, and heuristics"));
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.11.5 DR6, §6.12.5 DR6, §6.15.5 DR5]
/// BROADER, NARROWER and RELATED give the term with its terms, the term
/// alone for a term or a thesaurus the tables do not hold; FOR n LEVELS
/// passes the count, and its absence none.
#[test]
fn hierarchies_and_levels() {
    let double = cs();
    let pattern = expanded(r#"THESAURUS "cs" EXPAND BROADER TERM OF "mouse" FOR 2 LEVELS"#, &double);
    let words: Vec<Literal> = pattern.words().into_iter().cloned().collect();
    assert_eq!(words, vec![lit("mouse"), lit("rodent"), lit("mammal")]);
    let tree = Pattern::Or(
        Box::new(Pattern::Or(Box::new(Pattern::Literal(lit("mouse"))), Box::new(Pattern::Literal(lit("rodent"))))),
        Box::new(Pattern::Literal(lit("mammal"))),
    );
    assert_eq!(pattern, tree, "a balanced OR");
    for relation in ["NARROWER", "RELATED"] {
        let text = format!(r#"THESAURUS "cs" EXPAND {relation} TERM OF "mouse""#);
        assert_eq!(expanded(&text, &double), Pattern::Literal(lit("mouse")), "{relation}");
    }
    assert_eq!(expanded(r#"THESAURUS "other" EXPAND BROADER TERM OF "mouse""#, &double), Pattern::Literal(lit("mouse")));
    assert_eq!(*double.levels.borrow(), vec![Some(2), None, None, None]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.14.5 DR4, DR5, §6.16.5 DR3, DR4]
/// PREFERRED and TOP give their terms without the starting term, and the
/// starting term only when there are none.
#[test]
fn preferred_and_top_terms() {
    let personal = Pattern::Literal(lit("personal computer"));
    assert_eq!(expanded(r#"THESAURUS "cs" EXPAND PREFERRED TERM OF "PC""#, &cs()), personal);
    assert_eq!(expanded(r#"THESAURUS "cs" EXPAND PREFERRED TERM OF "laptop""#, &cs()), Pattern::Literal(lit("laptop")));
    assert_eq!(expanded(r#"THESAURUS "cs" EXPAND TOP TERM OF "mouse""#, &cs()), Pattern::Literal(lit("animal")));
    assert_eq!(expanded(r#"THESAURUS "cs" EXPAND TOP TERM OF "animal""#, &cs()), Pattern::Literal(lit("animal")));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 5f, 5g] in a
/// proximity or a context, an expansion becomes the list of its terms;
/// under NOT and other operators, its OR; other parts are kept.
#[test]
fn lists_in_proximity_and_context() {
    let near = expanded(r#""x" NEAR THESAURUS "cs" EXPAND SYNONYM TERM OF "list" WITHIN 1 WORDS IN ORDER"#, &cs());
    let Pattern::Near(near) = near else { panic!() };
    assert_eq!(near.right, Argument::List(vec![lit("list"), lit("sequence")]));
    assert_eq!(near.left, Argument::Literal(lit("x")));
    let context = expanded(r#"THESAURUS "cs" EXPAND TOP TERM OF "mouse" IN SAME SENTENCE AS SOUNDS LIKE "b""#, &cs());
    let Pattern::Context(context) = context else { panic!() };
    assert_eq!(context.arguments, vec![Argument::List(vec![lit("animal")]), Argument::Expansion(Expansion::Soundex(lit("b")))]);
    let not = expanded(r#"NOT THESAURUS "cs" EXPAND TOP TERM OF "mouse" & IS ABOUT "c""#, &cs());
    let expected = Pattern::And(Box::new(Pattern::Not(Box::new(Pattern::Literal(lit("animal"))))), Box::new(Pattern::About(lit("c"))));
    assert_eq!(not, expected);
    assert!(holds_expanded(r#""x" NEAR THESAURUS "cs" EXPAND SYNONYM TERM OF "list" WITHIN 1 WORDS IN ORDER"#, "x sequence"));
    assert!(!holds_expanded(r#""x" NEAR THESAURUS "cs" EXPAND SYNONYM TERM OF "list" WITHIN 1 WORDS IN ORDER"#, "x set"));
}

/// Verifies: [DESIGN] a failure reading the thesaurus is returned; many
/// terms make a balanced OR that evaluates.
#[test]
fn failures_and_many_terms() {
    let e = p(r#""a" | THESAURUS "cs" EXPAND SYNONYM TERM OF "list""#).expanded(&Failing).unwrap_err();
    assert_eq!(e.code, Code::Io);
    let names: Vec<String> = (0..1_000_u32).map(|i| format!("term{i}")).collect();
    let leaked: Vec<&'static str> = names.into_iter().map(|n| &*Box::leak(n.into_boxed_str())).collect();
    let many = Double { entries: vec![("t", Relation::Synonym, "x", leaked)], levels: RefCell::default() };
    let pattern = expanded(r#"THESAURUS "t" EXPAND SYNONYM TERM OF "x""#, &many);
    assert_eq!(pattern.words().len(), 1_001);
    assert!(contains(&pattern, "see term999".chars().map(Ok), Language::English).unwrap());
}

/// Verifies: [ISO/IEC 13249-2:2002 §6.13.5] [EXTENSION] RULES.md D329: a
/// term of a language read as bigrams is asked of the thesaurus as
/// written, its pairs joined back, and its terms match as pairs again.
#[test]
fn bigram_terms_are_joined_back() {
    let double = Double { entries: vec![("CITY", Relation::Synonym, "東京都", vec!["首都"])], levels: RefCell::default() };
    let pattern = parse("THESAURUS \"CITY\" EXPAND SYNONYM TERM OF \"東京都\"", Language::Japanese).unwrap().expanded(&double).unwrap();
    for document in ["東京都に住む", "首都で働く"] {
        assert!(contains(&pattern, document.chars().map(Ok), Language::Japanese).unwrap(), "{document}");
    }
    assert!(!contains(&pattern, "京都に行く".chars().map(Ok), Language::Japanese).unwrap());
    assert_eq!(double.levels.borrow().len(), 1);
}
