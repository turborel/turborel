//! \[DESIGN\] The formats of a database's files (RULES.md D30, D36):
//! the hash function each is checksummed with and the encodings each
//! passes through, both pluggable, and the page and log segment sizes.
//!
//! A file names in its header the hash function and encodings it was
//! written with, and is read with them: any registered. The formats new
//! files are written in are the database's settings fixed at its creation
//! (`settings::Effect::Creation`), given before it is created and recorded
//! in its first image; so a database is opened with no options, and a
//! file it adds is written as the others were.

use std::sync::Arc;

use super::{Database, State, api};
use crate::diagnostics::Extension;
use crate::disk::encoding::{Encoding, Pipeline};
use crate::error::{Code, Result, SqlError};
use crate::hash::HashFunction;
use crate::settings::{self, CHECKSUM, ENCODINGS, Effect, LOG_SEGMENT_BYTES, PAGE_BYTES, POOL_BYTES, Settings};
use crate::value::Value;

/// \[EXTENSION\] How a database's files are written and read: new files
/// checksummed with `hash` and encoded through `pipeline`; a file read with
/// the hash function and encodings it names, of `hashes` and `encodings`.
#[derive(Clone)]
#[non_exhaustive]
pub struct FileFormats {
    /// The hash function new files are checksummed with.
    pub hash: Arc<dyn HashFunction>,
    /// The encodings new files pass through.
    pub pipeline: Pipeline,
    /// The hash functions a file may be read with.
    pub hashes: Vec<Arc<dyn HashFunction>>,
    /// The encodings a file may be read through.
    pub encodings: Vec<Arc<dyn Encoding>>,
}

impl FileFormats {
    /// Files checksummed with `hash` alone, and not encoded.
    pub fn of(hash: Arc<dyn HashFunction>) -> FileFormats {
        FileFormats { hashes: vec![Arc::clone(&hash)], hash, pipeline: Pipeline::none(), encodings: Vec::new() }
    }
}

/// What a database kept in a directory is made of, as its settings say.
pub(super) struct FileLayout {
    /// Its files' formats.
    pub(super) formats: FileFormats,
    /// The bytes of a page of its data file, when it is made.
    pub(super) page_size: u32,
    /// The bytes after which a segment of its log is ended.
    pub(super) segment_size: u64,
    /// The bytes of its buffer pool.
    pub(super) pool_bytes: usize,
}

impl State {
    /// The layout `values` give, its names those registered.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a hash function or encoding they name is not
    /// registered, or a size is out of range.
    pub(super) fn layout(&self, values: &Settings) -> Result<FileLayout> {
        let named = |key| if let Value::Char(s) = values.get(key) { s.clone() } else { String::new() };
        let checksum = named(CHECKSUM);
        let hash = self
            .hashes
            .iter()
            .find(|h| h.name().eq_ignore_ascii_case(&checksum))
            .cloned()
            .ok_or_else(|| unregistered("hash function", &checksum))?;
        let pipeline = self.encodings.named(&named(ENCODINGS))?;
        let size = |key, what: &str| u64::try_from(values.integer(key)).map_err(|_| api(format!("[DESIGN] {what} is out of range")));
        let encodings = self.encodings.for_files(&pipeline)?;
        let formats = FileFormats { hash, pipeline, hashes: self.hashes.clone(), encodings };
        Ok(FileLayout {
            formats,
            page_size: u32::try_from(size(PAGE_BYTES, "STORAGE.PAGE_BYTES")?)
                .map_err(|_| api("[DESIGN] STORAGE.PAGE_BYTES is out of range"))?,
            segment_size: size(LOG_SEGMENT_BYTES, "LOG.SEGMENT_BYTES")?,
            pool_bytes: usize::try_from(size(POOL_BYTES, "STORAGE.POOL_BYTES")?).unwrap_or(usize::MAX),
        })
    }

    /// The settings of `recorded`, an image's values, over this state's:
    /// those fixed at creation, and those read as the database opens. A
    /// value given before it opened is kept over one read as it opens.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a value given before it opened differs from the one
    /// fixed at its creation.
    pub(super) fn recorded(&self, recorded: &dyn Fn(&str) -> Option<Value>) -> Result<Settings> {
        let mut values = self.settings.clone();
        for listed in self.settings.list(&settings::Own::default()) {
            let d = &listed.definition;
            let Some(kept) = recorded(&d.name) else { continue };
            match (d.effect, listed.source) {
                (Effect::Creation, settings::Source::Database) => {
                    if settings::accepted(&d.name, &d.kind, &kept)?.to_string() != listed.value.to_string() {
                        return Err(api(format!(
                            "[DESIGN] the database was created with {} = {kept}, fixed then: it cannot be {}",
                            d.name, listed.value
                        )));
                    }
                }
                (Effect::Creation | Effect::Start, settings::Source::Default | settings::Source::Session) => {
                    values.set(&d.name, Some(&kept))?;
                }
                // Given before it opened, or not read as it does.
                (Effect::Start, settings::Source::Database)
                | (
                    Effect::Statement | Effect::Transaction | Effect::Connection,
                    settings::Source::Default | settings::Source::Database | settings::Source::Session,
                ) => {}
            }
        }
        Ok(values)
    }
}

/// A refusal of a name nothing is registered under.
fn unregistered(what: &str, name: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] no {what} {name} is registered: register it before the database is opened"))
}

impl Database {
    /// \[EXTENSION\] Registers a hash function, before the database is
    /// opened from its files: one its files may be checksummed with
    /// (STORAGE.CHECKSUM), and must be read with if they are (RULES.md
    /// D30).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one of its name or id is registered.
    pub fn register_hash_function(&self, hash: Box<dyn HashFunction>) -> Result<()> {
        let name = hash.name();
        let done = self.change(|st| {
            if st.hashes.iter().any(|h| h.name().eq_ignore_ascii_case(name) || h.id() == hash.id()) {
                return Err(api(format!("[DESIGN] a hash function of the name {name}, or of its id, is registered")));
            }
            st.hashes.push(Arc::from(hash));
            Ok(())
        });
        self.registered((Extension::HashFunction, name), done)
    }

    /// \[EXTENSION\] Registers an encoding, before the database is opened
    /// from its files: one its files may pass through (STORAGE.ENCODINGS),
    /// and must be read through if they do (RULES.md D30).
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one of its name or id is registered.
    pub fn register_encoding(&self, encoding: Box<dyn Encoding>) -> Result<()> {
        let name = encoding.name();
        let done = self.change(|st| st.encodings.add(encoding));
        self.registered((Extension::Encoding, name), done)
    }
}
