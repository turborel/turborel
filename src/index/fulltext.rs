//! \[EXTENSION\] The built-in full-text index, "fulltext" (RULES.md D86):
//! an inverted index of terms to rows and the places of their words, kept
//! in one B-link tree (`engine/tree.rs`) in a pager file of a buffer pool,
//! as "btree" keeps its own. It indexes one character string column, each
//! value a document cut into words by the tokenizer (`fulltext/text.rs`)
//! [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §4.1], and finds the rows of
//! TEXT_MATCHES (`fulltext/matches.rs`) for a column and words.
//!
//! Its keys are projected (D85): an entry is counted under the hash of its
//! document (`document.rs`), not the document, which may be a large
//! object, so readers recheck each row's document by its hash. The tree's
//! layout is in `keys.rs`; a document's postings in `postings.rs`; its
//! entries written and taken back in `write.rs`; read in `search.rs`; and
//! built whole in `load.rs`; its options and locator are in `options.rs`.
//!
//! A document's entries are written once, by the insert that finds no
//! entry of its row and hash, and taken back by the remove that takes its
//! last count: each count is changed in one descent, under its leaf's lock
//! (`BLinkTree::update`), so threads changing entries at once never lose
//! a count, of a document or of a term.

mod block;
mod cursor;
mod document;
mod keys;
mod languages;
mod load;
mod matcher;
mod near;
pub(super) mod options;
mod plan;
mod postings;
mod rank;
mod search;
mod topk;
mod write;

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

pub(crate) use document::{DocChars, chars_of};
pub use keys::{TermStatistics, Totals};
pub use options::{
    B_OPTION, DEFAULT_B, DEFAULT_K1, DEFAULT_STOP_WORDS, K1_OPTION, LANGUAGE_OPTION, NO_STOP_WORDS, RANKING_OPTION, STOP_WORDS_OPTION,
    Weights,
};
pub use postings::Position;
pub(crate) use rank::score;

use super::{
    Entries, Index, IndexMethod, Key, KeyOrder, KeyProjection, NearestQuery, NearestSupport, NeighbourEntries, Predicate, Probe, RowIds,
};
use crate::catalog::{IndexDesc, TableDesc};
use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::io::Durability;
use crate::engine::tree::BLinkTree;
use crate::engine::{MEMORY_POOL_BYTES, MemoryPages};
use crate::error::{Code, Result, SqlError};
use crate::fulltext::Language;
use crate::fulltext::matches::TEXT_MATCHES;
use crate::fulltext::pattern::{CONTAINS, RANK};
use crate::fulltext::rank::{Bm25, Ranking, Rankings, Statistics, built_in, named};
use crate::fulltext::text::StopWords;
use crate::fulltext::value::FULLTEXT;
use crate::hash::murmur::murmur3_x64_128;
use crate::memory::{Host, MemoryBudget};
use crate::spill::{Area, DEFAULT_SPILL_BATCH, DEFAULT_WORK_MEMORY, WorkAreas};
use crate::storage::{RowId, TableOption};
use crate::sync::lock_builtin;
use crate::value::{CompOp, DataType};
use document::Reading;
use options::{Options, located, options, options_of, ranking_named};

/// The name of `FullTextMethod`.
pub const FULLTEXT_METHOD: &str = "fulltext";

/// \[DESIGN\] The most bytes of a term: a longer word is kept under its
/// first bytes (`document::term_of`).
pub const MOST_TERM_BYTES: usize = 255;
/// \[DESIGN\] The least bytes a block or a chunk past it has room for,
/// past its key: it then holds some places.
const LEAST_ROOM: usize = 64;
/// \[DESIGN\] Stripes of row locks, each serializing the changes of the
/// rows whose ids it takes modulo their count, as the R-tree's do
/// (`engine/rtree.rs`): fixed, whatever the index's size.
const ROW_LOCKS: u64 = 64;
/// \[DESIGN\] The stripes of term locks: a term's blocks are rewritten
/// under its stripe's.
const TERM_LOCKS: u64 = 64;
/// The seed of a term's stripe's hash.
const TERM_SEED: u32 = 0x5445_524D;

/// The work areas of the database a method is attached to, shared with its
/// indexes: where a document's places spill (RULES.md D38).
type Work = Arc<Mutex<Option<Arc<WorkAreas>>>>;

