//! Unit tests: an extent's head names it whole, and a torn or altered
//! extent is refused (RULES.md D1146, R-TEST-1).

use std::sync::Arc;

use super::{Sink, head_of};
use crate::disk::pager::PageId;
use crate::hash::HashFunction;
use crate::hash::crc::{CRC32_ISCSI, Crc};

fn hash() -> Arc<dyn HashFunction> {
    Arc::new(Crc::new(CRC32_ISCSI).unwrap())
}

/// Verifies: [DESIGN] RULES.md D1146: the head of an extent finished
/// names its rows, pages, header and length; no page is numbered 0; an
/// extent cut short, or with a byte changed past its head, fails.
#[test]
fn an_extent_head_names_it_and_refuses_a_torn_one() {
    let sink = Sink::new(hash()).unwrap();
    let first = sink.next();
    assert_ne!(first, 0, "page 0 reads as none");
    assert_eq!(sink.put(&[1; 100]).unwrap(), PageId(first));
    let second = sink.put(&[2; 200]).unwrap();
    let extent = sink.finish((10, 19, 10), second).unwrap();
    let head = head_of(&*hash(), &extent).unwrap();
    assert_eq!((head.first, head.last, head.rows, head.pages, head.header), (10, 19, 10, 2, second.0));
    assert_eq!(head.length, u64::try_from(extent.len()).unwrap());
    assert!(head_of(&*hash(), &extent[..extent.len() - 1]).is_err(), "torn");
    let mut changed = extent.clone();
    let last = changed.len() - 10;
    changed[last] ^= 1;
    assert!(head_of(&*hash(), &changed).is_err(), "changed");
}
