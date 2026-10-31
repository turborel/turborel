//! Unit tests of a pager file's encodings: a scratch file's, as they
//! change (RULES.md R-TEST-1, R-TEST-2). Split from pager/live.rs (RULES.md
//! R-SIZE-2).

use std::path::Path;
use std::sync::Arc;

use super::super::{PageId, Pager};
use super::{Encoded, MAX_PIPELINES};
use crate::disk::encoding::{Encoding, EncodingId, LivePipeline, Pipeline};
use crate::disk::io::{FileSystem, OpenMode};
use crate::disk::memory::MemoryFileSystem;
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;
use crate::hash::crc::{CRC32_ISCSI, Crc};

/// An encryption, near enough: flips every byte, then appends `pad`
/// bytes, checked as it decodes.
struct Flip {
    id: u32,
    pad: usize,
}

impl Encoding for Flip {
    fn id(&self) -> EncodingId {
        EncodingId(self.id)
    }
    fn name(&self) -> &'static str {
        "flip"
    }
    fn overhead(&self, _: &str) -> usize {
        self.pad
    }
    fn confidential(&self) -> bool {
        true
    }
    fn encode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        out.extend(input.iter().map(|b| !b));
        out.resize(out.len().saturating_add(self.pad), 0xEE);
        Ok(())
    }
    fn decode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let body = input.len().checked_sub(self.pad).map(|n| input.split_at(n));
        let (body, _) = body.ok_or_else(|| SqlError::new(Code::Corrupt, "not flipped"))?;
        out.extend(body.iter().map(|b| !b));
        Ok(())
    }
}

/// A pipeline of one `Flip`.
fn flip(id: u32, pad: usize) -> Pipeline {
    Pipeline::new(vec![Arc::new(Flip { id, pad })]).unwrap()
}

/// The bytes of the file at `path` of `fs`.
fn bytes(fs: &MemoryFileSystem, path: &Path) -> Vec<u8> {
    let file = fs.open(path, OpenMode::ReadOnly).unwrap();
    let mut all = vec![0_u8; usize::try_from(file.size().unwrap()).unwrap()];
    file.read_exact_at(&mut all, 0).unwrap();
    all
}

fn holds(all: &[u8], text: &[u8]) -> bool {
    all.windows(text.len()).any(|w| w == text)
}

/// Verifies: [DESIGN] RULES.md D48: a scratch file's pages pass through
/// the pipeline its source holds as each is written, a page written
/// before a change kept as it was, one written again after it encoded
/// anew; a pipeline growing pages past its page goes to overflow pages;
/// one of the encodings of one taken before replaces it; every page reads
/// back through the one that wrote it.
#[test]
fn a_scratch_files_pages_take_the_pipeline_held_as_each_is_written() {
    let (fs, path) = (MemoryFileSystem::new(), Path::new("/s"));
    let hash: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let live = Arc::new(LivePipeline::default());
    let pager = Pager::create_scratch(&fs, path, hash, &live, 4096).unwrap();
    let ids: Vec<PageId> = (0..4_u8).map(|_| pager.allocate(1)[0].start).collect();
    pager.write(ids[0], b"plain page one").unwrap();
    pager.write(ids[3], b"plain page kept").unwrap();
    live.set(flip(1001, 0));
    pager.write(ids[1], b"plain page two").unwrap();
    live.set(flip(1002, 5000));
    pager.write(ids[2], b"plain page three").unwrap();
    live.set(flip(1001, 0));
    pager.write(ids[0], b"plain page four").unwrap();
    let all = bytes(&fs, path);
    assert!(holds(&all, b"plain page kept"), "written before the change");
    for text in [&b"plain page one"[..], b"plain page two", b"plain page three", b"plain page four"] {
        assert!(!holds(&all, text), "{}", String::from_utf8_lossy(text));
    }
    assert!(pager.counts().overflow_pages_written > 0, "grown past its page");
    let read = |id: PageId| {
        let mut content = Vec::new();
        pager.read(id, &mut content).unwrap();
        content
    };
    let expected: [&[u8]; 4] = [b"plain page four", b"plain page two", b"plain page three", b"plain page kept"];
    assert_eq!(ids.iter().map(|&id| read(id)).collect::<Vec<_>>(), expected.map(<[u8]>::to_vec).to_vec());
}

/// Verifies: [DESIGN] RULES.md D48, robustness: a scratch file's content
/// naming no pipeline it took, or empty, is SQLCODE -916; past
/// `MAX_PIPELINES` of different encodings a write is refused (SQLCODE
/// -910), and one through a pipeline taken before still goes; its room
/// for growth is its tag's and its first pipeline's; a durable file's
/// pipeline is its own.
#[test]
fn a_scratch_files_pipelines_are_bounded_and_named() {
    let live = Arc::new(LivePipeline::default());
    live.set(flip(1001, 7));
    let (encoded, first) = Encoded::live(&live);
    assert_eq!((encoded.overhead(), encoded.is_empty(), first.ids()), (8_usize, false, vec![EncodingId(1001)]));
    for bad in [&[9_u8, 1, 2][..], &[]] {
        assert_eq!(encoded.decode(0, bad).err().map(|e| e.code), Some(Code::Corrupt));
    }
    let id = |n: usize| 2000 + u32::try_from(n).unwrap();
    for n in 1..MAX_PIPELINES {
        live.set(flip(id(n), 0));
        let written = encoded.encode(5, b"x").unwrap().into_owned();
        assert_eq!(&*encoded.decode(5, &written).unwrap(), b"x");
    }
    live.set(flip(id(MAX_PIPELINES), 0));
    assert_eq!(encoded.encode(5, b"x").err().map(|e| e.code), Some(Code::Api));
    live.set(flip(id(1), 0));
    assert_eq!(encoded.encode(5, b"x").unwrap()[0], 1_u8, "the tag of the one taken before");
    let fixed = Encoded::Fixed(flip(3, 2));
    assert_eq!(
        (fixed.overhead(), fixed.is_empty(), &*fixed.decode(0, &fixed.encode(0, b"y").unwrap()).unwrap()),
        (2_usize, false, &b"y"[..])
    );
    assert!(Encoded::Fixed(Pipeline::none()).is_empty());
}