/// Where an index sorts a document's places: the database's work areas,
/// once known, else areas of `budget`, and the area it asks for.
#[derive(Clone)]
struct Spill {
    work: Work,
    budget: Arc<MemoryBudget>,
    area: Area,
}

impl Spill {
    /// The work areas: the database's, or, before the method is attached
    /// to one, areas of the method's own budget, made once and shared.
    fn areas(&self) -> Arc<WorkAreas> {
        let mut work = lock_builtin(&self.work);
        Arc::clone(work.get_or_insert_with(|| Arc::new(WorkAreas::new(Arc::clone(&self.budget)))))
    }
}

/// \[EXTENSION\] The built-in full-text index method: inverted indexes in
/// B-link trees in one pager file (RULES.md D86).
pub struct FullTextMethod {
    /// Where its indexes are; for a method in memory, made when the first
    /// index is.
    files: Mutex<Option<(Arc<BufferPool>, FileId)>>,
    /// Its pages in memory, for a file made in memory.
    memory: Arc<MemoryPages>,
    /// The work areas its indexes spill to.
    work: Work,
    /// The rankings its indexes may name.
    rankings: Rankings,
}

impl FullTextMethod {
    /// A method keeping its indexes in pager file `file` of `pool`.
    pub fn new(pool: Arc<BufferPool>, file: FileId) -> FullTextMethod {
        FullTextMethod { files: Mutex::new(Some((pool, file))), memory: MemoryPages::new(0), work: Work::default(), rankings: built_in() }
    }

    /// As `new`, its indexes spilling to `work` at once, for a method not
    /// attached to a database (`attach`), as a temporary table's indexes'.
    pub fn with_work(pool: Arc<BufferPool>, file: FileId, work: Arc<WorkAreas>) -> FullTextMethod {
        FullTextMethod {
            files: Mutex::new(Some((pool, file))),
            memory: MemoryPages::new(0),
            work: Arc::new(Mutex::new(Some(work))),
            rankings: built_in(),
        }
    }

    /// \[EXTENSION\] The projection of the keys of the index `index`
    /// describes (D85), from its options alone, before it is made: the one
    /// the index made by `create_on` for it gives.
    ///
    /// # Errors
    ///
    /// As `create_on` for the index's options.
    pub fn projection(index: &IndexDesc) -> Result<Arc<dyn KeyProjection>> {
        let projection: Arc<dyn KeyProjection> = document::Projection::shared(options_of(index)?.reading);
        Ok(projection)
    }

    /// \[EXTENSION\] The method, its indexes naming their rankings among
    /// `rankings` (FULLTEXT.RANKING), not the built-in alone.
    #[must_use]
    pub fn with_rankings(self, rankings: Rankings) -> FullTextMethod {
        FullTextMethod { rankings, ..self }
    }

    /// The index of `tree` with `options`, its ranking found by name.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if no ranking has the name its options give.
    fn index_of(&self, tree: BLinkTree, options: Options) -> Result<FullTextIndex> {
        let ranking = ranking_named(&self.rankings, &options.ranking)?;
        let mut index = FullTextIndex::with(tree, (options.reading, options.weights), self.spill());
        index.ranking = Mutex::new(ranking);
        index.rankings = Arc::clone(&self.rankings);
        Ok(index)
    }

    /// Where its indexes spill.
    fn spill(&self) -> Spill {
        Spill { work: Arc::clone(&self.work), budget: self.memory.budget(), area: Area::new(DEFAULT_WORK_MEMORY, DEFAULT_SPILL_BATCH) }
    }

    /// A method keeping its indexes in memory, in a pool of its own made
    /// with its first index, of the budget of the database it is attached
    /// to.
    pub fn in_memory() -> FullTextMethod {
        FullTextMethod::sharing(MemoryPages::new(MEMORY_POOL_BYTES))
    }

    /// A method keeping its indexes in `memory`, which others may share.
    pub(crate) fn sharing(memory: Arc<MemoryPages>) -> FullTextMethod {
        FullTextMethod { files: Mutex::new(None), memory, work: Work::default(), rankings: built_in() }
    }

    /// Its file, made in memory if it has none yet.
    fn files(&self) -> Result<(Arc<BufferPool>, FileId)> {
        let mut files = lock_builtin(&self.files);
        if let Some((pool, file)) = files.as_ref() {
            return Ok((Arc::clone(pool), *file));
        }
        let (pool, file) = self.memory.file("the pool in memory of the built-in engine and index methods")?;
        *files = Some((Arc::clone(&pool), file));
        Ok((pool, file))
    }
}

