//! Reading every production of the `<search expression>` grammar.

use super::*;

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3a, 3c] a `<word>`
/// and a `<phrase>` are read normalized, white space separating a
/// phrase's words; a `<language specification>` names a language in any
/// case, the default standing for none.
#[test]
fn words_phrases_and_languages() {
    assert_eq!(p(r#""Fox""#), Pattern::Literal(lit("fox")));
    assert_eq!(p(r#" "International   Standard" "#), Pattern::Literal(lit("international standard")));
    assert_eq!(p(r#""Résumé""#), Pattern::Literal(lit("resume")));
    let none = Literal { language: Language::None, ..lit("fox") };
    assert_eq!(p(r#"none "fox""#), Pattern::Literal(none.clone()));
    assert_eq!(p(r#"NONE"fox""#), Pattern::Literal(none.clone()));
    assert_eq!(parse(r#""fox""#, Language::None).unwrap(), Pattern::Literal(none));
    assert_eq!(p(r#"english "fox""#), Pattern::Literal(lit("fox")));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3a, §4.5] a
/// `<doublequote symbol>` stands for one double quote; a word the
/// tokenizer reads as several is a phrase of them, and one with
/// punctuation around it is the word alone.
#[test]
fn doublequote_symbols_and_cut_words() {
    let Pattern::Literal(literal) = p(r#""x""%""#) else { panic!() };
    assert_eq!(literal.parts, vec![Part::Word(Mask(vec![MaskChar::Literal('x'), MaskChar::Literal('"'), MaskChar::Any]))]);
    assert_eq!(p(r#""e-mail""#), Pattern::Literal(lit("e mail")));
    assert_eq!(p(r#""Standard.""#), Pattern::Literal(lit("standard")));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3, §4.2.6.4] `|`
/// binds less tightly than `&`, and `&` than NOT; parentheses group;
/// both operators join from the left; key words are read in any case
/// [§5.3.2].
#[test]
fn boolean_operators_and_precedence() {
    let (a, b, c) = (Pattern::Literal(lit("a")), Pattern::Literal(lit("b")), Pattern::Literal(lit("c")));
    let not_c = Pattern::Not(Box::new(c.clone()));
    let and = Pattern::And(Box::new(b.clone()), Box::new(not_c));
    assert_eq!(p(r#""a" | "b" & NOT "c""#), Pattern::Or(Box::new(a.clone()), Box::new(and)));
    let or = Pattern::Or(Box::new(a.clone()), Box::new(b.clone()));
    assert_eq!(p(r#"("a" | "b") & "c""#), Pattern::And(Box::new(or.clone()), Box::new(c.clone())));
    assert_eq!(p(r#""a" | "b" | "c""#), Pattern::Or(Box::new(or), Box::new(c.clone())));
    assert_eq!(p(r#"not ((("a"))) | "b""#), Pattern::Or(Box::new(Pattern::Not(Box::new(a))), Box::new(b)));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3, 5d, 5e] `[
/// STEMMED ] FORM OF` a word or a phrase, the language after it.
#[test]
fn stemmed_words_and_phrases() {
    assert_eq!(p(r#"STEMMED FORM OF "mice""#), Pattern::Literal(stemmed("mice")));
    assert_eq!(p(r#"form of "data bases""#), Pattern::Literal(stemmed("data bases")));
    let none = Literal { language: Language::None, ..stemmed("mice") };
    assert_eq!(p(r#"FORM OF NONE "mice""#), Pattern::Literal(none));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3a, NOTE 11, NOTE
/// 13] `%` and `_` are wild cards; the escape character makes them, or
/// itself, characters; `%` alone in a phrase, once or more, is an optional
/// word, and escaped is not.
#[test]
fn masks_escapes_and_optional_words() {
    assert_eq!(p(r#""Stand%""#), Pattern::Literal(lit("stand%")));
    assert_eq!(p(r#""a_c""#), Pattern::Literal(lit("a_c")));
    let Pattern::Literal(escaped) = p(r#""snake!_case!!" ESCAPE "!""#) else { panic!() };
    assert_eq!(escaped.parts, vec![Part::Word(Mask("snake_case".chars().map(MaskChar::Literal).collect()))]);
    let Pattern::Literal(literal) = p(r#""100!%%" escape "!""#) else { panic!() };
    assert_eq!(
        literal.parts,
        vec![Part::Word(Mask(vec![
            MaskChar::Literal('1'),
            MaskChar::Literal('0'),
            MaskChar::Literal('0'),
            MaskChar::Literal('%'),
            MaskChar::Any
        ]))]
    );
    assert_eq!(p(r#""this % Standard%""#), Pattern::Literal(lit("this % standard%")));
    assert_eq!(p(r#""this %% x""#), Pattern::Literal(lit("this % x")));
    let Pattern::Literal(this) = p(r#""this !% x" ESCAPE "!""#) else { panic!() };
    assert_eq!(this.parts.get(1), Some(&Part::Word(Mask(vec![MaskChar::Literal('%')]))));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3, 3f, §4.2.5.1]
/// `<Proximity expansion>`: token lists of words and stemmed words, a
/// list of one written bare, every unit singular or plural, IN ORDER and
/// ANY ORDER.
#[test]
fn proximity() {
    let near = Proximity {
        left: Argument::List(vec![lit("a"), stemmed("b")]),
        right: Argument::List(vec![lit("c")]),
        distance: 3,
        unit: Unit::Words,
        in_order: true,
    };
    assert_eq!(p(r#"("a", FORM OF "b") NEAR ("c") WITHIN 3 WORDS IN ORDER"#), Pattern::Near(near));
    let bare = Proximity {
        left: Argument::Literal(lit("a")),
        right: Argument::Literal(lit("b")),
        distance: 0,
        unit: Unit::Sentences,
        in_order: false,
    };
    assert_eq!(p(r#""a" near "b" within 0 sentences any order"#), Pattern::Near(bare.clone()));
    for (unit, expected) in
        [("CHARACTERS", Unit::Characters), ("SENTENCE", Unit::Sentences), ("PARAGRAPH", Unit::Paragraphs), ("PARAGRAPHS", Unit::Paragraphs)]
    {
        let text = format!(r#""a" NEAR "b" WITHIN 0 {unit} ANY ORDER"#);
        assert_eq!(p(&text), Pattern::Near(Proximity { unit: expected, ..bare.clone() }), "{unit}");
    }
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 5f] either side of
/// NEAR may be an expansion, and a proximity may stand under NOT and
/// among other factors.
#[test]
fn proximity_of_expansions() {
    let pattern = p(r#"NOT SOUNDS LIKE "a" NEAR THESAURUS "t" EXPAND SYNONYM TERM OF "b" WITHIN 1 WORDS IN ORDER & "c""#);
    let Pattern::And(near, _) = pattern else { panic!() };
    let Pattern::Not(near) = *near else { panic!() };
    let Pattern::Near(near) = *near else { panic!() };
    assert_eq!(near.left, Argument::Expansion(Expansion::Soundex(lit("a"))));
    let Argument::Expansion(Expansion::Thesaurus(t)) = near.right else { panic!() };
    assert_eq!((t.name.as_str(), t.relation, t.of, t.levels), ("t", Relation::Synonym, lit("b"), None));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3] `<context
/// condition>`: literals, lists of words and phrases, and expansions,
/// joined by AND, in the same SENTENCE or PARAGRAPH.
#[test]
fn context() {
    let pattern = p(r#"("a", "b c") IN SAME SENTENCE AS "d" AND SOUNDS LIKE "e" and ("f")"#);
    let expected = Context {
        unit: Unit::Sentences,
        arguments: vec![
            Argument::List(vec![lit("a"), lit("b c")]),
            Argument::Literal(lit("d")),
            Argument::Expansion(Expansion::Soundex(lit("e"))),
            Argument::List(vec![lit("f")]),
        ],
    };
    assert_eq!(pattern, Pattern::Context(expected));
    let Pattern::Context(c) = p(r#"FUZZY FORM OF "a" in same paragraphs as "b""#) else { panic!() };
    assert_eq!((c.unit, c.arguments.len()), (Unit::Paragraphs, 2));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3] IS ABOUT a word
/// or a phrase, SOUNDS LIKE and FUZZY FORM OF a word.
#[test]
fn about_soundex_and_fuzzy() {
    assert_eq!(p(r#"IS ABOUT "text search""#), Pattern::About(lit("text search")));
    assert_eq!(p(r#"sounds like "Robert""#), Pattern::Expansion(Expansion::Soundex(lit("robert"))));
    assert_eq!(p(r#"FUZZY FORM OF "colour""#), Pattern::Expansion(Expansion::Fuzzy(lit("colour"))));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1 Description 3, NOTE 14] every
/// thesaurus relation; FOR n LEVEL or LEVELS of BROADER and NARROWER; the
/// name as written; a phrase as the term.
#[test]
fn thesaurus_expansions() {
    for (key, relation) in [
        ("BROADER", Relation::Broader),
        ("narrower", Relation::Narrower),
        ("SYNONYM", Relation::Synonym),
        ("PREFERRED", Relation::Preferred),
        ("RELATED", Relation::Related),
        ("TOP", Relation::Top),
    ] {
        let text = format!(r#"THESAURUS "Computer Science" EXPAND {key} TERM OF "rule of thumb""#);
        let expected = Thesaurus { name: "Computer Science".into(), relation, of: lit("rule of thumb"), levels: None };
        assert_eq!(p(&text), Pattern::Expansion(Expansion::Thesaurus(expected)), "{key}");
    }
    let Pattern::Expansion(Expansion::Thesaurus(t)) = p(r#"THESAURUS "t" EXPAND BROADER TERM OF "x" FOR 2 LEVELS"#) else { panic!() };
    assert_eq!(t.levels, Some(2));
    let Pattern::Expansion(Expansion::Thesaurus(one)) = p(r#"THESAURUS "t" EXPAND NARROWER TERM OF "x" for 1 level"#) else { panic!() };
    assert_eq!(one.levels, Some(1));
}

/// Verifies: [ISO/IEC 13249-2:2002 §5.3.1] `words` lists every literal in
/// the order written: words, list members, the words of expansions and of
/// thesaurus terms, under every operator.
#[test]
fn words_in_order() {
    let pattern = p(concat!(
        r#""a" | NOT ("b", "c") NEAR SOUNDS LIKE "d" WITHIN 1 WORDS IN ORDER & THESAURUS "t" EXPAND TOP TERM OF "e" "#,
        r#"& IS ABOUT "f" | "g" IN SAME SENTENCE AS ("h", "i") AND FUZZY FORM OF "j""#,
    ));
    let words: Vec<Literal> = pattern.words().into_iter().cloned().collect();
    let expected: Vec<Literal> = "a b c d e f g h i j".split(' ').map(lit).collect();
    assert_eq!(words, expected);
}
