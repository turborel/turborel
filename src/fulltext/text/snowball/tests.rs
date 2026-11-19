//! Unit tests of the Snowball interpreter.

use super::*;

const LANGS: [&str; 35] = ["arabic", "armenian", "basque", "catalan", "czech", "danish", "dutch", "english", "esperanto", "estonian", "finnish", "french", "german", "greek", "hindi", "hungarian", "indonesian", "irish", "italian", "lithuanian", "nepali", "norwegian", "persian", "polish", "portuguese", "romanian", "russian", "serbian", "sesotho", "spanish", "swedish", "tamil", "turkish", "ukrainian", "yiddish"];

/// Verifies: [DESIGN] dev full vocabulary.
#[test]
fn dev_full_vocabulary() {
    let base = "/media/agpmastersystem/0e938ae2-7b41-437b-a5ac-a27e9f111a4e/turborel-work/papers/snowball";
    let only = std::env::var("SB_LANG").ok();
    let mut bad = 0;
    for lang in LANGS {
        if only.as_deref().is_some_and(|o| o != lang) { continue; }
        let src = std::fs::read_to_string(format!("{base}/snowball-main/algorithms/{lang}.sbl")).unwrap();
        let stemmer = match Stemmer::compile(&src) { Ok(s) => s, Err(e) => { println!("{lang}: COMPILE {e}"); bad += 1; continue; } };
        let voc = std::fs::read_to_string(format!("{base}/snowball-data-main/{lang}/voc.txt")).unwrap_or_default();
        let out = std::fs::read_to_string(format!("{base}/snowball-data-main/{lang}/output.txt")).unwrap_or_default();
        let (mut n, mut wrong) = (0, 0);
        for (w, s) in voc.lines().zip(out.lines()) {
            n += 1;
            let got = stemmer.stem(w);
            if got != s { wrong += 1; if wrong <= 5 { println!("{lang}: {w} -> {got} want {s}"); } }
        }
        println!("{lang}: {n} words, {wrong} wrong");
        if wrong > 0 || n == 0 { bad += 1; }
    }
    assert_eq!(bad, 0);
}
