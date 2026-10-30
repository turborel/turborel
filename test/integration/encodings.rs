//! The encoding pipeline (RULES.md D30) in the block manager and the pager,
//! with two test encodings standing in for the plug-ins (RULES.md D48): a
//! run-length compression that stores input raw when compressing would not
//! shrink it, and a scrambling bound to its context, standing in for an
//! authenticated encryption.

use std::path::Path;
use std::sync::Arc;

use turborel::disk::block::{BlockFile, BlockOptions};
use turborel::disk::encoding::{Encoding, EncodingId, MAX_ENCODINGS, Pipeline};
use turborel::disk::io::{Durability, OsFileSystem};
use turborel::disk::pager::{PageId, Pager};
use turborel::error::{Code, Result, SqlError};

use crate::common::{RunLength, Scramble, Scratch, crc32c};

/// An encoding whose decoding always fails.
struct Broken;

impl Encoding for Broken {
    fn id(&self) -> EncodingId {
        EncodingId(1003)
    }
    fn name(&self) -> &'static str {
        "test broken"
    }
    fn overhead(&self, _: &str) -> usize {
        0
    }
    fn encode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        out.extend_from_slice(input);
        Ok(())
    }
    fn decode(&self, _: u64, _: &[u8], _: &mut Vec<u8>) -> Result<()> {
        Err(SqlError::new(Code::Corrupt, "cannot decode"))
    }
}

fn available() -> Vec<Arc<dyn Encoding>> {
    vec![Arc::new(RunLength), Arc::new(Scramble), Arc::new(Broken)]
}

/// Compress, then scramble.
fn pipeline() -> Pipeline {
    Pipeline::new(vec![Arc::new(RunLength), Arc::new(Scramble)]).unwrap()
}

/// Whether `needle` occurs in the file at `path`.
fn file_contains(path: &Path, needle: &[u8]) -> bool {
    std::fs::read(path).unwrap().windows(needle.len()).any(|w| w == needle)
}

/// Verifies: [DESIGN] RULES.md D30, encoding pipeline, block manager: each
/// payload passes through the pipeline, compressed then scrambled, so the
/// file holds neither the payload nor its plain bytes; reading by id and by
/// cursor decodes it; the counters show the bytes stored, fewer for what
/// compressed; the file records its pipeline, so it reopens with the
/// encodings given and is refused (SQLCODE -910) without one.
#[test]
fn block_payloads_pass_through_the_pipeline() {
    let scratch = Scratch::new("encodings-blocks");
    let path = scratch.path("b");
    let file = BlockFile::create(&OsFileSystem, &path, crc32c(), pipeline(), BlockOptions::default()).unwrap();
    let repetitive = vec![b'a'; 10_000];
    let varied: Vec<u8> = b"a sentence of plain words, kept from the file".to_vec();
    let ids = [file.append(&repetitive).unwrap(), file.append(&varied).unwrap()];
    assert_eq!((file.read(ids[0]).unwrap(), file.read(ids[1]).unwrap()), (repetitive.clone(), varied.clone()));
    let counts = file.counts();
    assert_eq!(counts.appended_bytes, 10_000 + 45);
    assert!(counts.stored_bytes < 200, "compressed: {} bytes stored", counts.stored_bytes);
    file.sync(Durability::Data).unwrap();
    drop(file);
    assert!(!file_contains(&path, b"plain words"), "scrambled on disk");
    let (file, _) = BlockFile::open(&OsFileSystem, &path, &[crc32c()], &available(), BlockOptions::default()).unwrap();
    let mut cursor = file.cursor(file.first());
    let mut payload = Vec::new();
    assert_eq!(cursor.next(&mut payload).unwrap(), Some(ids[0]));
    assert_eq!(payload, repetitive);
    drop(file);
    let missing = BlockFile::open(&OsFileSystem, &path, &[crc32c()], &[Arc::new(RunLength)], BlockOptions::default());
    assert_eq!(missing.err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [DESIGN] RULES.md D30, encoding pipeline, pager: each page's
/// whole content passes through the pipeline with its page number as
/// context; content that compresses needs no overflow pages where the
/// plain content would; content that does not compress is stored raw and
/// overflows; both read back decoded, also after reopening; the file holds
/// none of the plain bytes.
#[test]
fn page_contents_pass_through_the_pipeline() {
    let scratch = Scratch::new("encodings-pages");
    let path = scratch.path("p");
    let pager = Pager::create(&OsFileSystem, &path, crc32c(), pipeline(), 4096).unwrap();
    pager.allocate(2);
    let repetitive = vec![b'z'; 100_000];
    pager.write(PageId(0), &repetitive).unwrap();
    assert_eq!(pager.page_count(), 2, "100 KB compressed to fit its page: no overflow");
    let varied: Vec<u8> = (0..10_000_u32).map(|i| u8::try_from((i * 37 + i.div_euclid(251)) % 256).unwrap()).collect();
    pager.write(PageId(1), &varied).unwrap();
    assert!(pager.page_count() > 2, "stored raw: it overflowed");
    let mut content = Vec::new();
    pager.read(PageId(0), &mut content).unwrap();
    assert_eq!(content, repetitive);
    pager.read(PageId(1), &mut content).unwrap();
    assert_eq!(content, varied);
    let counts = pager.counts();
    assert!(counts.stored_bytes < counts.content_bytes, "{counts:?}");
    pager.write(PageId(0), b"plain words for page zero").unwrap();
    pager.sync(Durability::Data).unwrap();
    drop(pager);
    assert!(!file_contains(&path, b"plain words"), "scrambled on disk");
    let (pager, _) = Pager::open(&OsFileSystem, &path, &[crc32c()], &available()).unwrap();
    pager.read(PageId(0), &mut content).unwrap();
    assert_eq!(content, b"plain words for page zero");
    drop(pager);
    assert_eq!(Pager::open(&OsFileSystem, &path, &[crc32c()], &[]).err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [DESIGN] RULES.md D30, encoding pipeline: an encoding's
/// failure to decode is the read's failure, with its SQLCODE, in both
/// components; a pipeline longer than `MAX_ENCODINGS` cannot be made.
#[test]
fn an_encodings_failure_fails_the_read() {
    let scratch = Scratch::new("encodings-broken");
    let broken = Pipeline::new(vec![Arc::new(Broken)]).unwrap();
    let file = BlockFile::create(&OsFileSystem, &scratch.path("b"), crc32c(), broken.clone(), BlockOptions::default()).unwrap();
    let id = file.append(b"x").unwrap();
    assert_eq!(file.read(id).err().map(|e| e.code), Some(Code::Corrupt));
    let pager = Pager::create(&OsFileSystem, &scratch.path("p"), crc32c(), broken, 4096).unwrap();
    pager.allocate(1);
    pager.write(PageId(0), b"x").unwrap();
    assert_eq!(pager.read(PageId(0), &mut Vec::new()).err().map(|e| e.code), Some(Code::Corrupt));
    let too_many: Vec<Arc<dyn Encoding>> = (0..=MAX_ENCODINGS).map(|_| Arc::new(RunLength) as Arc<dyn Encoding>).collect();
    assert_eq!(Pipeline::new(too_many).err().map(|e| e.code), Some(Code::Api));
}
