//! \[EXTENSION\] A state transfer (kind 34, RULES.md D272): the frames of
//! a replica's state transfer, made and read by the program, the export
//! and the staging kept by the host. Serving, an export is told once and
//! each request answered from it, asking the host for the pieces of its
//! files it needs; fetching, requests are made and answers taken until
//! the fetch is done, then its files copied out in chunks, each with the
//! digest the host checks.

use turborel::bytes::Put;
use turborel::plugin::payload::replication::{self as p, Manifest, Served};

use super::super::super::dispatch::{read, u64_of};
use super::super::super::serve::Cx;
use super::super::super::Answer;
use super::super::HostObject;
use super::unknown;

/// A `u32` read.
fn u32_of(r: &mut turborel::bytes::Reader<'_>) -> turborel::error::Result<u32> {
    r.u32()
}

/// \[EXTENSION\] A state transfer.
pub trait StateTransfer: Send {
    /// Begins a fetch from replica `from`: its number, and its first
    /// request if any.
    ///
    /// # Errors
    ///
    /// The transfer's: the node stops.
    fn begin(&mut self, from: u32) -> Answer<(u64, Option<Vec<u8>>)>;
    /// Takes an answer to fetch `fetch`: whether it is done, and the next
    /// request if any.
    ///
    /// # Errors
    ///
    /// The transfer's: the node stops.
    fn take(&mut self, fetch: u64, frame: &[u8]) -> Answer<(bool, Option<Vec<u8>>)>;
    /// The manifest of fetch `fetch`, done.
    ///
    /// # Errors
    ///
    /// The transfer's: the node stops.
    fn fetched(&mut self, fetch: u64) -> Answer<Manifest>;
    /// At most `max` bytes of fetch `fetch`'s file `file` from `at`, at
    /// least one while any are left.
    ///
    /// # Errors
    ///
    /// The transfer's: the node stops.
    fn chunk(&mut self, fetch: u64, file: u32, at: u64, max: u32) -> Answer<Vec<u8>>;
    /// The export of number `export` is `manifest`, until another is told.
    ///
    /// # Errors
    ///
    /// The transfer's: the node stops.
    fn export(&mut self, export: u64, manifest: Manifest) -> Answer<()>;
    /// The answer to `request` of a replica, out of an export, the two
    /// numbers `to_export`,
    /// given the piece asked last, if one was.
    ///
    /// # Errors
    ///
    /// The transfer's: the node stops.
    fn serve(&mut self, to_export: (u32, u64), request: &[u8], given: Option<(u32, u64, Vec<u8>)>) -> Answer<Served>;
}

/// A state transfer as a host object.
pub(super) struct Transferring<T>(pub(super) T);

impl<T: StateTransfer> HostObject for Transferring<T> {
    fn answer(&mut self, _cx: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        let r = &mut turborel::plugin::payload::reader(payload);
        let (t, mut out) = (&mut self.0, Vec::new());
        match op {
            p::TRANSFER_BEGIN => {
                let (fetch, request) = t.begin(read(r, u32_of)?)?;
                out.put_u64(fetch);
                p::put_optional(&mut out, request.as_deref());
            }
            p::TRANSFER_TAKE => {
                let (fetch, frame) = read(r, |r| Ok((r.u64()?, r.bytes()?.to_vec())))?;
                let (done, request) = t.take(fetch, &frame)?;
                out.put_u8(u8::from(done));
                p::put_optional(&mut out, request.as_deref());
            }
            p::TRANSFER_FETCHED => p::put_manifest(&mut out, &t.fetched(read(r, u64_of)?)?),
            p::TRANSFER_CHUNK => {
                let (fetch, file, at, max) = read(r, |r| Ok((r.u64()?, r.u32()?, r.u64()?, r.u32()?)))?;
                let chunk = t.chunk(fetch, file, at, max)?;
                out.put_bytes(&chunk);
                out.put_u64(p::chunk_digest(&chunk));
            }
            p::TRANSFER_SERVE => {
                let (to, export, request, given) = read(r, |r| Ok((r.u32()?, r.u64()?, r.bytes()?.to_vec(), p::given(r)?)))?;
                p::put_served(&mut out, &t.serve((to, export), &request, given)?);
            }
            p::TRANSFER_EXPORT => {
                let (export, manifest) = read(r, |r| Ok((r.u64()?, p::manifest(r)?)))?;
                t.export(export, manifest)?;
            }
            _ => return Err(unknown("state transfer", op)),
        }
        Ok(out)
    }
}
