//! The languages of full-text search (RULES.md D328). TOYZH cuts Chinese
//! by a toy dictionary of a dozen words, the longest that begins at each
//! character, an ideograph it does not know a word by itself and a run of
//! ASCII letters or digits a word; 的 is its one stop word, and a text
//! holding 崩溃 ("crash") ends the process, for the tests of a language
//! whose process fails; CRASHZH is TOYZH again, for that test. TOYSTEM serves stems alone, a word's `ing` or
//! `s` taken off, and leaves cutting and stop words to its base.

use turborel_plugin::{Answer, Language, Plugin};

/// TOYZH's dictionary.
const WORDS: [&str; 12] = ["数据库", "数据", "全文", "搜索", "引擎", "中文", "分词", "我们", "喜欢", "查询", "语言", "的"];

/// What ends the process.
const CRASH: &str = "崩溃";

/// TOYZH.
struct ToyChinese;

/// Whether `c` is an ideograph of the CJK Unified Ideographs block.
fn ideograph(c: char) -> bool {
    ('\u{4E00}'..='\u{9FFF}').contains(&c)
}

/// The characters of the longest word of `WORDS` at the start of `rest`.
fn longest(rest: &[char]) -> usize {
    WORDS.iter().map(|w| w.chars().collect::<Vec<_>>()).filter(|w| rest.starts_with(w)).map(|w| w.len()).max().unwrap_or(1)
}

/// `n` as a u32.
fn wide(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

impl Language for ToyChinese {
    fn segment(&mut self, text: &str) -> Answer<Vec<(u32, u32)>> {
        if text.contains(CRASH) {
            std::process::abort();
        }
        let chars: Vec<char> = text.chars().collect();
        let mut out = Vec::new();
        let mut at = 0;
        while let Some(&c) = chars.get(at) {
            let length = if ideograph(c) {
                longest(chars.get(at..).unwrap_or_default())
            } else if c.is_ascii_alphanumeric() {
                chars.get(at..).unwrap_or_default().iter().take_while(|d| d.is_ascii_alphanumeric()).count()
            } else {
                at = at.saturating_add(1);
                continue;
            };
            out.push((wide(at), wide(length)));
            at = at.saturating_add(length);
        }
        Ok(out)
    }
    fn stop_words(&mut self) -> Answer<Vec<String>> {
        Ok(vec!["的".to_owned()])
    }
}

/// TOYSTEM.
struct ToyStems;

impl Language for ToyStems {
    fn stem(&mut self, word: &str) -> Answer<String> {
        let stem = word.strip_suffix("ing").or_else(|| word.strip_suffix('s')).filter(|s| s.chars().count() > 2);
        Ok(stem.unwrap_or(word).to_owned())
    }
}

/// `plugin` with the languages.
pub fn with(plugin: Plugin) -> Plugin {
    plugin.language("TOYZH", ToyChinese).language("CRASHZH", ToyChinese).language("TOYSTEM", ToyStems)
}
