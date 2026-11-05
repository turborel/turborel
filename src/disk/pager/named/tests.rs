//! Unit tests of what a durable pager file's pages name: their encodings
//! and their checksum's key, as both change (RULES.md R-TEST-1, R-TEST-2).

use std::path::Path;
use std::sync::Arc;

use super::super::{PageId, Pager};
use crate::disk::encoding::{Encoding, EncodingId, LivePipeline, Pipeline};
use crate::disk::io::Durability;
use crate::disk::memory::MemoryFileSystem;
use crate::error::{Code, Result, SqlError};
use crate::hash::crc::{CRC32_ISCSI, Crc};
use crate::hash::{Digest, HashFunction, HashId, MAX_KEY_NAME, key_field, read_key_field, verifier_of};

/// An encoding, near enough: adds `id` to every byte.
pub(in crate::disk::pager) struct Shift(pub(in crate::disk::pager) u32);

impl Encoding for Shift {
    fn id(&self) -> EncodingId {
        EncodingId(self.0)
    }
    fn name(&self) -> &'static str {
        "shift"
    }
    fn overhead(&self, _: &str) -> usize {
        0
    }
    fn encode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        out.extend(input.iter().map(|b| b.wrapping_add(u8::try_from(self.0).unwrap_or(1))));
        Ok(())
    }
    fn decode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        out.extend(input.iter().map(|b| b.wrapping_sub(u8::try_from(self.0).unwrap_or(1))));
        Ok(())
    }
}

/// A pipeline of one `Shift`.
pub(in crate::disk::pager) fn shift(id: u32) -> Pipeline {
    Pipeline::new(vec![Arc::new(Shift(id))]).unwrap()
}

/// A keyed hash function, near enough: CRC-32 of the key's name and the
/// bytes; keyed by any name beginning with `k`, or by those of a keyring
/// given.
pub(in crate::disk::pager) struct Keyed {
    name: String,
    held: Option<Vec<String>>,
    crc: Crc,
}

impl Keyed {
    pub(in crate::disk::pager) fn by(name: &str) -> Arc<dyn HashFunction> {
        Arc::new(Keyed { name: name.to_owned(), held: None, crc: Crc::new(CRC32_ISCSI).unwrap() })
    }

    /// Keyed by `name`, of a keyring holding the keys `held` alone.
    pub(in crate::disk::pager) fn holding(name: &str, held: &[&str]) -> Arc<dyn HashFunction> {
        let held = Some(held.iter().map(|&k| k.to_owned()).collect());
        Arc::new(Keyed { name: name.to_owned(), held, crc: Crc::new(CRC32_ISCSI).unwrap() })
    }

    /// Whether the keyring holds `name`.
    fn holds(&self, name: &str) -> bool {
        self.held.as_ref().map_or_else(|| name.starts_with('k'), |held| held.iter().any(|k| k == name))
    }
}

impl HashFunction for Keyed {
    fn id(&self) -> HashId {
        HashId(1000)
    }
    fn name(&self) -> &'static str {
        "keyed"
    }
    fn digest_len(&self) -> usize {
        4
    }
    fn hash_parts(&self, parts: &[&[u8]]) -> Digest {
        let mut all = self.name.as_bytes().to_vec();
        for part in parts {
            all.extend_from_slice(part);
        }
        self.crc.hash(&all)
    }
    fn keyed(&self) -> bool {
        true
    }
    fn key_name(&self) -> Option<&str> {
        Some(&self.name)
    }
    fn keyed_by(&self, name: &str) -> Result<Arc<dyn HashFunction>> {
        if !self.holds(name) {
            return Err(SqlError::new(Code::Api, format!("no key {name}")));
        }
        Ok(Arc::new(Keyed { name: name.to_owned(), held: self.held.clone(), crc: Crc::new(CRC32_ISCSI)? }))
    }
}

/// The content of page `id` of `pager`, or the error's code.
fn content(pager: &Pager, id: u64) -> std::result::Result<Vec<u8>, Code> {
    let mut out = Vec::new();
    pager.read(PageId(id), &mut out).map(|()| out).map_err(|e| e.code)
}

