//! \[DESIGN\] Node records. "For each point i, we store its full precision
//! vector x_i followed by the identities of its ≤ R neighbors. If the
//! degree of a node is smaller than R, we pad with zeros" [DiskANN,
//! NeurIPS 2019 §3.2]. Every record has one fixed width, so node i is at
//! slot i mod n of chunk i / n, and its vector and edges come in one read.
//!
//! A record is a header (flags u8, degree u16, row id u64, version count
//! u32), the vector in its element format, then R u32 neighbours.

use crate::error::{Code, Result, SqlError};

use super::half::{from_half, to_half};

/// The flags, degree, row id and version count before the vector.
pub(super) const HEADER: usize = 15;
/// Bytes of a neighbour's node number.
const EDGE: usize = size_of::<u32>();
/// Bytes of a binary32 coordinate.
const F32: usize = size_of::<f32>();
/// Bytes of a binary16 coordinate.
const F16: usize = size_of::<u16>();
/// The meta page's code of binary32 node vectors.
const FLOAT32_CODE: u8 = 1;
/// The meta page's code of binary16 node vectors.
const FLOAT16_CODE: u8 = 2;
/// Where the degree is.
const DEGREE_AT: usize = 1;
/// Where the row id is.
const ROW_AT: usize = 3;
/// Where the version count is.
const COUNT_AT: usize = 11;
/// The most neighbours a record holds (DISKANN.MAX_DEGREE's greatest).
pub const MOST_DEGREE: u32 = 512;
/// The most dimensions a node's vector has.
pub const MOST_DIMENSIONS: u32 = 65_535;

/// A result of searches: inserted and not since removed as often.
pub(super) const LIVE: u8 = 1;
/// Removed as often as inserted, awaiting consolidation (FreshDiskANN).
pub(super) const TOMBSTONE: u8 = 2;
/// A free slot, its first neighbour the next free slot.
pub(super) const FREE: u8 = 4;
/// Taken out of the row map by a consolidation now running.
pub(super) const FREEING: u8 = 8;
/// Every flag a record may carry.
const FLAGS: u8 = LIVE | TOMBSTONE | FREE | FREEING;

/// \[DESIGN\] How a node's vector is stored. Its code is on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Element {
    /// IEEE binary32, exact.
    Float32,
    /// IEEE binary16, half the bytes, rounded to nearest even.
    Float16,
}

impl Element {
    /// Bytes per coordinate.
    pub fn bytes(self) -> usize {
        match self {
            Element::Float32 => F32,
            Element::Float16 => F16,
        }
    }

    /// Its code in the meta page.
    pub(super) fn code(self) -> u8 {
        match self {
            Element::Float32 => FLOAT32_CODE,
            Element::Float16 => FLOAT16_CODE,
        }
    }

    /// The element of meta code `code`.
    pub(super) fn of_code(code: u8) -> Option<Element> {
        [Element::Float32, Element::Float16].into_iter().find(|e| e.code() == code)
    }
}

/// A decoded record.
#[derive(Debug, Clone, Default, PartialEq)]
pub(super) struct Node {
    pub(super) flags: u8,
    pub(super) row: u64,
    pub(super) count: u32,
    pub(super) vector: Vec<f32>,
    pub(super) neighbours: Vec<u32>,
}

impl Node {
    /// Whether searches return it.
    pub(super) fn live(&self) -> bool {
        self.flags & LIVE != 0
    }

    /// Whether it is a free slot, whose edges are a free list's link.
    pub(super) fn free(&self) -> bool {
        self.flags & FREE != 0
    }
}

/// The fixed widths of an index's records.
#[derive(Debug, Clone, Copy)]
pub(super) struct Shape {
    pub(super) dims: usize,
    pub(super) degree: usize,
    pub(super) element: Element,
    /// Bytes of a stored vector.
    pub(super) vector: usize,
    /// Bytes of a record.
    pub(super) record: usize,
    /// Records in a chunk page.
    pub(super) per_page: usize,
}

