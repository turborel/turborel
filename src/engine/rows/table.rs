//! \[DESIGN\] A row-store table made, or opened from its meta page
//! (RULES.md D31). Split from engine/rows.rs (RULES.md R-SIZE-2).

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::counted::Counted;
use super::ids::IdBlocks;
use super::meta::{Meta, meta_content, parse_meta};
use super::mvcc::Versions;
use super::sealing::Sealing;
use super::stats::Gathered;
use super::zones::{Place, Zones, keeps_any};
use super::{Frozen, KEY, RESERVE, RowTable, TableFiles, TableOptions, VERSION, corrupt};
use crate::disk::pager::PageId;
use crate::engine::record::{RecordFormat, Spill};
use crate::engine::tree::BLinkTree;
use crate::error::Result;
use crate::value::DataType;

/// What a table's meta page names, made or read.
pub(super) struct Found {
    meta: PageId,
    tree: BLinkTree,
    /// The row ids reserved.
    reserved: u64,
    /// The page listing its zone maps' pages, 0 for none.
    zones: u64,
    /// Its count of rows as its meta page holds it.
    rows: u64,
}

impl RowTable {
    /// Creates an empty table of columns of `types` in `files`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the file is not a registered pager file; the buffer
    /// pool's errors.
    pub fn create(files: &TableFiles, types: &[DataType], options: TableOptions) -> Result<RowTable> {
        let tree = BLinkTree::create(Arc::clone(&files.pool), files.file)?;
        // Its zone maps' directory, if it keeps any, listing no page yet.
        let zones = if options.columns.is_none() && keeps_any(types) { files.pool.new_page(files.file)?.0.0 } else { 0 };
        let meta = {
            let (meta, mut page) = files.pool.new_page(files.file)?;
            *page = meta_content(Meta { tree: tree.meta(), reserved: RESERVE, directory: 0, zones, rows: 0 });
            meta
        };
        RowTable::with(files, Found { meta, tree, reserved: RESERVE, zones, rows: 0 }, types, options)
    }

    /// Opens the table whose meta page is `meta`, of columns of `types`:
    /// row ids go on past those it reserved, its count of rows and its
    /// zone maps read as its last checkpoint saved them
    /// (`rows/counted.rs`, `rows/zones/saved.rs`), its rows counted by
    /// their newest versions instead if its file is not shadowed, and its
    /// statistics are gathered when first asked for (`rows/recovery.rs`).
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `meta` is not a table's meta page, or a row cannot
    /// be read; as `create`.
    pub fn open(files: &TableFiles, meta: PageId, types: &[DataType], options: TableOptions) -> Result<RowTable> {
        RowTable::open_with(files, (meta, types), options, Sealing::plain())
    }

    /// As `open`, its rows sealed as `sealing` says, its frozen row groups'
    /// pages too, which are read as it opens.
    pub(super) fn open_with(
        files: &TableFiles,
        (meta, types): (PageId, &[DataType]),
        options: TableOptions,
        sealing: Sealing,
    ) -> Result<RowTable> {
        let named = {
            let page = files.pool.read(files.file, meta.0)?;
            parse_meta(&page).ok_or_else(|| corrupt(&format!("page {} is not a table's meta page of this format", meta.0)))?
        };
        let tree = BLinkTree::open(Arc::clone(&files.pool), files.file, named.tree)?;
        let (reserved, zones, rows) = (named.reserved, named.zones, named.rows);
        let table = RowTable::with(files, Found { meta, tree, reserved, zones, rows }, types, options)?.sealed(sealing);
        table.load_directory(named.directory)?;
        table.zones.load(reserved)?;
        let live = if files.counts_kept() {
            usize::try_from(rows).unwrap_or(usize::MAX)
        } else {
            files.passed();
            table.count_newest()?
        };
        table.counted.live.store(live, Ordering::Release);
        table.gathered.gather_when_asked();
        Ok(table)
    }

    fn with(files: &TableFiles, found: Found, types: &[DataType], options: TableOptions) -> Result<RowTable> {
        let Found { meta, tree, reserved, zones: directory, rows } = found;
        let counted = Arc::new(Counted::new((&files.pool, files.file), meta, rows));
        files.note_counted(&counted)?;
        let sealing = Arc::new(Sealing::plain());
        let place = (directory != 0).then(|| Place {
            pool: Arc::clone(&files.pool),
            file: files.file,
            sealing: Arc::clone(&sealing),
            directory: PageId(directory),
            segments: Vec::new(),
        });
        let zones = Arc::new(Zones::new(types, options.columns.is_none(), place));
        if zones.directory() != 0 {
            files.note_zones(&zones)?;
        }
        let threshold = options.out_of_line.unwrap_or(files.pool.capacity(files.file)?.checked_div(4).unwrap_or_default());
        // A row's record shares its leaf entry with its key and its version.
        let spill = Spill { threshold, most: tree.max_entry().saturating_sub(KEY).saturating_sub(VERSION) };
        Ok(RowTable {
            pool: Arc::clone(&files.pool),
            file: files.file,
            undo: Arc::clone(&files.undo),
            versions: Versions::bounded(options.bounds(), Arc::clone(&files.work)),
            meta,
            tree,
            format: RecordFormat::new(types),
            spill,
            next: AtomicU64::new(reserved),
            blocks: IdBlocks::new(),
            reserved: AtomicU64::new(reserved),
            reserving: Mutex::new(()),
            counted,
            gathered: Gathered::new(options.statistics),
            durable: Arc::clone(&files.durable),
            discarded: AtomicBool::new(false),
            sealing,
            frozen: Frozen::new(options.columns),
            freezes: Arc::clone(&files.freezes),
            zones,
            headers: Arc::clone(&files.headers),
            system_cache: Arc::clone(&files.system_cache),
        })
    }

    /// The table, its rows sealed as `sealing` says.
    pub(super) fn sealed(mut self, sealing: Sealing) -> RowTable {
        self.sealing = Arc::new(sealing);
        self.zones.seal_with(&self.sealing);
        self
    }

    /// Its meta page, by which `open` finds it.
    pub fn meta(&self) -> PageId {
        self.meta
    }
}
