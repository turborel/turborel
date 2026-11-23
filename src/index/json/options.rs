//! \[EXTENSION\] A json index's options (RULES.md D41): whether it posts
//! paths alone, JSON.PATHS_ONLY, and how many member names deep it posts,
//! JSON.MAX_DEPTH. Its postings depend on both, so a change of either
//! makes the index anew. Both are kept in its locator too, so an index
//! opened reads documents as it was made to.

use super::terms::Shape;
use crate::bytes::{Put, Reader};
use crate::catalog::IndexDesc;
use crate::disk::pager::PageId;
use crate::error::Result;
use crate::json::MAX_NESTING;
use crate::settings::{Kind, OFF, ON, SWITCHES};
use crate::storage::{Rebuild, TableOption, option_value};
use crate::value::{Decimal, Value};

/// The option of whether an index posts paths alone, no values.
pub const PATHS_ONLY_OPTION: &str = "JSON.PATHS_ONLY";
/// The option of the most member names of a path an index posts.
pub const MAX_DEPTH_OPTION: &str = "JSON.MAX_DEPTH";
/// The format of a locator: the format, the tree's meta page, then the
/// shape.
const LOCATOR_VERSION: u8 = 1;

/// The shape of an index made with no options: every path and value.
pub(super) fn whole() -> Shape {
    Shape { paths_only: false, max_depth: MAX_NESTING }
}

/// The options its indexes take (RULES.md D41).
pub(super) fn options() -> Vec<TableOption> {
    let most = i128::try_from(MAX_NESTING).unwrap_or(i128::MAX);
    vec![
        TableOption::new(
            (PATHS_ONLY_OPTION, Kind::Choice(SWITCHES), Value::Char(OFF.into())),
            Rebuild::Yes,
            "ON to post the paths of a document's nodes alone, not its values: smaller, and a filter of a value finds every document with its path.",
        ),
        TableOption::new(
            (MAX_DEPTH_OPTION, Kind::Integer { min: 0, max: most }, Value::Exact(Decimal::new(most, 0))),
            Rebuild::Yes,
            "The most member names of a path posted: a deeper path is found by its first names, and each row found is checked.",
        ),
    ]
}

/// The shape the options of `desc` set.
pub(super) fn shape_of(desc: &IndexDesc) -> Shape {
    let declared = options();
    let paths_only = match option_value(&declared, &desc.options, PATHS_ONLY_OPTION) {
        Some(Value::Char(word)) => word == ON,
        Some(_) | None => false,
    };
    let max_depth = match option_value(&declared, &desc.options, MAX_DEPTH_OPTION) {
        Some(Value::Exact(d)) => d.to_string().parse::<usize>().unwrap_or(MAX_NESTING),
        Some(_) | None => MAX_NESTING,
    };
    Shape { paths_only, max_depth: max_depth.min(MAX_NESTING) }
}

/// The locator of an index whose tree's meta page is `meta`, of `shape`.
pub(super) fn locator(meta: PageId, shape: Shape) -> Vec<u8> {
    let mut out = vec![LOCATOR_VERSION];
    out.put_u64(meta.0);
    out.push(u8::from(shape.paths_only));
    out.put_u32(u32::try_from(shape.max_depth).unwrap_or(u32::MAX));
    out
}

/// The meta page and shape a locator holds.
///
/// # Errors
///
/// SQLCODE -916 if it is not of this format.
pub(super) fn located(locator: &[u8]) -> Result<(PageId, Shape)> {
    let mut reader = Reader::new(locator, "json index locator");
    if reader.u8()? != LOCATOR_VERSION {
        return Err(reader.corrupt("its version is not known"));
    }
    let meta = PageId(reader.u64()?);
    let paths_only = reader.u8()? != 0;
    let max_depth = usize::try_from(reader.u32()?).unwrap_or(MAX_NESTING).min(MAX_NESTING);
    reader.finish()?;
    Ok((meta, Shape { paths_only, max_depth }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] a locator reads back its meta page and shape;
    /// another version is refused.
    #[test]
    fn a_locator_reads_back() {
        let shape = Shape { paths_only: true, max_depth: 3 };
        assert_eq!(located(&locator(PageId(9), shape)).unwrap(), (PageId(9), shape));
        let mut other = locator(PageId(9), shape);
        if let Some(first) = other.first_mut() {
            *first = 2;
        }
        assert!(located(&other).is_err());
    }
}
