//! The sample frames beside the protocol's reference
//! (`doc/client/05-protocol-frames.tsv`, RULES.md D398): made here from
//! the protocol's own encoders, each decoded again, and compared with the
//! file checked in, so the file never says what the code does not.

mod replies;
mod requests;
mod types;
mod values;

use std::fmt::Write as _;

use crate::compress::{COMPRESSED, decompress};
use crate::{HEADER, MAX_BODY, Reply, Request, header};

/// The file of sample frames, from the protocol crate's directory.
const FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../doc/client/05-protocol-frames.tsv");
/// What the file says of itself, its lines before the frames.
const PREAMBLE: &str = "\
# Sample frames of the turborel native protocol, version 1 (doc/client/05-protocol.md).
# Made by the protocol crate's tests from its own encoders; do not edit by hand.
# One frame a line, five fields apart by one tab each:
#   direction  client or server, who sends it
#   name       a name of the sample, unique in the file
#   needs      the capabilities it needs granted, apart by commas, or - for none
#   hex        the whole frame, its header too, two upper-case hex digits an octet
#   means      what it decodes to
# A line beginning with # is a comment. No field holds a tab.
";
/// Samples whose bytes decode without a type registry but encode again
/// otherwise, as a plug-in's type is then read under no kind of its own.
const OPAQUE: [&str; 1] = ["description-plugin-type"];

/// Who sends a frame.
#[derive(Clone, Copy)]
pub(super) enum Side {
    Client,
    Server,
}

/// A sample frame, with who sends it, its name, the capabilities it
/// needs, its bytes and what they mean.
pub(super) struct Sample {
    side: Side,
    name: &'static str,
    needs: &'static [&'static str],
    bytes: Vec<u8>,
    means: String,
}

/// The sample of `request`, its frame encoded.
pub(super) fn client(name: &'static str, needs: &'static [&'static str], request: &Request, means: &str) -> Sample {
    let mut bytes = Vec::new();
    request.encode(&mut bytes);
    Sample { side: Side::Client, name, needs, bytes, means: means.to_owned() }
}

/// The sample of `reply`, its frame encoded.
pub(super) fn server(name: &'static str, needs: &'static [&'static str], reply: &Reply, means: &str) -> Sample {
    let mut bytes = Vec::new();
    reply.encode(&mut bytes);
    Sample { side: Side::Server, name, needs, bytes, means: means.to_owned() }
}

/// The sample of `bytes` as they are, sent by `side`.
pub(super) fn raw(side: Side, (name, needs): (&'static str, &'static [&'static str]), bytes: Vec<u8>, means: &str) -> Sample {
    Sample { side, name, needs, bytes, means: means.to_owned() }
}

/// Every sample, in the order of the file.
fn samples() -> Vec<Sample> {
    let mut all = requests::samples();
    all.extend(replies::samples());
    all.extend(types::samples());
    all.extend(values::samples());
    all
}

/// The kind and body of the one frame `bytes` are, inflated if
/// compressed.
fn unframed(bytes: &[u8]) -> (u8, Vec<u8>) {
    let (head, body) = bytes.split_first_chunk::<HEADER>().unwrap();
    let (kind, length) = header(*head, MAX_BODY).unwrap();
    assert_eq!(body.len(), length, "one frame, whole");
    if kind == COMPRESSED { decompress(body, MAX_BODY).unwrap() } else { (kind, body.to_vec()) }
}

/// Checks that `sample` decodes, and encodes again to its own bytes, or
/// to its inflated frame's if it is compressed.
fn decodes(sample: &Sample) {
    let (kind, body) = unframed(&sample.bytes);
    let mut again = Vec::new();
    match sample.side {
        Side::Client => Request::decode(kind, &body).unwrap_or_else(|e| panic!("{}: {e}", sample.name)).encode(&mut again),
        Side::Server => Reply::decode(kind, &body).unwrap_or_else(|e| panic!("{}: {e}", sample.name)).encode(&mut again),
    }
    if OPAQUE.contains(&sample.name) {
        return;
    }
    let inflated = [&[kind][..], &u32::try_from(body.len()).unwrap().to_le_bytes(), &body].concat();
    assert_eq!(again, inflated, "{} encodes again as it was", sample.name);
}

/// The file's text of `samples`.
fn rendered(samples: &[Sample]) -> String {
    let mut text = String::from(PREAMBLE);
    for s in samples {
        let side = match s.side {
            Side::Client => "client",
            Side::Server => "server",
        };
        let needs = if s.needs.is_empty() { "-".to_owned() } else { s.needs.join(",") };
        let hex: String = s.bytes.iter().fold(String::new(), |mut h, b| {
            write!(h, "{b:02X}").unwrap();
            h
        });
        writeln!(text, "{side}\t{}\t{needs}\t{hex}\t{}", s.name, s.means).unwrap();
    }
    text
}

/// Verifies: [DESIGN] RULES.md D398, R-DOC-8: the file of sample frames
/// beside the protocol's reference is what the protocol's encoders make:
/// every request and reply kind, every value and data type tag, each
/// tail, compressed frames and the edges a decoder meets; each frame
/// decodes and encodes again the same, each name is unique, and no field
/// holds a tab. A file that differs fails, the frames as they should be
/// written beside the temporary directory's for the file to take.
#[test]
fn the_sample_frames_file_is_what_the_code_sends() {
    let all = samples();
    for s in &all {
        decodes(s);
        assert!(!s.means.contains(['\t', '\n']), "{}", s.name);
        assert_eq!(all.iter().filter(|o| o.name == s.name).count(), 1, "{} is unique", s.name);
    }
    let text = rendered(&all);
    let checked_in = std::fs::read_to_string(FILE).unwrap_or_default();
    if checked_in != text {
        let made = std::env::temp_dir().join("05-protocol-frames.tsv");
        std::fs::write(&made, &text).unwrap();
        panic!("{FILE} is not what the code sends; the frames it should hold are in {}", made.display());
    }
}
