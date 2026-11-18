//! The English stemmer [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §4.2.3.4]:
//! the base reduced form of an English word is its Porter2 stem, the
//! Snowball English algorithm of Martin Porter as published with its
//! exceptional forms (snowballstem.org, `algorithms/english/stemmer.html`,
//! the 2006 definition).
//!
//! The word is held as characters; `R1` and `R2` are the places their
//! regions begin. A word of characters outside a to z is stemmed all the
//! same, every such character taken for a non-vowel.

/// The vowels.
const VOWELS: [char; 6] = ['a', 'e', 'i', 'o', 'u', 'y'];
/// The doubles.
const DOUBLES: [&str; 9] = ["bb", "dd", "ff", "gg", "mm", "nn", "pp", "rr", "tt"];
/// The letters a `li` ending may follow.
const LI_ENDINGS: [char; 10] = ['c', 'd', 'e', 'g', 'h', 'k', 'm', 'n', 'r', 't'];
/// The prefixes after which `R1` begins, whatever the letters.
const R1_PREFIXES: [&str; 3] = ["gener", "commun", "arsen"];
/// The non-vowels a short syllable may not end in.
const NOT_SHORT: [char; 3] = ['w', 'x', MARKED_Y];
/// A `y` taken for a consonant.
const MARKED_Y: char = 'Y';
/// The letter `y`.
const Y: char = 'y';
/// The letter `i`, which a final `y` becomes.
const I: char = 'i';
/// The letter `e`.
const E: char = 'e';
/// The letter `l`.
const L: char = 'l';
/// The apostrophe.
const APOSTROPHE: char = '\'';
/// The longest word left as it is.
const SHORTEST: usize = 2;
/// The words stemmed as listed, the algorithm's exception1.
const SPECIAL: [(&str, &str); 18] = [
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
];
/// The words left as they are after step 1a, the algorithm's exception2.
const AFTER_1A: [&str; 8] = ["inning", "outing", "canning", "herring", "earring", "proceed", "exceed", "succeed"];

/// Step 0's suffixes, each removed.
const STEP_0: [&str; 3] = ["'s'", "'s", "'"];

/// What step 1a does to a suffix.
#[derive(Debug, Clone, Copy)]
enum Step1a {
    /// Replace it by `ss`.
    Sses,
    /// Replace it by `i` after two letters or more, else by `ie`.
    Ies,
    /// Leave it.
    Keep,
    /// Remove it if a vowel comes before the letter before it.
    S,
}

/// Step 1a's suffixes.
const STEP_1A: [(&str, Step1a); 6] =
    [("sses", Step1a::Sses), ("ied", Step1a::Ies), ("ies", Step1a::Ies), ("us", Step1a::Keep), ("ss", Step1a::Keep), ("s", Step1a::S)];
/// What `sses` becomes.
const SS: &str = "ss";
/// What `ies` becomes after two letters or more.
const IES_LONG: &str = "i";
/// What `ies` becomes after one letter.
const IES_SHORT: &str = "ie";

/// Step 1b's suffixes, and whether each is `eed` or `eedly`.
const STEP_1B: [(&str, bool); 6] = [("eed", true), ("eedly", true), ("ed", false), ("edly", false), ("ing", false), ("ingly", false)];
/// What `eed` and `eedly` become.
const EE: &str = "ee";
/// The endings after which step 1b adds an `e`.
const ADD_E: [&str; 3] = ["at", "bl", "iz"];

/// What a suffix of steps 2 to 4 also needs.
#[derive(Debug, Clone, Copy)]
enum Need {
    /// Nothing.
    Nothing,
    /// An `l` before it.
    AfterL,
    /// A valid `li` ending before it.
    LiEnding,
    /// To be in `R2`.
    InR2,
    /// An `s` or `t` before it.
    AfterSOrT,
}

/// A suffix of steps 2 to 4, what replaces it and what it needs.
type Rule = (&'static str, &'static str, Need);

/// Step 2's suffixes, in `R1`.
const STEP_2: [Rule; 24] = [
    ("tional", "tion", Need::Nothing),
    ("enci", "ence", Need::Nothing),
    ("anci", "ance", Need::Nothing),
    ("abli", "able", Need::Nothing),
    ("entli", "ent", Need::Nothing),
    ("izer", "ize", Need::Nothing),
    ("ization", "ize", Need::Nothing),
    ("ational", "ate", Need::Nothing),
    ("ation", "ate", Need::Nothing),
    ("ator", "ate", Need::Nothing),
    ("alism", "al", Need::Nothing),
    ("aliti", "al", Need::Nothing),
    ("alli", "al", Need::Nothing),
    ("fulness", "ful", Need::Nothing),
    ("ousli", "ous", Need::Nothing),
    ("ousness", "ous", Need::Nothing),
    ("iveness", "ive", Need::Nothing),
    ("iviti", "ive", Need::Nothing),
    ("biliti", "ble", Need::Nothing),
    ("bli", "ble", Need::Nothing),
    ("ogi", "og", Need::AfterL),
    ("fulli", "ful", Need::Nothing),
    ("lessli", "less", Need::Nothing),
    ("li", "", Need::LiEnding),
];

