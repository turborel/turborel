//! Unit tests of the English stemmer, Snowball's English algorithm, against
//! the stems its published definition and vocabulary give.

use crate::fulltext::text::{Language, stem};

/// Checks each word of `pairs` stems to its stem.
fn check(pairs: &[(&str, &str)]) {
    for (word, expected) in pairs {
        assert_eq!(stem(Language::English, word).unwrap(), *expected, "{word}");
    }
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] the exceptional forms stem
/// as the algorithm lists them, among them the invariant ones.
#[test]
fn exceptional_forms() {
    check(&[
        ("skis", "ski"),
        ("skies", "sky"),
        ("dying", "die"),
        ("lying", "lie"),
        ("tying", "tie"),
        ("idly", "idl"),
        ("gently", "gentl"),
        ("ugly", "ugli"),
        ("early", "earli"),
        ("only", "onli"),
        ("singly", "singl"),
        ("sky", "sky"),
        ("news", "news"),
        ("howe", "howe"),
        ("atlas", "atlas"),
        ("cosmos", "cosmos"),
        ("bias", "bias"),
        ("andes", "andes"),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] the words left as they are
/// after step 1a stop there, plural or possessive.
#[test]
fn invariant_after_step_1a() {
    check(&[
        ("inning", "inning"),
        ("innings", "inning"),
        ("outing", "outing"),
        ("outings", "outing"),
        ("canning", "canning"),
        ("cannings", "canning"),
        ("herring", "herring"),
        ("herring's", "herring"),
        ("earring", "earring"),
        ("earrings", "earring"),
        ("proceed", "proceed"),
        ("proceeds", "proceed"),
        ("exceed", "exceed"),
        ("succeed", "succeed"),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] a word of two letters or
/// fewer is its own stem.
#[test]
fn short_words_stay() {
    check(&[("a", "a"), ("is", "is"), ("by", "by"), ("", "")]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] an initial apostrophe goes,
/// and step 0 takes possessives off.
#[test]
fn apostrophes_and_possessives() {
    check(&[("dog's", "dog"), ("dogs'", "dog"), ("'tis", "tis"), ("o'clock", "o'clock")]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] `y` after a vowel or first
/// is a consonant, and step 1c makes a final `y` after a non-vowel `i`.
#[test]
fn consonant_y_and_step_1c() {
    check(&[
        ("youth", "youth"),
        ("yelling", "yell"),
        ("sayings", "say"),
        ("enjoying", "enjoy"),
        ("say", "say"),
        ("toy", "toy"),
        ("aye", "aye"),
        ("cry", "cri"),
        ("crying", "cri"),
        ("happy", "happi"),
        ("ally", "alli"),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] `R1` begins after `gener`,
/// `commun` and `arsen` whatever follows.
#[test]
fn r1_prefixes() {
    check(&[
        ("generate", "generat"),
        ("generations", "generat"),
        ("generously", "generous"),
        ("general", "general"),
        ("generically", "generic"),
        ("communism", "communism"),
        ("commune", "commune"),
        ("arsenal", "arsenal"),
        ("arsenals", "arsenal"),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] step 1a's plurals.
#[test]
fn step_1a_plurals() {
    check(&[
        ("caresses", "caress"),
        ("ponies", "poni"),
        ("ties", "tie"),
        ("cries", "cri"),
        ("cried", "cri"),
        ("cats", "cat"),
        ("gas", "gas"),
        ("gaps", "gap"),
        ("kiwis", "kiwi"),
        ("bliss", "bliss"),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] step 1b's `eed`, `ed` and
/// `ing`, and the `e` added or the double undone after them.
#[test]
fn step_1b_endings() {
    check(&[
        ("feed", "feed"),
        ("agreed", "agre"),
        ("needed", "need"),
        ("exceeding", "exceed"),
        ("plastered", "plaster"),
        ("motoring", "motor"),
        ("sing", "sing"),
        ("conflated", "conflat"),
        ("troubled", "troubl"),
        ("sized", "size"),
        ("hopping", "hop"),
        ("embedded", "embed"),
        ("falling", "fall"),
        ("filing", "file"),
        ("feeling", "feel"),
        ("luxuriating", "luxuri"),
        ("lyingly", "ly"),
        ("skying", "ski"),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] step 2's suffixes in `R1`,
/// with `ogi` after `l` and `li` after a valid ending only.
#[test]
fn step_2_suffixes() {
    check(&[
        ("relational", "relat"),
        ("conditional", "condit"),
        ("rational", "ration"),
        ("valency", "valenc"),
        ("digitizer", "digit"),
        ("operator", "oper"),
        ("angularity", "angular"),
        ("hopefulness", "hope"),
        ("vicariously", "vicari"),
        ("callousness", "callous"),
        ("decisiveness", "decis"),
        ("sensitivity", "sensit"),
        ("sensibility", "sensibl"),
        ("possibly", "possibl"),
        ("geology", "geolog"),
        ("biology", "biolog"),
        ("fully", "fulli"),
        ("hopelessly", "hopeless"),
        ("ideally", "ideal"),
        ("fluently", "fluentli"),
        ("cannily", "cannili"),
        ("speedily", "speedili"),
        ("knightly", "knight"),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] step 3's suffixes in `R1`,
/// with `ative` in `R2` only.
#[test]
fn step_3_suffixes() {
    check(&[
        ("triplicate", "triplic"),
        ("formative", "format"),
        ("hopeful", "hope"),
        ("goodness", "good"),
        ("electrical", "electr"),
        ("electricity", "electr"),
        ("logically", "logic"),
        ("usefulness", "use"),
        ("emptiness", "empti"),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] step 4's suffixes in `R2`,
/// with `ion` after `s` or `t` only.
#[test]
fn step_4_suffixes() {
    check(&[
        ("revival", "reviv"),
        ("allowance", "allow"),
        ("inference", "infer"),
        ("airliner", "airlin"),
        ("adjustable", "adjust"),
        ("defensible", "defens"),
        ("irritant", "irrit"),
        ("replacement", "replac"),
        ("adjustment", "adjust"),
        ("dependent", "depend"),
        ("adoption", "adopt"),
        ("tradition", "tradit"),
        ("vision", "vision"),
        ("motion", "motion"),
        ("onion", "onion"),
        ("activate", "activ"),
        ("homologous", "homolog"),
        ("effective", "effect"),
        ("bowdlerize", "bowdler"),
        ("criticize", "critic"),
        ("nationalism", "nation"),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] step 5's final `e` and `ll`.
#[test]
fn step_5_endings() {
    check(&[
        ("probate", "probat"),
        ("rate", "rate"),
        ("cease", "ceas"),
        ("probe", "probe"),
        ("hope", "hope"),
        ("controll", "control"),
        ("roll", "roll"),
        ("spelling", "spell"),
        ("fulfilling", "fulfil"),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] a run of the algorithm's
/// sample vocabulary stems as published.
#[test]
fn sample_vocabulary() {
    check(&[
        ("consign", "consign"),
        ("consigned", "consign"),
        ("consigning", "consign"),
        ("consignment", "consign"),
        ("consistency", "consist"),
        ("consistently", "consist"),
        ("consolation", "consol"),
        ("consolatory", "consolatori"),
        ("consolidating", "consolid"),
        ("consolingly", "consol"),
        ("conspicuously", "conspicu"),
        ("conspiracy", "conspiraci"),
        ("conspirators", "conspir"),
        ("constables", "constabl"),
        ("constancy", "constanc"),
        ("knackeries", "knackeri"),
        ("knavish", "knavish"),
        ("kneeling", "kneel"),
        ("knees", "knee"),
        ("knives", "knive"),
        ("knitting", "knit"),
        ("knockers", "knocker"),
        ("knopp", "knopp"),
    ]);
}

/// Verifies: [ISO/IEC 13249-2:2002 §4.2.3.4] regions are places fixed
/// before the steps, so a suffix step 2 shortens leaves `R2` where it was
/// and a word and its derived forms meet.
#[test]
fn regions_stay_fixed() {
    check(&[("realize", "realiz"), ("realization", "realiz"), ("ionization", "ioniz"), ("sensationalism", "sensat")]);
}