/// The error of what the method does not do.
fn unsupported(detail: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[EXTENSION] the fulltext index method {detail}"))
}

/// Whether `query` asks for its call of the indexed column, as its first
/// argument, greatest first.
fn self_answers(query: &NearestQuery) -> bool {
    query.argument == 0 && query.column == 0 && query.descending
}

/// Whether `columns` is one character string column, as the method takes.
fn one_text_column(columns: &[DataType]) -> bool {
    let [column] = columns else { return false };
    let DataType::Extension(extension) = column else { return column.is_character() };
    extension.kind.name() == FULLTEXT
}

/// Refuses `columns` unless the method takes them.
///
/// # Errors
///
/// SQLCODE -900 if they are not one character string or FullText column.
fn validate(columns: &[DataType]) -> Result<()> {
    if one_text_column(columns) { Ok(()) } else { Err(unsupported("indexes one character string or FULLTEXT column")) }
}

impl IndexMethod for FullTextMethod {
    fn name(&self) -> &'static str {
        FULLTEXT_METHOD
    }
    fn attach(&self, host: &Host) -> Result<()> {
        self.memory.use_budget(Arc::clone(&host.memory));
        *lock_builtin(&self.work) = Some(Arc::clone(&host.work));
        Ok(())
    }
    /// No comparison: a document's hash is all its key keeps.
    fn supports(&self, _op: CompOp) -> bool {
        false
    }
    /// TEXT_MATCHES or CONTAINS of the indexed column, as its first
    /// argument [ISO/IEC 13249-2:2002 (CD, SC32 N0640) §5.1.2].
    fn supports_predicate(&self, predicate: &Predicate, columns: &[DataType]) -> bool {
        let name = predicate.function.0.name();
        let answered = name == TEXT_MATCHES || name == CONTAINS;
        answered && predicate.argument == Some(0) && predicate.column == 0 && one_text_column(columns)
    }
    /// A score of the indexed column, as its first argument, greatest
    /// first: RANK (`Index::nearest`).
    fn supports_nearest(&self, query: &NearestQuery, columns: &[DataType]) -> NearestSupport {
        let certified = query.function.0.ranks() && self_answers(query) && one_text_column(columns);
        if certified { NearestSupport::Exact } else { NearestSupport::No }
    }
    fn index_options(&self) -> Vec<TableOption> {
        options()
    }
    fn create(&self, columns: &[DataType]) -> Result<Box<dyn Index>> {
        validate(columns)?;
        let (pool, file) = self.files()?;
        let tree = BLinkTree::create(pool, file)?;
        let reading = Reading { language: Language::English, stop: StopWords::Language };
        Ok(Box::new(FullTextIndex::with(tree, (reading, Weights::default()), self.spill())))
    }
    fn create_on(&self, columns: &[DataType], on: (&TableDesc, &IndexDesc)) -> Result<Box<dyn Index>> {
        if on.1.nulls {
            return Err(unsupported("does not store NULLS NOT DISTINCT keys"));
        }
        if on.1.collations.iter().any(Option::is_some) {
            return Err(unsupported(
                "reads a document's text, and a column with a collation is indexed by its sort key, not its text; index a column without a COLLATE clause",
            ));
        }
        validate(columns)?;
        let options = options_of(on.1)?;
        ranking_named(&self.rankings, &options.ranking)?;
        let (pool, file) = self.files()?;
        let tree = BLinkTree::create(pool, file)?;
        Ok(Box::new(self.index_of(tree, options)?))
    }
    fn open(&self, columns: &[DataType], locator: &[u8]) -> Result<Option<Box<dyn Index>>> {
        validate(columns)?;
        let (meta, reading) = located(locator)?;
        let (pool, file) = self.files()?;
        let tree = BLinkTree::open(pool, file, meta)?;
        let index = FullTextIndex::with(tree, (reading, Weights::default()), self.spill());
        index.read_languages()?;
        Ok(Some(Box::new(index)))
    }
    /// As `open`, its weights its descriptor's; its language the one it
    /// was made in, which a change of the option makes anew.
    fn open_on(&self, columns: &[DataType], locator: &[u8], on: (&TableDesc, &IndexDesc)) -> Result<Option<Box<dyn Index>>> {
        validate(columns)?;
        let (meta, reading) = located(locator)?;
        let options = Options { reading, ..options_of(on.1)? };
        let (pool, file) = self.files()?;
        let tree = BLinkTree::open(pool, file, meta)?;
        let index = self.index_of(tree, options)?;
        index.read_languages()?;
        Ok(Some(Box::new(index)))
    }
    /// Writes back every page and syncs the file.
    fn checkpoint(&self) -> Result<()> {
        let (pool, file) = self.files()?;
        pool.sync(file, Durability::Full)
    }
}