/// Step 3's suffixes, in `R1`.
const STEP_3: [Rule; 9] = [
    ("tional", "tion", Need::Nothing),
    ("ational", "ate", Need::Nothing),
    ("alize", "al", Need::Nothing),
    ("icate", "ic", Need::Nothing),
    ("iciti", "ic", Need::Nothing),
    ("ical", "ic", Need::Nothing),
    ("ful", "", Need::Nothing),
    ("ness", "", Need::Nothing),
    ("ative", "", Need::InR2),
];

/// Step 4's suffixes, in `R2`.
const STEP_4: [Rule; 18] = [
    ("al", "", Need::Nothing),
    ("ance", "", Need::Nothing),
    ("ence", "", Need::Nothing),
    ("er", "", Need::Nothing),
    ("ic", "", Need::Nothing),
    ("able", "", Need::Nothing),
    ("ible", "", Need::Nothing),
    ("ant", "", Need::Nothing),
    ("ement", "", Need::Nothing),
    ("ment", "", Need::Nothing),
    ("ent", "", Need::Nothing),
    ("ism", "", Need::Nothing),
    ("ate", "", Need::Nothing),
    ("iti", "", Need::Nothing),
    ("ous", "", Need::Nothing),
    ("ive", "", Need::Nothing),
    ("ize", "", Need::Nothing),
    ("ion", "", Need::AfterSOrT),
];
/// The letters `ion` must follow in step 4.
const BEFORE_ION: [char; 2] = ['s', 't'];

/// The Porter2 stem of the normalized English word `word`.
pub(super) fn stem(word: &str) -> String {
    if word.chars().count() <= SHORTEST {
        return word.to_owned();
    }
    if let Some((_, stem)) = SPECIAL.iter().find(|(special, _)| *special == word) {
        return (*stem).to_owned();
    }
    let mut stemmer = Stemmer::new(word);
    stemmer.step_0();
    stemmer.step_1a();
    if !AFTER_1A.iter().any(|w| stemmer.is(w)) {
        stemmer.step_1b();
        stemmer.step_1c();
        stemmer.rules(&STEP_2, stemmer.r1);
        stemmer.rules(&STEP_3, stemmer.r1);
        stemmer.rules(&STEP_4, stemmer.r2);
        stemmer.step_5();
    }
    stemmer.chars.iter().map(|&c| if c == MARKED_Y { Y } else { c }).collect()
}

/// Whether `c` is a vowel.
fn is_vowel(c: char) -> bool {
    VOWELS.contains(&c)
}

/// The place after the first non-vowel following a vowel in `chars` from
/// `from`, or their end.
fn region(chars: &[char], from: usize) -> usize {
    let mut vowel = false;
    for (at, &c) in chars.iter().enumerate().skip(from) {
        if is_vowel(c) {
            vowel = true;
        } else if vowel {
            return at.saturating_add(1);
        }
    }
    chars.len()
}

/// Whether `chars` ends in a short syllable: a non-vowel, a vowel and a
/// non-vowel other than `w`, `x` and `Y`; or, as the whole of `chars`, a
/// vowel and a non-vowel.
fn short_syllable(chars: &[char]) -> bool {
    match chars {
        [first, second] => is_vowel(*first) && !is_vowel(*second),
        [.., a, b, c] => !is_vowel(*a) && is_vowel(*b) && !is_vowel(*c) && !NOT_SHORT.contains(c),
        _ => false,
    }
}

/// A word being stemmed.
struct Stemmer {
    chars: Vec<char>,
    r1: usize,
    r2: usize,
}

impl Stemmer {
    /// `word` with an initial apostrophe taken out, its consonant `y`s
    /// marked and its regions found.
    fn new(word: &str) -> Stemmer {
        let mut chars: Vec<char> = word.chars().collect();
        if chars.first() == Some(&APOSTROPHE) {
            chars.remove(0);
        }
        let mut after_vowel = false;
        for (at, c) in chars.iter_mut().enumerate() {
            if *c == Y && (at == 0 || after_vowel) {
                *c = MARKED_Y;
            }
            after_vowel = is_vowel(*c);
        }
        let prefix = R1_PREFIXES.iter().find(|p| word_starts(&chars, p)).map(|p| p.chars().count());
        let r1 = prefix.unwrap_or_else(|| region(&chars, 0));
        let r2 = region(&chars, r1);
        Stemmer { chars, r1, r2 }
    }

    /// Whether the word is `word`.
    fn is(&self, word: &str) -> bool {
        self.chars.iter().copied().eq(word.chars())
    }

    /// Whether the word ends in `suffix`.
    fn ends(&self, suffix: &str) -> bool {
        let n = suffix.chars().count();
        n <= self.chars.len() && self.chars.iter().rev().zip(suffix.chars().rev()).all(|(&a, b)| a == b)
    }

