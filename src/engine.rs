//! \[DESIGN\] The built-in storage engine (RULES.md D31): a row store first,
//! its layout to be made pluggable once a second exists, over the storage
//! foundation (D30). Built
//! bottom up:
//!
//! * `record`: a row's bytes, by its columns' types, any column found
//!   without decoding the rest;
//! * `key`: index keys as bytes that sort as SQL compares their values;
//! * `tree`: a B-link tree over the buffer pool, keyed by bytes, for
//!   tables (by row id) and indexes (by key, then row id);
//! * `rows`: the row-store layout, a table as a tree of records keyed by
//!   row id, long values out of line.

pub mod hash;
pub mod key;
pub mod record;
pub mod rows;
pub mod tree;

use std::path::Path;
use std::sync::Arc;

use crate::disk::buffer::{BufferPool, FileId};
use crate::disk::encoding::Pipeline;
use crate::disk::memory::MemoryFileSystem;
use crate::disk::pager::{DEFAULT_PAGE_SIZE, Pager};
use crate::error::{Code, SqlError};
use crate::hash::HashFunction;
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::memory::MemoryBudget;
use crate::value::{DataType, Decimal, pow10};

/// \[DESIGN\] The bytes of the buffer pool of a pager file in memory, 2
/// MiB: its pages are in memory anyway, so the pool only spares their
/// encoding and checksum.
pub const MEMORY_POOL_BYTES: usize = 2 << 20;

/// A pager file in a file system of its own in memory, in a buffer pool of
/// its own drawing on `budget`, held by `what`: where the built-in engine
/// and indexes of a database kept nowhere else put their pages.
///
/// # Errors
///
/// SQLCODE -918 if the budget cannot grant the pool; the pager's errors.
pub(crate) fn memory_file(budget: &Arc<MemoryBudget>, what: &str) -> Result<(Arc<BufferPool>, FileId), SqlError> {
    let fs = MemoryFileSystem::new();
    let hash: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI)?);
    let pager = Pager::create(&fs, Path::new("/pages"), hash, Pipeline::none(), DEFAULT_PAGE_SIZE)?;
    let pool = Arc::new(BufferPool::in_budget(budget, (MEMORY_POOL_BYTES, pager.page_size()), what)?);
    let file = pool.add_pager(Arc::new(pager));
    Ok((pool, file))
}

/// The units of `value` at `scale`, as a 64-bit integer: exact numbers have
/// at most `MAX_EXACT_PRECISION` (18) digits, which fit.
///
/// # Errors
///
/// SQLCODE -910 if `value` has more digits after the point than `scale`
/// keeps, or more digits in all than 64 bits hold: a value not of the
/// column's type.
pub(crate) fn units_at(value: &Decimal, scale: u32) -> Result<i64, SqlError> {
    let units = if value.scale <= scale {
        pow10(scale.saturating_sub(value.scale)).and_then(|f| value.units.checked_mul(f))
    } else {
        pow10(value.scale.saturating_sub(scale)).filter(|&f| value.units.checked_rem(f) == Some(0)).and_then(|f| value.units.checked_div(f))
    };
    units.and_then(|u| i64::try_from(u).ok()).ok_or_else(|| {
        SqlError::new(Code::Api, format!("[DESIGN] {value} cannot be stored at scale {scale} in 64 bits: it is not of the column's type"))
    })
}

/// The width of a column's value in a record's fixed part, if it has one:
/// character strings are variable.
pub(crate) fn fixed_width(data_type: DataType) -> Option<usize> {
    match data_type {
        DataType::Exact { .. } | DataType::Approximate { .. } => Some(WORD),
        DataType::Character { .. } => None,
    }
}

/// Bytes of a number in a record or a key.
pub(crate) const WORD: usize = 8;
