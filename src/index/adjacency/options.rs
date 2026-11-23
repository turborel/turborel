//! \[EXTENSION\] An adjacency index's options (RULES.md D41) and its
//! layout: how many of its key's first columns are the edge's source,
//! ADJACENCY.SOURCE_COLUMNS; how many of its last are properties kept
//! for covering reads, ADJACENCY.INCLUDE; the columns between are the
//! destination. ADJACENCY.DIRECTION keeps the edges by source (OUT), by
//! destination (IN) or both, and ADJACENCY.CSR_BYTES bounds the compacted
//! adjacency (`csr.rs`). The first three shape the entries, so a change
//! makes the index anew, and are kept in its locator, so an index opened
//! reads its entries as it wrote them; the last changes in place.
//!
//! ```text
//! locator: version u8, source u32, include u32, direction u8, then the
//!   key counts' locator (`index/stats.rs`)
//! ```

use std::ops::Range;

use crate::bytes::{Put, Reader};
use crate::catalog::IndexDesc;
use crate::error::{Code, Result, SqlError};
use crate::graph::adjacency::Direction;
use crate::settings::Kind;
use crate::storage::{Rebuild, TableOption, option_value};
use crate::value::{DataType, Decimal, Value};

/// The option of how many of the key's first columns are the source.
pub const SOURCE_COLUMNS_OPTION: &str = "ADJACENCY.SOURCE_COLUMNS";
/// The option of how many of the key's last columns are included properties.
pub const INCLUDE_OPTION: &str = "ADJACENCY.INCLUDE";
/// The option of which ways the edges are kept.
pub const DIRECTION_OPTION: &str = "ADJACENCY.DIRECTION";
/// The option of the most bytes the compacted adjacency holds.
pub const CSR_BYTES_OPTION: &str = "ADJACENCY.CSR_BYTES";
/// Edges kept both ways, the default, so a path is followed either way.
pub const BOTH: &str = "BOTH";
/// Edges kept by their source alone.
pub const OUT: &str = "OUT";
/// Edges kept by their destination alone.
pub const IN: &str = "IN";
/// The words DIRECTION takes.
const DIRECTIONS: &[&str] = &[BOTH, OUT, IN];
/// \[DESIGN\] The default of CSR_BYTES, 256 MiB: a quarter of the default
/// memory budget (`memory::DEFAULT_BUDGET`), room for some ten million
/// edges of integer endpoints.
pub const DEFAULT_CSR_BYTES: usize = 1 << 28;
/// The most columns a part of the key may have, as the locator keeps them.
const MOST_COLUMNS: i128 = 1 << 16;
/// The format of a locator.
const LOCATOR_VERSION: u8 = 1;

/// Which ways an index keeps its edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kept {
    /// By source and by destination.
    Both,
    /// By source alone.
    Out,
    /// By destination alone.
    In,
}

impl Kept {
    /// Whether edges are kept to be followed in `direction`.
    pub fn keeps(self, direction: Direction) -> bool {
        match (self, direction) {
            (Kept::Both, Direction::Out | Direction::In) | (Kept::Out, Direction::Out) | (Kept::In, Direction::In) => true,
            (Kept::Out, Direction::In) | (Kept::In, Direction::Out) => false,
        }
    }

    /// The directions it keeps, `Out` first, as their entries sort.
    pub fn directions(self) -> &'static [Direction] {
        match self {
            Kept::Both => &[Direction::Out, Direction::In],
            Kept::Out => &[Direction::Out],
            Kept::In => &[Direction::In],
        }
    }

    /// Its byte in a locator.
    fn byte(self) -> u8 {
        match self {
            Kept::Both => 0,
            Kept::Out => 1,
            Kept::In => 2,
        }
    }
}

/// How an index's key is laid out, fixed for its life.
#[derive(Debug, Clone, PartialEq)]
pub struct Layout {
    /// The types of the key's columns, in order.
    pub types: Vec<DataType>,
    /// How many first columns are the source.
    pub source: usize,
    /// How many last columns are included properties.
    pub include: usize,
    /// Which ways edges are kept.
    pub kept: Kept,
}

/// The error of a layout the key cannot have.
fn refused(detail: &str) -> SqlError {
    SqlError::new(Code::Unsupported, format!("[EXTENSION] the adjacency index method {detail}"))
}

impl Layout {
    /// The layout of a key of `types`, checked.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if the source or the destination has no column.
    pub fn new(types: &[DataType], (source, include): (usize, usize), kept: Kept) -> Result<Layout> {
        let ends = types.len().saturating_sub(include);
        if source == 0 || ends <= source {
            return Err(refused(&format!(
                "keys an edge by its source's columns, then its destination's, then those included: a key of {} column(s), {source} of the source and {include} included, has no source or no destination",
                types.len()
            )));
        }
        Ok(Layout { types: types.to_vec(), source, include, kept })
    }

    /// Where the destination's columns are in the key.
    fn destination(&self) -> Range<usize> {
        self.source..self.types.len().saturating_sub(self.include)
    }

    /// The types of the source's columns.
    pub fn source_types(&self) -> &[DataType] {
        self.types.get(..self.source).unwrap_or_default()
    }

    /// The types of the destination's columns.
    pub fn destination_types(&self) -> &[DataType] {
        self.types.get(self.destination()).unwrap_or_default()
    }

    /// The types of the included columns.
    pub fn included_types(&self) -> &[DataType] {
        self.types.get(self.destination().end..).unwrap_or_default()
    }

    /// The types of the vertex followed from, then of the vertex reached,
    /// in `direction`.
    pub fn ends(&self, direction: Direction) -> (&[DataType], &[DataType]) {
        match direction {
            Direction::Out => (self.source_types(), self.destination_types()),
            Direction::In => (self.destination_types(), self.source_types()),
        }
    }