impl Shape {
    /// The shape of `dims` coordinates of `element` and `degree` edges, in
    /// pages of `capacity` bytes.
    ///
    /// # Errors
    ///
    /// SQLCODE -900 if a record does not fit a page, naming the bytes it
    /// needs, so a smaller degree, FLOAT16 or bigger pages can be chosen.
    pub(super) fn new(dims: u32, degree: u32, element: Element, capacity: usize) -> Result<Shape> {
        let (dims, degree) = (usize::try_from(dims).unwrap_or(usize::MAX), usize::try_from(degree).unwrap_or(usize::MAX));
        let vector = dims.saturating_mul(element.bytes());
        let record = HEADER.saturating_add(vector).saturating_add(degree.saturating_mul(EDGE));
        let per_page = capacity.checked_div(record).unwrap_or(0);
        if per_page == 0 || dims == 0 || degree == 0 {
            return Err(SqlError::new(
                Code::Unsupported,
                format!(
                    "[EXTENSION] a DiskANN node needs {record} bytes, but a page holds {capacity}: lower DISKANN.MAX_DEGREE, store FLOAT16 node vectors, or use bigger pages"
                ),
            ));
        }
        Ok(Shape { dims, degree, element, vector, record, per_page })
    }

    /// The chunk of node `id`, and its record's offset in the chunk page.
    pub(super) fn place(&self, id: u32) -> (usize, usize) {
        let id = usize::try_from(id).unwrap_or(usize::MAX);
        let chunk = id.checked_div(self.per_page).unwrap_or(0);
        let slot = id.checked_rem(self.per_page).unwrap_or(0);
        (chunk, slot.saturating_mul(self.record))
    }

    /// Bytes of a whole chunk page.
    pub(super) fn page_bytes(&self) -> usize {
        self.per_page.saturating_mul(self.record)
    }

    /// `vector` in this shape's element format.
    pub(super) fn vector_bytes(&self, vector: &[f32], out: &mut Vec<u8>) {
        out.clear();
        match self.element {
            Element::Float32 => vector.iter().for_each(|v| out.extend_from_slice(&v.to_le_bytes())),
            Element::Float16 => vector.iter().for_each(|v| out.extend_from_slice(&to_half(*v).to_le_bytes())),
        }
    }

    /// The stored bytes of the record at `bytes`' vector.
    pub(super) fn stored_vector<'a>(&self, record: &'a [u8]) -> Result<&'a [u8]> {
        record.get(HEADER..HEADER.saturating_add(self.vector)).ok_or_else(short)
    }

    /// Decodes the record at the start of `record` into `node`, reusing
    /// its buffers.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the record is short, its flags unknown or its
    /// degree past R.
    pub(super) fn decode(&self, record: &[u8], node: &mut Node) -> Result<()> {
        let header = record.get(..HEADER).ok_or_else(short)?;
        let (flags, degree) = (byte(header, 0), u16_at(header, DEGREE_AT));
        if flags & !FLAGS != 0 || usize::from(degree) > self.degree {
            return Err(corrupt("a node's flags or degree are invalid"));
        }
        node.flags = flags;
        node.row = u64_at(header, ROW_AT);
        node.count = u32_at(header, COUNT_AT);
        self.decode_vector(self.stored_vector(record)?, &mut node.vector);
        let edges = HEADER.saturating_add(self.vector);
        let end = edges.saturating_add(usize::from(degree).saturating_mul(EDGE));
        let list = record.get(edges..end).ok_or_else(short)?;
        node.neighbours.clear();
        node.neighbours.extend(list.as_chunks::<EDGE>().0.iter().map(|c| u32::from_le_bytes(*c)));
        Ok(())
    }

    /// The coordinates of stored vector bytes.
    pub(super) fn decode_vector(&self, bytes: &[u8], out: &mut Vec<f32>) {
        out.clear();
        match self.element {
            Element::Float32 => out.extend(bytes.as_chunks::<F32>().0.iter().map(|c| f32::from_le_bytes(*c))),
            Element::Float16 => out.extend(bytes.as_chunks::<F16>().0.iter().map(|c| from_half(u16::from_le_bytes(*c)))),
        }
    }

    /// Writes `node` over the record at the start of `record`, its unused
    /// edges zero.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if the slot is short; -910 if the node breaks the shape.
    pub(super) fn encode(&self, node: &Node, record: &mut [u8]) -> Result<()> {
        let slot = record.get_mut(..self.record).ok_or_else(short)?;
        if node.neighbours.len() > self.degree || node.vector.len() != self.dims {
            return Err(SqlError::new(Code::Api, "[DESIGN] a DiskANN node has too many edges or the wrong dimensions"));
        }
        slot.fill(0);
        let degree = u16::try_from(node.neighbours.len()).unwrap_or(u16::MAX).to_le_bytes();
        let mut head = vec![node.flags];
        head.extend_from_slice(&degree);
        head.extend_from_slice(&node.row.to_le_bytes());
        head.extend_from_slice(&node.count.to_le_bytes());
        let mut stored = Vec::with_capacity(self.vector);
        self.vector_bytes(&node.vector, &mut stored);
        head.extend_from_slice(&stored);
        node.neighbours.iter().for_each(|n| head.extend_from_slice(&n.to_le_bytes()));
        slot.get_mut(..head.len()).ok_or_else(short)?.copy_from_slice(&head);
        Ok(())
    }
}

