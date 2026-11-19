//! \[EXTENSION\] A host plug-in's encodings and checksums (RULES.md
//! D246): the transforms each page and record passes through as it is
//! written and read, and the digests that check them.
//!
//! An encoding is asked one call of a page as the page is written or
//! read, off any critical section, as the buffer pool writes and reads
//! with no lock held. A checksum is one call of its parts. Each is
//! registered by the id its declaration gives, which the files it wrote
//! name, so a database is read through the plug-in that wrote it.

use crate::bytes::Put;
use crate::disk::encoding::{Encoding, EncodingId};
use crate::error::Result;
use crate::hash::{Digest, HashFunction, HashId, MAX_DIGEST_BYTES};
use crate::plugin::bridge::{answer, leaked};
use crate::plugin::errors::invalid_manifest;
use crate::plugin::hosted::HostLink;
use crate::plugin::manifest::HostDeclared;
use crate::plugin::payload::hosted::{ENCODING_ACCEPTS, ENCODING_DECODE, ENCODING_ENCODE, HASH_HASH, put_parts};

/// The id a declaration gives, which must be written and not 0.
fn id_of(d: &HostDeclared) -> Result<u32> {
    let id = u32::try_from(d.number("ID", 0)?).unwrap_or(0);
    if id == 0 {
        return Err(invalid_manifest(&format!(
            "HOST {} {} must give its ID, a number from 1 the files it writes name",
            d.kind.word(),
            d.name
        )));
    }
    Ok(id)
}

/// A host plug-in's encoding.
#[derive(Debug)]
pub struct PluginEncoding {
    link: HostLink,
    id: EncodingId,
    name: &'static str,
    overhead: usize,
    confidential: bool,
}

impl PluginEncoding {
    /// The encoding `d` declares: its ID, the OVERHEAD bytes it may add,
    /// 0 if not written, and CONFIDENTIAL 1 if it encrypts.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 if its ID is missing.
    pub fn new(d: &HostDeclared, link: HostLink) -> Result<PluginEncoding> {
        let overhead = usize::try_from(d.number("OVERHEAD", 0)?).unwrap_or(usize::MAX);
        let confidential = d.number("CONFIDENTIAL", 0)? != 0;
        Ok(PluginEncoding { link, id: EncodingId(id_of(d)?), name: leaked(&d.name), overhead, confidential })
    }
}

impl Encoding for PluginEncoding {
    fn id(&self) -> EncodingId {
        self.id
    }
    fn name(&self) -> &'static str {
        self.name
    }
    fn overhead(&self, _argument: &str) -> usize {
        self.overhead
    }
    fn confidential(&self) -> bool {
        self.confidential
    }
    fn accepts(&self, argument: &str) -> Result<()> {
        let mut payload = Vec::new();
        payload.put_str(argument);
        self.link.ask(ENCODING_ACCEPTS, payload).map(drop)
    }
    fn encode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        self.encode_with(context, "", input, out)
    }
    fn encode_with(&self, context: u64, argument: &str, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let mut payload = Vec::with_capacity(input.len().saturating_add(32));
        payload.put_u64(context);
        payload.put_str(argument);
        payload.put_bytes(input);
        let got = self.link.call(ENCODING_ENCODE, payload)?;
        out.extend_from_slice(&answer(&got, |r| r.bytes().map(<[u8]>::to_vec))?);
        Ok(())
    }
    fn decode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let mut payload = Vec::with_capacity(input.len().saturating_add(16));
        payload.put_u64(context);
        payload.put_bytes(input);
        // A page that does not decode is corrupt, an answer; only the
        // process failing stops the node.
        let got = self.link.ask(ENCODING_DECODE, payload)?;
        out.extend_from_slice(&answer(&got, |r| r.bytes().map(<[u8]>::to_vec))?);
        Ok(())
    }
}

/// A host plug-in's checksum.
#[derive(Debug)]
pub struct PluginHash {
    link: HostLink,
    id: HashId,
    name: &'static str,
    length: usize,
}

impl PluginHash {
    /// The checksum `d` declares: its ID and its digests' LENGTH in bytes,
    /// 1 to 64.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 if its ID is missing or its LENGTH out of range.
    pub fn new(d: &HostDeclared, link: HostLink) -> Result<PluginHash> {
        let length = usize::try_from(d.number("LENGTH", 0)?).unwrap_or(0);
        if length == 0 || length > MAX_DIGEST_BYTES {
            return Err(invalid_manifest(&format!(
                "HOST HASH FUNCTION {} must give its LENGTH, from 1 to {MAX_DIGEST_BYTES} bytes",
                d.name
            )));
        }
        Ok(PluginHash { link, id: HashId(id_of(d)?), name: leaked(&d.name), length })
    }

    /// The digest of `parts` the program gives, of its length.
    fn digest(&self, parts: &[&[u8]]) -> Result<Digest> {
        let mut payload = Vec::new();
        put_parts(&mut payload, parts);
        let got = self.link.call(HASH_HASH, payload)?;
        let bytes = answer(&got, |r| r.bytes().map(<[u8]>::to_vec))?;
        let digest = Some(bytes).filter(|b| b.len() == self.length).and_then(|b| Digest::new(&b));
        digest.ok_or_else(|| crate::plugin::errors::violation(&format!("{} gave a digest not of {} bytes", self.name, self.length)))
    }
}

impl HashFunction for PluginHash {
    fn id(&self) -> HashId {
        self.id
    }
    fn name(&self) -> &'static str {
        self.name
    }
    fn digest_len(&self) -> usize {
        self.length
    }
    fn hash_parts(&self, parts: &[&[u8]]) -> Digest {
        // A digest the program cannot give stops the node, every later
        // statement refused, so the zeros given in its place are checked
        // against nothing that is kept.
        self.digest(parts).unwrap_or_else(|e| {
            self.link.plugin().stop.stop(&e.msg);
            Digest::from_le(0, self.length).unwrap_or_default()
        })
    }
}