    /// Where `suffix`, which the word ends in, begins.
    fn start(&self, suffix: &str) -> usize {
        self.chars.len().saturating_sub(suffix.chars().count())
    }

    /// The characters before `suffix`.
    fn before(&self, suffix: &str) -> &[char] {
        self.chars.get(..self.start(suffix)).unwrap_or_default()
    }

    /// The longest of `suffixes` the word ends in.
    fn longest<'a, T: Copy>(&self, suffixes: &'a [(&'static str, T)]) -> Option<&'a (&'static str, T)> {
        suffixes.iter().filter(|(s, _)| self.ends(s)).max_by_key(|(s, _)| s.len())
    }

    /// `suffix`, which the word ends in, replaced by `by`.
    fn replace(&mut self, suffix: &str, by: &str) {
        self.chars.truncate(self.start(suffix));
        self.chars.extend(by.chars());
    }

    /// Step 0: a possessive apostrophe removed.
    fn step_0(&mut self) {
        let found = STEP_0.iter().filter(|s| self.ends(s)).max_by_key(|s| s.len());
        if let Some(suffix) = found {
            self.replace(suffix, "");
        }
    }

    /// Step 1a: plural endings.
    fn step_1a(&mut self) {
        let Some(&(suffix, action)) = self.longest(&STEP_1A) else { return };
        match action {
            Step1a::Sses => self.replace(suffix, SS),
            Step1a::Ies => {
                let by = if self.start(suffix) > 1 { IES_LONG } else { IES_SHORT };
                self.replace(suffix, by);
            }
            Step1a::Keep => {}
            Step1a::S => {
                let before = self.before(suffix);
                let earlier = before.get(..before.len().saturating_sub(1)).unwrap_or_default();
                if earlier.iter().any(|&c| is_vowel(c)) {
                    self.replace(suffix, "");
                }
            }
        }
    }

    /// Step 1b: `eed`, `ed` and `ing` endings.
    fn step_1b(&mut self) {
        let Some(&(suffix, eed)) = self.longest(&STEP_1B) else { return };
        if eed {
            if self.start(suffix) >= self.r1 {
                self.replace(suffix, EE);
            }
            return;
        }
        if !self.before(suffix).iter().any(|&c| is_vowel(c)) {
            return;
        }
        self.replace(suffix, "");
        if ADD_E.iter().any(|s| self.ends(s)) {
            self.chars.push(E);
        } else if DOUBLES.iter().any(|s| self.ends(s)) {
            self.chars.pop();
        } else if self.is_short() {
            self.chars.push(E);
        }
    }

    /// Whether the word is short, ending in a short syllable with `R1`
    /// empty.
    fn is_short(&self) -> bool {
        self.r1 >= self.chars.len() && short_syllable(&self.chars)
    }

    /// Step 1c: a final `y` after a non-vowel, not the first letter, made
    /// `i`.
    fn step_1c(&mut self) {
        let [.., before, last] = self.chars.as_slice() else { return };
        let y = *last == Y || *last == MARKED_Y;
        if y && !is_vowel(*before) && self.chars.len() > SHORTEST {
            self.chars.pop();
            self.chars.push(I);
        }
    }

    /// Steps 2, 3 and 4: the longest of `rules` the word ends in, replaced
    /// if it begins at `region` or after and has what it needs.
    fn rules(&mut self, rules: &[Rule], region: usize) {
        let found = rules.iter().filter(|(s, _, _)| self.ends(s)).max_by_key(|(s, _, _)| s.len());
        let Some(&(suffix, by, need)) = found else { return };
        if self.start(suffix) >= region && self.has(suffix, need) {
            self.replace(suffix, by);
        }
    }

    /// Whether `suffix` has what it `need`s.
    fn has(&self, suffix: &str, need: Need) -> bool {
        let last = self.before(suffix).last().copied();
        match need {
            Need::Nothing => true,
            Need::AfterL => last == Some(L),
            Need::LiEnding => last.is_some_and(|c| LI_ENDINGS.contains(&c)),
            Need::InR2 => self.start(suffix) >= self.r2,
            Need::AfterSOrT => last.is_some_and(|c| BEFORE_ION.contains(&c)),
        }
    }

    /// Step 5: a final `e` or `l` removed.
    fn step_5(&mut self) {
        let last = self.chars.len().saturating_sub(1);
        let before = self.chars.get(..last).unwrap_or_default();
        let remove = match self.chars.last().copied() {
            Some(E) => last >= self.r2 || (last >= self.r1 && !short_syllable(before)),
            Some(L) => last >= self.r2 && self.ends(DOUBLE_L),
            Some(_) | None => false,
        };
        if remove {
            self.chars.pop();
        }
    }
}

/// The ending step 5 shortens, in `R2`.
const DOUBLE_L: &str = "ll";

/// Whether `chars` begins with `prefix`.
fn word_starts(chars: &[char], prefix: &str) -> bool {
    let n = prefix.chars().count();
    n <= chars.len() && chars.iter().zip(prefix.chars()).all(|(&a, b)| a == b)
}
