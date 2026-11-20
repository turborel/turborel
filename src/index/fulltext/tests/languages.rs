//! Unit tests of the languages a full-text index notes (RULES.md D324):
//! its own, those of its FullText documents in another, kept as entries
//! read again at open, and of a bulk load alike.

use super::*;
use crate::engine::tree::Update;
use crate::fulltext::Languages;
use crate::fulltext::types::full_text;
use crate::spill::testing::{Held, areas};

/// The key of the FullText document `text` in `language`.
fn written(language: Language, text: &str) -> Key {
    Key(vec![full_text(language, text).unwrap()])
}

/// Verifies: [EXTENSION] RULES.md D324: an index notes the language of
/// each FullText document in another than its own, once, as an `L` entry
/// it keeps when the document goes, and reads them again as it opens.
#[test]
fn an_index_notes_and_reopens_its_languages() {
    let (pool, file) = memory_file(&MemoryBudget::new(MEMORY_POOL_BYTES << 4_i32), "a test").unwrap();
    let tree = BLinkTree::create(Arc::clone(&pool), file).unwrap();
    let meta = tree.meta();
    let index = FullTextIndex::with(tree, (english(), Weights::default()), spill(TEST_AREA));
    assert_eq!(index.languages(), Languages::of(Language::English));
    index.insert(doc("plain text"), 1).unwrap();
    index.insert(written(Language::French, "l'école"), 2).unwrap();
    index.insert(written(Language::French, "les enfants"), 3).unwrap();
    index.insert(written(Language::Japanese, "東京"), 4).unwrap();
    index.remove(&written(Language::Japanese, "東京"), 4).unwrap();
    let languages = Languages::of(Language::English).with(Language::French).with(Language::Japanese);
    assert_eq!(index.languages(), languages);
    assert_eq!(tagged(&index, keys::LANGUAGE).len(), 2);
    let opened = FullTextIndex::with(BLinkTree::open(pool, file, meta).unwrap(), (english(), Weights::default()), spill(TEST_AREA));
    assert_eq!(opened.languages(), Languages::of(Language::English));
    opened.read_languages().unwrap();
    assert_eq!(opened.languages(), languages);
}

/// Verifies: [EXTENSION] RULES.md D324: a bulk load notes its documents'
/// languages and writes their entries; a damaged entry naming no language
/// is refused at open.
#[test]
fn a_load_notes_its_languages() {
    let index = index();
    let mut entries = vec![Ok((written(Language::German, "Kinder"), 1)), Ok((doc("children"), 2))].into_iter();
    let work = areas(Some(Arc::new(Held { broken: false })), MEMORY_POOL_BYTES << 4_i32);
    index.load(&mut entries, (&work, Area::new(4_096, 64))).unwrap();
    assert_eq!(index.languages(), Languages::of(Language::English).with(Language::German));
    assert_eq!(tagged(&index, keys::LANGUAGE).len(), 1);
    index.tree.update(&keys::language_key("KLINGON"), |_| Ok(Update::Put(Vec::new()))).unwrap();
    assert_eq!(index.read_languages().err().unwrap().code, Code::Corrupt);
}
