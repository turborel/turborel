//! \[DESIGN\] The Lob messages (RULES.md D45, D79): what a client asks of
//! a large object it reaches by its locator [ISO/IEC 9075-2:2023
//! §4.37.5], a range of it read, a new one made a piece at a time, or the
//! locator freed or held, and what the server gives back, the octets read
//! or the new locator. Freeing, holding and appending reply with a
//! completion, as a statement does.

use turborel::bytes::{Put, Reader};
use turborel::error::Result;
use turborel::value::lob::LobKind;

use crate::fields::{flag, put_flag};

/// What a client asks of a large object.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LobRequest {
    /// At most `count` of its octets from octet `from`, from 0, or of a
    /// character one its characters if `characters`: a chunk at most, of
    /// whole characters, as the server sends at once.
    Read {
        /// Its locator.
        locator: u32,
        /// The first octet or character, from 0.
        from: u64,
        /// The most octets or characters.
        count: u64,
        /// In characters, not octets.
        characters: bool,
    },
    /// A new large object of `kind`, empty, to append to.
    Create(LobKind),
    /// `bytes` added to one made by Create and not yet used: UTF-8 of a
    /// character one, a character possibly split between two Appends.
    Append {
        /// Its locator.
        locator: u32,
        /// Its next octets.
        bytes: Vec<u8>,
    },
    /// [§14.17] Its locator no longer valid.
    Free(u32),
    /// [§14.18] Its locator holdable, valid past its transaction's commit.
    Hold(u32),
}

/// What the server gives back of a LobRequest but a completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LobReply {
    /// The octets a Read asked for.
    Data(Vec<u8>),
    /// The locator of the large object Create made.
    Locator(u32),
}

const READ: u8 = 0;
const CREATE: u8 = 1;
const APPEND: u8 = 2;
const FREE: u8 = 3;
const HOLD: u8 = 4;
const DATA: u8 = 0;
const LOCATOR: u8 = 1;

/// A large object's kind, as one octet.
fn put_kind(b: &mut Vec<u8>, kind: LobKind) {
    b.put_u8(match kind {
        LobKind::Character => 0,
        LobKind::Binary => 1,
    });
}

/// A large object's kind, as `put_kind` wrote it.
fn kind(r: &mut Reader<'_>) -> Result<LobKind> {
    match r.u8()? {
        0 => Ok(LobKind::Character),
        1 => Ok(LobKind::Binary),
        _ => Err(r.corrupt("not a kind of large object")),
    }
}

impl LobRequest {
    /// Appends its body to `b`.
    pub(crate) fn put(&self, b: &mut Vec<u8>) {
        match self {
            LobRequest::Read { locator, from, count, characters } => {
                b.put_u8(READ);
                b.put_u32(*locator);
                b.put_u64(*from);
                b.put_u64(*count);
                put_flag(b, *characters);
            }
            LobRequest::Create(k) => {
                b.put_u8(CREATE);
                put_kind(b, *k);
            }
            LobRequest::Append { locator, bytes } => {
                b.put_u8(APPEND);
                b.put_u32(*locator);
                b.put_bytes(bytes);
            }
            LobRequest::Free(locator) => {
                b.put_u8(FREE);
                b.put_u32(*locator);
            }
            LobRequest::Hold(locator) => {
                b.put_u8(HOLD);
                b.put_u32(*locator);
            }
        }
    }

    /// The request `r` holds, as `put` wrote it.
    pub(crate) fn read(r: &mut Reader<'_>) -> Result<LobRequest> {
        match r.u8()? {
            READ => Ok(LobRequest::Read { locator: r.u32()?, from: r.u64()?, count: r.u64()?, characters: flag(r)? }),
            CREATE => Ok(LobRequest::Create(kind(r)?)),
            APPEND => Ok(LobRequest::Append { locator: r.u32()?, bytes: r.bytes()?.to_vec() }),
            FREE => Ok(LobRequest::Free(r.u32()?)),
            HOLD => Ok(LobRequest::Hold(r.u32()?)),
            _ => Err(r.corrupt("no Lob request is of this kind")),
        }
    }
}

impl LobReply {
    /// Appends its body to `b`.
    pub(crate) fn put(&self, b: &mut Vec<u8>) {
        match self {
            LobReply::Data(bytes) => {
                b.put_u8(DATA);
                b.put_bytes(bytes);
            }
            LobReply::Locator(locator) => {
                b.put_u8(LOCATOR);
                b.put_u32(*locator);
            }
        }
    }

    /// The reply `r` holds, as `put` wrote it.
    pub(crate) fn read(r: &mut Reader<'_>) -> Result<LobReply> {
        match r.u8()? {
            DATA => Ok(LobReply::Data(r.bytes()?.to_vec())),
            LOCATOR => Ok(LobReply::Locator(r.u32()?)),
            _ => Err(r.corrupt("no Lob reply is of this kind")),
        }
    }
}
