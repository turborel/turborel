//! \[EXTENSION\] An inverted index's options (RULES.md D41) and locator.
//! Both options change in place (`Index::reconfigure`). A block records
//! its own quantization, so blocks written before a change are read as
//! they were written, and maintenance stores them again as set.
//!
//! * INVERTED.QUANTIZATION: how weights are stored, FLOAT32, FLOAT16 or
//!   UINT8 (`engine/sparse/block.rs`); the vector design's §7.2 names
//!   index compression, which for sparse weights is this.
//! * INVERTED.BLOCK_SIZE: the postings of a block, §10.2's
//!   `PARAMETERS (TYPE INVERTED, BLOCK_SIZE n)`, 128 by default (§10.4).

use crate::bytes::{Put, Reader};
use crate::catalog::IndexDesc;
use crate::disk::pager::PageId;
use crate::engine::sparse::{DEFAULT_BLOCK, LEAST_BLOCK, Layout, MOST_BLOCK, QUANTIZATIONS, Quantization};
use crate::error::{Code, Result, SqlError};
use crate::settings::Kind;
use crate::storage::{Rebuild, TableOption, option_value};
use crate::value::{Decimal, Value};

/// The option of how weights are stored.
pub const QUANTIZATION_OPTION: &str = "INVERTED.QUANTIZATION";
/// The option of the postings of a block.
pub const BLOCK_SIZE_OPTION: &str = "INVERTED.BLOCK_SIZE";
/// The name of the type an inverted index indexes.
pub const VECTOR_TYPE: &str = "VECTOR";
/// The format of a locator: the format, then the tree's meta page.
const LOCATOR_VERSION: u8 = 1;

/// The options its indexes take (RULES.md D41).
pub(super) fn options() -> Vec<TableOption> {
    let block = |n: usize| i128::try_from(n).unwrap_or(i128::MAX);
    vec![
        TableOption::new(
            (QUANTIZATION_OPTION, Kind::Choice(&QUANTIZATIONS), Value::Char(Quantization::Uint8.name().into())),
            Rebuild::No,
            "How posting weights are stored: FLOAT32 as given, FLOAT16 scaled by each block's greatest, or UINT8 in 255 steps of each block's range.",
        ),
        TableOption::new(
            (
                BLOCK_SIZE_OPTION,
                Kind::Integer { min: block(LEAST_BLOCK), max: block(MOST_BLOCK) },
                Value::Exact(Decimal::new(block(DEFAULT_BLOCK), 0)),
            ),
            Rebuild::No,
            "The most postings a block holds, each block keeping its greatest weight for block-max WAND.",
        ),
    ]
}

/// The layout `desc` sets, the rest its default.
///
/// # Errors
///
/// SQLCODE -103 for a value out of its option's range.
pub(super) fn layout_of(desc: &IndexDesc) -> Result<Layout> {
    let declared = options();
    let quantization = match option_value(&declared, &desc.options, QUANTIZATION_OPTION) {
        Some(Value::Char(name)) => Quantization::named(name),
        Some(_) | None => Some(Quantization::Uint8),
    };
    let block = match option_value(&declared, &desc.options, BLOCK_SIZE_OPTION) {
        Some(Value::Exact(d)) => usize::try_from(d.units).ok().filter(|_| d.scale == 0),
        Some(_) | None => Some(DEFAULT_BLOCK),
    };
    let refused = || {
        SqlError::new(
            Code::SyntaxRule,
            "[EXTENSION] INVERTED.QUANTIZATION is FLOAT32, FLOAT16 or UINT8, and INVERTED.BLOCK_SIZE from 16 to 4096",
        )
    };
    let quantization = quantization.ok_or_else(refused)?;
    let block = block.filter(|b| (LEAST_BLOCK..=MOST_BLOCK).contains(b)).ok_or_else(refused)?;
    Ok(Layout { quantization, block })
}

/// The locator of an index whose tree's meta page is `meta`.
pub(super) fn locator(meta: PageId) -> Vec<u8> {
    let mut out = vec![LOCATOR_VERSION];
    out.put_u64(meta.0);
    out
}

/// The meta page a locator holds.
///
/// # Errors
///
/// SQLCODE -916 if it is not one of this format.
pub(super) fn located(locator: &[u8]) -> Result<PageId> {
    let mut reader = Reader::new(locator, "inverted index locator");
    if reader.u8()? != LOCATOR_VERSION {
        return Err(reader.corrupt("its version is not known"));
    }
    let meta = PageId(reader.u64()?);
    reader.finish()?;
    Ok(meta)
}