fn byte(bytes: &[u8], at: usize) -> u8 {
    bytes.get(at).copied().unwrap_or(0)
}

fn u16_at(bytes: &[u8], at: usize) -> u16 {
    let mut b = [0_u8; 2];
    b.iter_mut().zip(bytes.iter().skip(at)).for_each(|(d, s)| *d = *s);
    u16::from_le_bytes(b)
}

pub(super) fn u32_at(bytes: &[u8], at: usize) -> u32 {
    let mut b = [0_u8; 4];
    b.iter_mut().zip(bytes.iter().skip(at)).for_each(|(d, s)| *d = *s);
    u32::from_le_bytes(b)
}

pub(super) fn u64_at(bytes: &[u8], at: usize) -> u64 {
    let mut b = [0_u8; 8];
    b.iter_mut().zip(bytes.iter().skip(at)).for_each(|(d, s)| *d = *s);
    u64::from_le_bytes(b)
}

fn short() -> SqlError {
    corrupt("a node record is short")
}

pub(super) fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] a DiskANN index is malformed: {why}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] [DiskANN, NeurIPS 2019 §3.2] Records are fixed
    /// width, so node i's offset is arithmetic; a record decodes to what
    /// was encoded, short lists padded; a record too wide for a page is
    /// refused with SQLCODE -900, naming the bytes it needs.
    #[test]
    fn records_are_fixed_width_and_round_trip() {
        let shape = Shape::new(3, 4, Element::Float32, 100).unwrap();
        assert_eq!((shape.record, shape.per_page), (HEADER + 12 + 16, 2));
        assert_eq!(shape.place(5), (2, shape.record));
        let node = Node { flags: LIVE, row: 9, count: 2, vector: vec![1.0_f32, -2.5_f32, 3.25_f32], neighbours: vec![7, 3] };
        let mut page = vec![0xAA_u8; shape.page_bytes()];
        shape.encode(&node, &mut page).unwrap();
        let mut back = Node::default();
        shape.decode(&page, &mut back).unwrap();
        assert_eq!(back, node);
        let half = Shape::new(3, 4, Element::Float16, 100).unwrap();
        assert_eq!(half.vector, 6);
        let wide = Shape::new(1000, 64, Element::Float32, 4096).unwrap_err();
        assert_eq!(wide.code, Code::Unsupported);
        assert!(wide.msg.contains("4271 bytes"), "{}", wide.msg);
        page[0] = 0x80;
        assert_eq!(shape.decode(&page, &mut back).unwrap_err().code, Code::Corrupt);
    }
}