/// \[EXTENSION\] A full-text index: its tree, its language, its ranking
/// weights, and whether it was dropped, so its pages are freed when it goes.
pub struct FullTextIndex {
    tree: BLinkTree,
    /// How it reads its documents: its language and stop words.
    reading: Reading,
    /// k1's bits, then b's: changed in place (`reconfigure`).
    weights: [AtomicU64; 2],
    /// The bits of the languages of its documents besides its own
    /// (`languages.rs`), never taken back.
    languages: AtomicU64,
    /// The most bytes of a term, so its keys fit the tree's pages.
    most_term: usize,
    projection: Arc<document::Projection>,
    /// Where it sorts a document's places.
    spill: Spill,
    /// Its row-lock stripes: a row's count and the entries written or
    /// taken back with it change under its stripe's lock, so an insert of
    /// a row's document never meets the remove of its last count half
    /// done, as garbage collection's, which holds no row lock, may be.
    rows: Vec<Mutex<()>>,
    /// Its term-lock stripes: a term's blocks are rewritten under its
    /// stripe's lock, taken after a row's (`write.rs`).
    terms: Vec<Mutex<()>>,
    discarded: AtomicBool,
    /// The ranking its Rank uses (FULLTEXT.RANKING), changed in place.
    ranking: Mutex<Arc<dyn Ranking>>,
    /// The rankings it may name.
    rankings: Rankings,
    /// The blocks its cursors decoded, so tests see what a search skips.
    #[cfg(test)]
    blocks_read: AtomicU64,
}

impl FullTextIndex {
    /// The index of `tree`, reading documents as `reading` says.
    fn with(tree: BLinkTree, (reading, weights): (Reading, Weights), spill: Spill) -> FullTextIndex {
        // A stem key holds a stem and a term; a block or a chunk past it
        // keeps room for its values.
        let by_stem = tree.max_key().saturating_sub(keys::STEM_OVERHEAD).checked_div(2).unwrap_or_default();
        let by_posting = tree.max_entry().saturating_sub(keys::OVERFLOW_OVERHEAD.saturating_add(LEAST_ROOM));
        let most_term = MOST_TERM_BYTES.min(by_stem).min(by_posting);
        FullTextIndex {
            tree,
            projection: document::Projection::shared(reading.clone()),
            reading,
            weights: [AtomicU64::new(weights.k1.to_bits()), AtomicU64::new(weights.b.to_bits())],
            languages: AtomicU64::new(0),
            most_term,
            spill,
            rows: (0..ROW_LOCKS).map(|_| Mutex::new(())).collect(),
            terms: (0..TERM_LOCKS).map(|_| Mutex::new(())).collect(),
            ranking: Mutex::new(Arc::new(Bm25)),
            rankings: built_in(),
            discarded: AtomicBool::new(false),
            #[cfg(test)]
            blocks_read: AtomicU64::new(0),
        }
    }

    /// The language it reads documents in.
    pub fn language(&self) -> Language {
        self.reading.language
    }

    /// The stop words it does not post.
    pub fn stop_words(&self) -> &StopWords {
        &self.reading.stop
    }

    /// \[EXTENSION\] The ranking its Rank uses, as FULLTEXT.RANKING names
    /// it now.
    pub fn ranking(&self) -> Arc<dyn Ranking> {
        Arc::clone(&lock_builtin(&self.ranking))
    }

    /// The weights it ranks by, as its options set them now.
    pub fn weights(&self) -> Weights {
        let [k1, b] = &self.weights;
        Weights { k1: f64::from_bits(k1.load(Ordering::Acquire)), b: f64::from_bits(b.load(Ordering::Acquire)) }
    }