    /// The columns of both endpoints.
    pub fn endpoints(&self) -> usize {
        self.destination().end
    }
}

/// The options its indexes take (RULES.md D41).
pub fn options() -> Vec<TableOption> {
    vec![
        TableOption::new(
            (SOURCE_COLUMNS_OPTION, Kind::Integer { min: 1, max: MOST_COLUMNS }, Value::Exact(Decimal::new(1, 0))),
            Rebuild::Yes,
            "How many of the key's first columns are the edge's source; the destination's follow.",
        ),
        TableOption::new(
            (INCLUDE_OPTION, Kind::Integer { min: 0, max: MOST_COLUMNS }, Value::Exact(Decimal::new(0, 0))),
            Rebuild::Yes,
            "How many of the key's last columns are edge properties kept in each entry for covering reads; each is NOT NULL.",
        ),
        TableOption::new(
            (DIRECTION_OPTION, Kind::Choice(DIRECTIONS), Value::Char(BOTH.into())),
            Rebuild::Yes,
            "BOTH, OUT or IN: whether edges are kept by source, by destination, or both, to be followed either way.",
        ),
        TableOption::new(
            (
                CSR_BYTES_OPTION,
                Kind::Integer { min: 0, max: i128::try_from(usize::MAX).unwrap_or(i128::MAX) },
                bytes_value(DEFAULT_CSR_BYTES),
            ),
            Rebuild::No,
            "The most bytes of the memory budget its compacted adjacency holds; 0 for none, the edges then read from the index alone.",
        ),
    ]
}

/// A count of bytes as an option's value.
fn bytes_value(n: usize) -> Value {
    Value::Exact(Decimal::new(i128::try_from(n).unwrap_or(i128::MAX), 0))
}

/// The whole number an option holds, of those declared, in `desc`.
fn number(desc: &IndexDesc, name: &str) -> Option<usize> {
    match option_value(&options(), &desc.options, name) {
        Some(Value::Exact(d)) => d.to_string().parse::<usize>().ok(),
        Some(_) | None => None,
    }
}

/// The layout `desc`'s options give a key of `types`.
///
/// # Errors
///
/// As `Layout::new`.
pub fn layout_of(types: &[DataType], desc: &IndexDesc) -> Result<Layout> {
    let declared = options();
    let word = match option_value(&declared, &desc.options, DIRECTION_OPTION) {
        Some(Value::Char(word)) => word.as_str(),
        Some(_) | None => BOTH,
    };
    let kept = if word == OUT {
        Kept::Out
    } else if word == IN {
        Kept::In
    } else {
        Kept::Both
    };
    let (source, include) = (number(desc, SOURCE_COLUMNS_OPTION).unwrap_or(1), number(desc, INCLUDE_OPTION).unwrap_or(0));
    Layout::new(types, (source, include), kept)
}

/// \[GS §16 R16.1\] Of adjacency index `desc`, for the planner: the places
/// of its key's source columns and destination columns, and whether it
/// keeps reverse entries, by which an equality of the destination's is a
/// vertex's list too.
pub fn planned_ends(desc: &IndexDesc) -> (Range<usize>, Range<usize>, bool) {
    let source = number(desc, SOURCE_COLUMNS_OPTION).unwrap_or(1);
    let include = number(desc, INCLUDE_OPTION).unwrap_or(0);
    let endpoints = desc.columns.len().saturating_sub(include);
    let reverse = match option_value(&options(), &desc.options, DIRECTION_OPTION) {
        Some(Value::Char(word)) => word != OUT,
        Some(_) | None => true,
    };
    (0..source.min(endpoints), source.min(endpoints)..endpoints, reverse)
}

/// The most bytes `desc` lets its compacted adjacency hold.
pub fn csr_bytes_of(desc: &IndexDesc) -> usize {
    number(desc, CSR_BYTES_OPTION).unwrap_or(DEFAULT_CSR_BYTES)
}

/// The locator of an index of `layout` whose key counts' locator, its
/// tree's meta page first, is `counts`.
pub fn locator(layout: &Layout, counts: &[u8]) -> Vec<u8> {
    let mut out = vec![LOCATOR_VERSION];
    out.put_u32(u32::try_from(layout.source).unwrap_or(u32::MAX));
    out.put_u32(u32::try_from(layout.include).unwrap_or(u32::MAX));
    out.put_u8(layout.kept.byte());
    out.extend_from_slice(counts);
    out
}

/// The layout of a key of `types` a locator holds, and the key counts'
/// locator after it.
///
/// # Errors
///
/// SQLCODE -916 if it is not of this format; as `Layout::new`.
pub fn located<'a>(locator: &'a [u8], types: &[DataType]) -> Result<(Layout, &'a [u8])> {
    let header = 1 + 4 + 4 + 1;
    let (head, counts) =
        locator.split_at_checked(header).ok_or_else(|| Reader::new(locator, "adjacency index locator").corrupt("short"))?;
    let mut reader = Reader::new(head, "adjacency index locator");
    if reader.u8()? != LOCATOR_VERSION {
        return Err(reader.corrupt("its version is not known"));
    }
    let source = usize::try_from(reader.u32()?).unwrap_or(usize::MAX);
    let include = usize::try_from(reader.u32()?).unwrap_or(usize::MAX);
    let kept = match reader.u8()? {
        0 => Kept::Both,
        1 => Kept::Out,
        2 => Kept::In,
        _ => return Err(reader.corrupt("its direction is not known")),
    };
    reader.finish()?;
    Ok((Layout::new(types, (source, include), kept)?, counts))
}