/// Verifies: [DESIGN] RULES.md D30, D48: a page names the pipeline it was
/// written through: pages written before and after the file's pipeline
/// changes read back as written, a page written again takes the new one,
/// and the file reopened reads each page by what it names, refusing one
/// naming an encoding it was not opened with (SQLCODE -910), the others
/// read still; a scratch file's pipeline is its source's, not set.
#[test]
fn pages_name_the_pipeline_they_were_written_through() {
    let (fs, path) = (MemoryFileSystem::new(), Path::new("/named"));
    let crc: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    let pager = Pager::create(&fs, path, Arc::clone(&crc), Pipeline::none(), 4096).unwrap();
    pager.allocate(3);
    pager.write(PageId(0), b"first, plain").unwrap();
    pager.write(PageId(1), b"second, plain").unwrap();
    pager.set_pipeline(shift(5)).unwrap();
    pager.write(PageId(2), b"third, shifted").unwrap();
    pager.write(PageId(1), b"second, shifted now").unwrap();
    assert_eq!(content(&pager, 0).unwrap(), b"first, plain");
    assert_eq!(content(&pager, 1).unwrap(), b"second, shifted now");
    assert_eq!(content(&pager, 2).unwrap(), b"third, shifted");
    pager.sync(Durability::None).unwrap();
    drop(pager);
    let hashes: Vec<Arc<dyn HashFunction>> = vec![crc];
    let with: Vec<Arc<dyn Encoding>> = vec![Arc::new(Shift(5))];
    let (again, _) = Pager::open(&fs, path, &hashes, &with).unwrap();
    assert_eq!(content(&again, 1).unwrap(), b"second, shifted now");
    assert_eq!(content(&again, 2).unwrap(), b"third, shifted");
    again.write(PageId(0), b"first, plain again").unwrap();
    assert_eq!(content(&again, 0).unwrap(), b"first, plain again");
    drop(again);
    let (without, _) = Pager::open(&fs, path, &hashes, &[]).unwrap();
    assert_eq!(content(&without, 0).unwrap(), b"first, plain again");
    assert_eq!(content(&without, 2), Err(Code::Api), "names an encoding not given");
    let live = Arc::new(LivePipeline::default());
    let scratch = Pager::create_scratch(&fs, Path::new("/scratch"), Keyed::by("k0"), &live, 4096).unwrap();
    assert_eq!(scratch.set_pipeline(Pipeline::none()).err().map(|e| e.code), Some(Code::Api));
}

/// Verifies: [DESIGN] RULES.md D30, D48: a keyed function names its key
/// in each page's trailer, the header and the meta slots: pages checksummed
/// under one key read back once the file is keyed by another, the file
/// reopened keyed by either verifies them all, its header by the key it
/// was made with, and a page naming a key the function cannot take is
/// refused (SQLCODE -910); the file is keyed again only by its own
/// function, keyed.
#[test]
fn pages_keep_the_key_their_checksums_were_made_with() {
    let (fs, path) = (MemoryFileSystem::new(), Path::new("/keyed"));
    let pager = Pager::create(&fs, path, Keyed::by("k1"), Pipeline::none(), 4096).unwrap();
    pager.allocate(3);
    pager.write(PageId(0), b"under k1").unwrap();
    pager.sync(Durability::None).unwrap();
    pager.set_hash(Keyed::by("k2")).unwrap();
    pager.write(PageId(1), b"under k2").unwrap();
    pager.sync(Durability::None).unwrap();
    assert_eq!(content(&pager, 0).unwrap(), b"under k1");
    assert_eq!(content(&pager, 1).unwrap(), b"under k2");
    let crc: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    assert_eq!(pager.set_hash(crc).err().map(|e| e.code), Some(Code::Api), "another function");
    pager.set_hash(Keyed::by("x9")).unwrap();
    pager.write(PageId(2), b"under a key no one has").unwrap();
    drop(pager);
    for name in ["k1", "k2"] {
        let (again, _) = Pager::open(&fs, path, &[Keyed::by(name)], &[]).unwrap();
        assert_eq!(content(&again, 0).unwrap(), b"under k1", "keyed by {name}");
        assert_eq!(content(&again, 1).unwrap(), b"under k2", "keyed by {name}");
        assert_eq!(content(&again, 2), Err(Code::Api), "keyed by {name}: no key x9");
    }
}

/// Verifies: [DESIGN] RULES.md D30: a key name's field holds the name,
/// at most `MAX_KEY_NAME` bytes, read back as written, none as empty; one
/// claiming more is malformed; a function not keyed verifies with itself
/// whatever the name, a keyed one with itself for its own name and keyed
/// anew for another.
#[test]
fn key_names_are_recorded_and_read_back() {
    assert_eq!(read_key_field(&key_field(Some("payroll")), 0), Some("payroll"));
    assert_eq!(read_key_field(&key_field(None), 0), Some(""));
    let long = "n".repeat(MAX_KEY_NAME + 5);
    assert_eq!(read_key_field(&key_field(Some(&long)), 0).map(str::len), Some(MAX_KEY_NAME));
    let mut bad = key_field(Some("k"));
    bad[0] = u8::try_from(MAX_KEY_NAME + 1).unwrap();
    assert_eq!(read_key_field(&bad, 0), None);
    assert_eq!(read_key_field(&bad[..1], 0), None);
    let crc: Arc<dyn HashFunction> = Arc::new(Crc::new(CRC32_ISCSI).unwrap());
    assert_eq!(verifier_of(&crc, "anything").unwrap().id(), crc.id());
    let k1 = Keyed::by("k1");
    assert_eq!(verifier_of(&k1, "k1").unwrap().key_name(), Some("k1"));
    assert_eq!(verifier_of(&k1, "k7").unwrap().key_name(), Some("k7"));
    assert_eq!(verifier_of(&k1, "x7").err().map(|e| e.code), Some(Code::Api));
    assert_eq!(crc.keyed_by("k1").err().map(|e| e.code), Some(Code::Api), "not keyed");
}
