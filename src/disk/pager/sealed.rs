//! \[DESIGN\] A pager file's pages as bytes on the file (RULES.md D30):
//! each sealed with its trailer, its number, the name of the key its
//! checksum is made with if the file's hash function is keyed, and the
//! digest of all before it; and unsealed, verified by the function keyed
//! as its trailer names (`named.rs`), so a page keeps its key while the
//! file's changes.

use std::sync::atomic::Ordering;

use super::format::{self, PAGE_NUMBER_BYTES};
use super::{Pager, add};
use crate::error::{Code, Result, SqlError};
use crate::hash::{KEY_FIELD, key_field, read_key_field};

impl Pager {
    /// Bytes of a page's body: the page less its trailer.
    pub(super) fn body_size(&self) -> usize {
        self.page_size.saturating_sub(self.hash.trailer_len())
    }

    /// The byte offset of page `id`.
    pub(super) fn offset(&self, id: u64) -> Result<u64> {
        let page = u64::try_from(self.page_size).unwrap_or(u64::MAX);
        id.checked_add(1)
            .and_then(|slot| slot.checked_mul(page))
            .ok_or_else(|| SqlError::new(Code::Io, format!("[DESIGN] page {id} of {} would be beyond 2^64 bytes", self.path.display())))
    }

    /// Appends page `id` holding `body`, padded, with its trailer, to `out`.
    pub(super) fn seal(&self, id: u64, body: &[u8], out: &mut Vec<u8>) {
        let hash = self.hash.current();
        let start = out.len();
        out.extend_from_slice(body);
        out.resize(start.saturating_add(self.body_size()), 0);
        out.extend_from_slice(&id.to_le_bytes());
        if hash.keyed() {
            out.extend_from_slice(&key_field(hash.key_name()));
        }
        let digest = hash.hash(out.get(start..).unwrap_or_default());
        out.extend_from_slice(digest.as_bytes());
    }

    /// The body of page `id` within its bytes `page`; `None` if it was never
    /// written.
    pub(super) fn unseal<'a>(&self, id: u64, page: &'a [u8]) -> Result<Option<&'a [u8]>> {
        if page.iter().all(|&b| b == 0) {
            return Ok(None);
        }
        let body_size = self.body_size();
        let (body, trailer) = page.split_at(body_size.min(page.len()));
        let stored = format::le_u64(trailer, 0);
        let keyed = self.hash.current().keyed();
        let name = if keyed { read_key_field(trailer, PAGE_NUMBER_BYTES) } else { Some("") };
        let digest_at = PAGE_NUMBER_BYTES.saturating_add(if keyed { KEY_FIELD } else { 0 });
        let digest = trailer.get(digest_at..).unwrap_or_default();
        let signed = page.get(..body_size.saturating_add(digest_at)).unwrap_or_default();
        let verified = match name.map(|name| self.hash.verifier(name)) {
            Some(Ok(hash)) => hash.hash(signed).as_bytes() == digest,
            Some(Err(e)) => return Err(e),
            None => false,
        };
        if stored == Some(id) && verified {
            return Ok(Some(body));
        }
        add(&self.counters.corrupt_reads, 1);
        let why = match stored.filter(|&other| other != id) {
            Some(other) => format!("it holds page {other}, written in the wrong place"),
            None => "it fails its checksum: torn or corrupted".to_owned(),
        };
        Err(SqlError::new(Code::Corrupt, format!("[DESIGN] page {id} of {}: {why}", self.path.display())))
    }

    /// The body of page `id`; `None` if it was never written.
    pub(super) fn read_body(&self, id: u64) -> Result<Option<Vec<u8>>> {
        if id >= self.on_file.load(Ordering::Acquire) {
            return Ok(None);
        }
        let page = self.read_run(id, 1)?;
        Ok(self.unseal(id, &page)?.map(<[u8]>::to_vec))
    }

    /// The bytes of `count` pages from `start`, in one read.
    pub(super) fn read_run(&self, start: u64, count: u64) -> Result<Vec<u8>> {
        let len =
            usize::try_from(count).ok().and_then(|n| n.checked_mul(self.page_size)).ok_or_else(|| self.damaged("an extent is too long"))?;
        let mut bytes = vec![0; len];
        self.file.read_exact_at(&mut bytes, self.offset(start)?)?;
        Ok(bytes)
    }

    /// Writes `sealed`, whole pages from `start`, in one write.
    pub(super) fn write_run(&self, start: u64, sealed: &[u8]) -> Result<()> {
        self.file.write_all_at(sealed, self.offset(start)?)?;
        let pages = u64::try_from(sealed.len().checked_div(self.page_size).unwrap_or_default()).unwrap_or(u64::MAX);
        self.on_file.fetch_max(start.saturating_add(pages), Ordering::AcqRel);
        Ok(())
    }
}
