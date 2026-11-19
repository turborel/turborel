//! The example's state transfer SHIPPED (RULES.md D272), which keeps what
//! it fetches in `shipped/` of the working directory, a directory a fetch,
//! cleared as the fetch begins.
//!
//! Its frames are a request, `REQUEST`, a file's number and an offset,
//! the file `ALL` asking for the export's manifest, and an answer,
//! `ANSWER`, the same two and the manifest's or the piece's bytes. Serving,
//! SERVE answers the manifest it was told at EXPORT, and a piece once the
//! host gives it. Fetching, each answer is appended to its file, and the
//! next request names what is still missing. CHUNK reads the files back.
//! The file `crash.chunk` makes CHUNK past a file's first piece write part
//! of a message and end, as a process killed mid-chunk.

use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use turborel::bytes::Put;
use turborel::plugin::payload::replication::{manifest, put_manifest};
use turborel_plugin::{Answer, Manifest, Plugin, PluginError, Served, StateTransfer};

/// The plug-in with it.
pub fn with(plugin: Plugin) -> Plugin {
    plugin.state_transfer("SHIPPED", Shipped::default())
}

/// The first byte of a request, as the driver tells frames apart.
const REQUEST: u8 = 0xF0;
/// The first byte of an answer.
const ANSWER: u8 = 0xF1;
/// The file a request names to ask for the manifest.
const ALL: u32 = u32::MAX;
/// The most bytes a piece or a chunk holds.
const PIECE: u32 = 65_536;

/// SHIPPED: the export last told, and the fetch under way.
#[derive(Default)]
struct Shipped {
    exported: Option<Manifest>,
    fetches: u64,
    fetching: Option<Fetch>,
}

/// A fetch: its number, and the manifest once it came.
struct Fetch {
    number: u64,
    manifest: Option<Manifest>,
}

/// An I/O failure, as an ERROR.
fn failed(e: &std::io::Error) -> PluginError {
    PluginError::external(&format!("shipped/: {e}"))
}

/// A frame of `tag` naming `file` at `at`, followed by `bytes`.
fn frame(tag: u8, (file, at): (u32, u64), bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    out.put_u32(file);
    out.put_u64(at);
    out.extend_from_slice(bytes);
    out
}

/// A frame's file and offset, and the bytes after them.
fn unframed(bytes: &[u8]) -> Answer<((u32, u64), Vec<u8>)> {
    let mut r = turborel::plugin::payload::reader(bytes.get(1..).unwrap_or_default());
    let head = (|| Ok::<_, turborel::error::SqlError>(((r.u32()?, r.u64()?), r.rest().to_vec())))();
    head.map_err(|e| PluginError::external(&format!("a frame is malformed: {}", e.msg)))
}

/// The directory of fetch `number`.
fn dir_of(number: u64) -> PathBuf {
    Path::new("shipped").join(number.to_string())
}

/// The length of `path`, 0 if it is not there.
fn length_of(path: &Path) -> u64 {
    fs::metadata(path).map_or(0, |m| m.len())
}

impl Shipped {
    /// The fetch numbered `number`.
    fn fetch(&mut self, number: u64) -> Answer<&mut Fetch> {
        self.fetching.as_mut().filter(|f| f.number == number).ok_or_else(|| PluginError::external(&format!("no fetch {number}")))
    }
}

impl Fetch {
    /// The next request, of the first file not yet whole, or none.
    fn next(&self) -> Option<Vec<u8>> {
        let files = &self.manifest.as_ref()?.files;
        let missing = files.iter().enumerate().find_map(|(i, (_, length, _))| {
            let held = length_of(&dir_of(self.number).join(i.to_string()));
            (held < *length).then(|| (u32::try_from(i).unwrap_or(ALL), held))
        });
        missing.map(|at| frame(REQUEST, at, &[]))
    }

    /// `piece` of `file` at `at` appended, if it comes where the file ends.
    fn append(&self, (file, at): (u32, u64), piece: &[u8]) -> Answer<()> {
        let path = dir_of(self.number).join(file.to_string());
        if length_of(&path) != at {
            return Ok(());
        }
        let mut out = OpenOptions::new().create(true).append(true).open(path).map_err(|e| failed(&e))?;
        out.write_all(piece).and_then(|()| out.sync_all()).map_err(|e| failed(&e))
    }
}

impl StateTransfer for Shipped {
    fn begin(&mut self, _from: u32) -> Answer<(u64, Option<Vec<u8>>)> {
        self.fetches = self.fetches.checked_add(1).unwrap_or(1);
        let dir = dir_of(self.fetches);
        if dir.exists() {
            fs::remove_dir_all(&dir).map_err(|e| failed(&e))?;
        }
        fs::create_dir_all(&dir).map_err(|e| failed(&e))?;
        self.fetching = Some(Fetch { number: self.fetches, manifest: None });
        Ok((self.fetches, Some(frame(REQUEST, (ALL, 0), &[]))))
    }
    fn take(&mut self, fetch: u64, answer: &[u8]) -> Answer<(bool, Option<Vec<u8>>)> {
        let ((file, at), bytes) = unframed(answer)?;
        let f = self.fetch(fetch)?;
        if file == ALL {
            let got = manifest(&mut turborel::plugin::payload::reader(&bytes)).map_err(|e| PluginError::external(&e.msg))?;
            f.manifest = Some(got);
        } else {
            f.append((file, at), &bytes)?;
        }
        let next = f.next();
        Ok((f.manifest.is_some() && next.is_none(), next))
    }
    fn fetched(&mut self, fetch: u64) -> Answer<Manifest> {
        self.fetch(fetch)?.manifest.clone().ok_or_else(|| PluginError::external("the manifest has not come"))
    }
    fn chunk(&mut self, fetch: u64, file: u32, at: u64, max: u32) -> Answer<Vec<u8>> {
        if at > 0 && Path::new("crash.chunk").exists() {
            // A RESULT's header claiming a whole message, and part of one.
            super::replication::killed(&mut std::io::stdout(), &[0, 0, 1, 0, 3, 7])
        }
        let mut read = fs::File::open(dir_of(self.fetch(fetch)?.number).join(file.to_string())).map_err(|e| failed(&e))?;
        read.seek(SeekFrom::Start(at)).map_err(|e| failed(&e))?;
        let mut chunk = Vec::new();
        read.take(u64::from(max.min(PIECE))).read_to_end(&mut chunk).map_err(|e| failed(&e))?;
        Ok(chunk)
    }
    fn export(&mut self, _export: u64, manifest: Manifest) -> Answer<()> {
        self.exported = Some(manifest);
        Ok(())
    }
    fn serve(&mut self, _to_export: (u32, u64), request: &[u8], given: Option<(u32, u64, Vec<u8>)>) -> Answer<Served> {
        let Some(exported) = &self.exported else { return Ok(Served::Nothing) };
        let ((file, at), _) = unframed(request)?;
        if file == ALL {
            let mut all = Vec::new();
            put_manifest(&mut all, exported);
            return Ok(Served::Answer(frame(ANSWER, (ALL, 0), &all)));
        }
        Ok(match given {
            Some((_, _, piece)) => Served::Answer(frame(ANSWER, (file, at), &piece)),
            None => Served::Need(file, at, PIECE),
        })
    }
}