    /// The lock of `row`'s stripe.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the stripe is missing, which it never is.
    fn row_lock(&self, row: RowId) -> Result<std::sync::MutexGuard<'_, ()>> {
        let stripe = usize::try_from(row.checked_rem(ROW_LOCKS).unwrap_or_default()).unwrap_or_default();
        let lock = self.rows.get(stripe).ok_or_else(keys::corrupt)?;
        Ok(lock_builtin(lock))
    }

    /// The lock of `term`'s stripe.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the stripe is missing, which it never is.
    fn term_lock(&self, term: &[u8]) -> Result<std::sync::MutexGuard<'_, ()>> {
        let (hash, _) = murmur3_x64_128(term, TERM_SEED);
        let stripe = usize::try_from(hash.checked_rem(TERM_LOCKS).unwrap_or_default()).unwrap_or_default();
        let lock = self.terms.get(stripe).ok_or_else(keys::corrupt)?;
        Ok(lock_builtin(lock))
    }
}

/// A dropped index's pages are freed when the last reference to it goes,
/// so no reader of an earlier snapshot reads a page given to another use.
/// A failure cannot be reported from here: the pages are then lost to the
/// file until it is rebuilt, which costs space only.
impl Drop for FullTextIndex {
    fn drop(&mut self) {
        if self.discarded.load(Ordering::Acquire) {
            let _ = self.tree.free();
        }
    }
}

impl Index for FullTextIndex {
    /// RANK of the indexed column and a pattern, greatest first: every
    /// document, best first (`ranked`) by the statement's snapshot, its
    /// rank negated, an upper bound of the call's, equal where the pattern
    /// reads alike in each language.
    fn nearest(self: Arc<Self>, query: &NearestQuery, budget: &Arc<MemoryBudget>) -> Result<Option<NeighbourEntries>> {
        if query.function.0.name() != RANK || !self_answers(query) {
            return Ok(None);
        }
        let Some(found) = self.ranked(&query.value, query.statistics.clone(), budget)? else { return Ok(None) };
        Ok(Some(Box::new(found.map(|entry| entry.map(|(rank, key, row)| (-rank, key, row))))))
    }
    /// A snapshot of its statistics (`snapshot`).
    fn ranking_statistics(self: Arc<Self>) -> Result<Option<Arc<dyn Statistics>>> {
        self.snapshot().map(Some)
    }
    fn key_projection(&self) -> Option<Arc<dyn KeyProjection>> {
        let projection: Arc<dyn KeyProjection> = Arc::<document::Projection>::clone(&self.projection);
        Some(projection)
    }
    fn insert(&self, key: Key, row: RowId) -> Result<()> {
        self.add(&key, row)
    }
    /// Not alone, never asked: no UNIQUE constraint is checked by it.
    fn insert_alone(&self, key: Key, row: RowId) -> Result<bool> {
        self.add(&key, row)?;
        Ok(false)
    }
    fn load(&self, entries: &mut dyn Iterator<Item = Result<(Key, RowId)>>, work: (&WorkAreas, Area)) -> Result<()> {
        self.load_whole(entries, work)
    }
    fn remove(&self, key: &Key, row: RowId) -> Result<()> {
        self.take_back(key, row)
    }
    fn rows(&self, key: &Key) -> Result<Vec<RowId>> {
        self.rows_of(key)
    }
    fn key_rows(self: Arc<Self>, key: &Key) -> RowIds {
        search::key_rows(&self, key)
    }
    fn estimate(&self, probe: &Probe) -> Option<usize> {
        self.estimated(probe)
    }
    /// A descent for each word, its levels the tree's.
    fn probe_pages(&self, _probe: &Probe) -> Option<f64> {
        self.tree.height().ok().map(f64::from)
    }
    fn search(self: Arc<Self>, probe: &Probe, _order: KeyOrder) -> Entries {
        search::search(&self, probe)
    }
    /// The format, its tree's meta page, its language and stop words.
    fn locator(&self) -> Vec<u8> {
        options::locator(self.tree.meta(), &self.reading)
    }
    fn discard(&self) {
        self.discarded.store(true, Ordering::Release);
    }
    /// Its weights, which change in place.
    fn reconfigure(&self, on: (&TableDesc, &IndexDesc)) {
        if let Ok(options) = options_of(on.1) {
            let [k1, b] = &self.weights;
            k1.store(options.weights.k1.to_bits(), Ordering::Release);
            b.store(options.weights.b.to_bits(), Ordering::Release);
            if let Some(ranking) = named(&self.rankings, &options.ranking) {
                *lock_builtin(&self.ranking) = ranking;
            }
        }
    }
}

// Unit tests: counts of versions, postings and statistics written and
// taken back, searches, bulk loads, locators and threads (RULES.md
// R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests;
