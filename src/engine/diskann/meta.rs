//! \[DESIGN\] The meta page: what reopens an index. Its layout (dimensions,
//! degree bound R, element format), the build seed, the medoid search
//! starts from [DiskANN, NeurIPS 2019 §2.3], the node counter and free
//! list, the live and dead counts that make consolidation due, and the
//! heads of the node and code chunk directories, the codebook chain and
//! the row map. Format versions are refused, never guessed.

use crate::bytes::{Put, Reader};
use crate::disk::pager::PageId;
use crate::error::Result;

use super::node::Element;

/// The tag of a DiskANN meta page.
const TAG: u8 = 78;
/// The meta page's format.
const VERSION: u8 = 1;
/// No node: an empty free list, or no medoid yet.
pub(super) const NO_NODE: u32 = u32::MAX;

/// What the meta page holds.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Meta {
    pub(super) dims: u32,
    pub(super) degree: u32,
    pub(super) element: Element,
    pub(super) seed: u64,
    pub(super) medoid: u32,
    /// Node numbers below it have been allocated.
    pub(super) next: u32,
    /// The first free slot, or `NO_NODE`.
    pub(super) free: u32,
    pub(super) live: u64,
    /// Tombstoned or unfinished nodes, which consolidation frees.
    pub(super) dead: u64,
    pub(super) nodes: PageId,
    pub(super) codes: PageId,
    /// The codebook chain's first page, `NO_PAGE` until trained.
    pub(super) book: u64,
    /// Bytes of a node's code, 0 until trained.
    pub(super) code_width: u32,
    pub(super) rowmap: PageId,
}

impl Meta {
    /// Its bytes.
    pub(super) fn bytes(&self) -> Vec<u8> {
        let mut b = vec![TAG, VERSION];
        b.put_u32(self.dims);
        b.put_u32(self.degree);
        b.put_u8(self.element.code());
        b.put_u64(self.seed);
        b.put_u32(self.medoid);
        b.put_u32(self.next);
        b.put_u32(self.free);
        b.put_u64(self.live);
        b.put_u64(self.dead);
        b.put_u64(self.nodes.0);
        b.put_u64(self.codes.0);
        b.put_u64(self.book);
        b.put_u32(self.code_width);
        b.put_u64(self.rowmap.0);
        b
    }

    /// The meta page `bytes`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 for another tag, format or element, or a short page.
    pub(super) fn parse(bytes: &[u8]) -> Result<Meta> {
        let mut r = Reader::new(bytes, "DiskANN meta page");
        if r.u8()? != TAG || r.u8()? != VERSION {
            return Err(r.corrupt("its tag or format version is unsupported"));
        }
        let (dims, degree) = (r.u32()?, r.u32()?);
        let element = Element::of_code(r.u8()?).ok_or_else(|| r.corrupt("its element format is unknown"))?;
        let meta = Meta {
            dims,
            degree,
            element,
            seed: r.u64()?,
            medoid: r.u32()?,
            next: r.u32()?,
            free: r.u32()?,
            live: r.u64()?,
            dead: r.u64()?,
            nodes: PageId(r.u64()?),
            codes: PageId(r.u64()?),
            book: r.u64()?,
            code_width: r.u32()?,
            rowmap: PageId(r.u64()?),
        };
        r.finish()?;
        Ok(meta)
    }
}
