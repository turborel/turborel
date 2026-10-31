//! \[DESIGN\] The encoding pipeline (RULES.md D30): optional encodings that
//! data passes through on its way to a file and back, such as compression
//! or encryption. A `Pipeline` is an ordered list of `Encoding`s: writing
//! applies them in order (compress, then encrypt), reading in reverse. The
//! block manager encodes each block's payload, the pager each page's whole
//! content; both record the pipeline's ids in their file's header, so a
//! file is always read through the pipeline it was written with. An empty
//! pipeline, the default, costs nothing.
//!
//! An encoding may change the length of what it encodes; both components
//! store any length. One that does not help a particular input (data that
//! will not compress) says so within its own output.
//!
//! A database's files pass through the pipeline STORAGE.ENCODINGS names; a
//! table's data, in an engine that seals it (`registry.rs`), through the
//! one its options name. An encoding may take an argument, written after
//! a colon (`AES-256-GCM:PAYROLL`, the key's name): it is bound to it
//! (`Bound`), and records in its output whatever its decoding needs, so a
//! file's header names only ids. No encoding is built in: compression and
//! encryption come as plug-ins on vetted crates (RULES.md D26, D48), LZ4
//! from `turborel-lz4`, AES-256-GCM from `turborel-aes`. Keys are never
//! stored in the file: an encryption plug-in gets them from a keyring its
//! host gives it.

use std::borrow::Cow;
use std::path::Path;
use std::sync::Arc;

use crate::error::{Code, Result, SqlError};

mod registry;
pub use registry::Encodings;

/// \[DESIGN\] The most encodings in one pipeline: room for them in a file
/// header, and more than compression and encryption need.
pub const MAX_ENCODINGS: usize = 4;

/// \[DESIGN\] Identifies an encoding in file formats. An id, once given, is
/// never changed or reused: a file written with it must always be read with
/// the same encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EncodingId(pub u32);

/// \[DESIGN\] An encoding: an extension point (RULES.md D30).
///
/// Contract (RULES.md R-SAFE-3; checked by `test/integration/encodings.rs`
/// with test encodings):
/// * `decode(c, encode(c, x)) == x` for every input `x` and context `c`.
///   `encode` need not be deterministic (an encryption may draw a nonce).
/// * `decode` of bytes `encode` did not produce, or produced with another
///   context, fails rather than returning other bytes, where the encoding
///   can tell (an authenticated encryption can; a compression may not).
/// * The context is where the data is stored: the page number for the
///   pager; 0 for the block manager, whose block ids are known only once a
///   block is written. Pages are rewritten in place under the same number,
///   so an encryption must not derive its nonce from the context alone.
/// * `id` is unique among the encodings in use, and constant forever.
/// * `encode` adds at most `overhead` bytes to any input: the pager keeps
///   that room in every page, so an encoded page fits its slot.
/// * An argument `accepts` takes is one `encode_with` encodes with; what
///   decoding needs of it, the output records.
/// * `encode`, with no argument, may use what lasts only while the process
///   does, as an encryption's ephemeral key: it writes what no later
///   process reads, as a spill area's files (`TempFiles::pipeline`). What
///   is kept names its argument, which `accepts` checks.
/// * Methods may be called from several threads at once (`Send + Sync`), and
///   never call back into the component using them (RULES.md R-ERR-4).
pub trait Encoding: Send + Sync {
    /// Its id in file formats.
    fn id(&self) -> EncodingId;
    /// Its name, for messages.
    fn name(&self) -> &'static str;
    /// \[DESIGN\] Itself as it declares settings and is told them
    /// (`Configurable`, RULES.md D36), if it has any. Default: none.
    fn configurable(&self) -> Option<&dyn crate::settings::Configurable> {
        None
    }
    /// The most bytes `encode_with` adds to any input, with `argument`.
    fn overhead(&self, argument: &str) -> usize;
    /// Whether whoever reads what it encodes, without what it was given (a
    /// key), learns nothing of it: an encryption. Default: no.
    fn confidential(&self) -> bool {
        false
    }
    /// Whether it takes `argument`. Default: none but the empty one.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if it does not, saying why.
    fn accepts(&self, argument: &str) -> Result<()> {
        if argument.is_empty() {
            return Ok(());
        }
        Err(SqlError::new(Code::Api, format!("[DESIGN] the encoding {} takes no argument, not {argument}", self.name())))
    }
    /// Appends the encoding of `input` to `out`, with no argument.
    ///
    /// # Errors
    ///
    /// Any failure to encode; the write that asked for it fails.
    fn encode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()>;
    /// Appends the encoding of `input` to `out`, with `argument`, one it
    /// accepts. Default: `encode`, for the empty argument alone.
    ///
    /// # Errors
    ///
    /// As `encode`; as `accepts`.
    fn encode_with(&self, context: u64, argument: &str, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        self.accepts(argument)?;
        self.encode(context, input, out)
    }
    /// Appends the decoding of `input` to `out`.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if `input` is not something this encoding produced;
    /// any other failure to decode.
    fn decode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()>;
}

/// \[DESIGN\] An encoding bound to its argument: an encoding itself, so a
/// pipeline and the encodings a file is read through hold it as any other.
pub struct Bound {
    encoding: Arc<dyn Encoding>,
    argument: String,
}

impl Bound {
    /// `encoding`, bound to `argument`.
    ///
    /// # Errors
    ///
    /// As `Encoding::accepts`.
    pub fn bind(encoding: Arc<dyn Encoding>, argument: &str) -> Result<Arc<dyn Encoding>> {
        if argument.is_empty() {
            return Ok(encoding);
        }
        encoding.accepts(argument)?;
        Ok(Arc::new(Bound { encoding, argument: argument.to_owned() }))
    }
}

impl Encoding for Bound {
    fn id(&self) -> EncodingId {
        self.encoding.id()
    }
    fn name(&self) -> &'static str {
        self.encoding.name()
    }
    fn overhead(&self, _: &str) -> usize {
        self.encoding.overhead(&self.argument)
    }
    fn confidential(&self) -> bool {
        self.encoding.confidential()
    }
    fn encode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        self.encoding.encode_with(context, &self.argument, input, out)
    }
    fn decode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
        self.encoding.decode(context, input, out)
    }
}

/// \[DESIGN\] An ordered list of encodings. Cloning it is cheap.
#[derive(Clone, Default)]
pub struct Pipeline {
    stages: Vec<Arc<dyn Encoding>>,
}

impl Pipeline {
    /// The pipeline that leaves data as it is.
    pub fn none() -> Pipeline {
        Pipeline::default()
    }

    /// The pipeline applying `stages` in order when writing.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if there are more than `MAX_ENCODINGS`, or two share an id.
    pub fn new(stages: Vec<Arc<dyn Encoding>>) -> Result<Pipeline> {
        if stages.len() > MAX_ENCODINGS {
            return Err(SqlError::new(
                Code::Api,
                format!("[DESIGN] a pipeline has at most {MAX_ENCODINGS} encodings, not {}", stages.len()),
            ));
        }
        for (n, stage) in stages.iter().enumerate() {
            if stages.iter().skip(n.saturating_add(1)).any(|later| later.id() == stage.id()) {
                return Err(SqlError::new(Code::Api, format!("[DESIGN] the encoding {} appears twice in a pipeline", stage.name())));
            }
        }
        Ok(Pipeline { stages })
    }

    /// The pipeline whose ids a file records, from the encodings `available`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one of `ids` is not among `available`.
    pub(super) fn resolve(ids: &[EncodingId], available: &[Arc<dyn Encoding>], path: &Path) -> Result<Pipeline> {
        let stages = ids
            .iter()
            .map(|id| {
                available.iter().find(|e| e.id() == *id).map(Arc::clone).ok_or_else(|| {
                    SqlError::new(Code::Api, format!("[DESIGN] {} is encoded with encoding {}, which is not given", path.display(), id.0))
                })
            })
            .collect::<Result<Vec<_>>>()?;
        Pipeline::new(stages)
    }

    /// The ids of its encodings, in order.
    pub fn ids(&self) -> Vec<EncodingId> {
        self.stages.iter().map(|s| s.id()).collect()
    }

    /// Whether it has no encoding.
    pub fn is_empty(&self) -> bool {
        self.stages.is_empty()
    }

    /// Its encodings, in order, each bound to its argument.
    pub fn stages(&self) -> &[Arc<dyn Encoding>] {
        &self.stages
    }

    /// The most bytes it adds to any input.
    pub fn overhead(&self) -> usize {
        self.stages.iter().fold(0_usize, |sum, s| sum.saturating_add(s.overhead("")))
    }

    /// Whether one of its encodings is confidential: an encryption.
    pub fn confidential(&self) -> bool {
        self.stages.iter().any(|s| s.confidential())
    }

    /// `input` passed through every encoding in order.
    pub(super) fn encode<'a>(&self, context: u64, input: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        Self::run(input, |stage, from, to| stage.encode(context, from, to), self.stages.iter())
    }

    /// `input` passed back through every encoding, last first.
    pub(super) fn decode<'a>(&self, context: u64, input: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        Self::run(input, |stage, from, to| stage.decode(context, from, to), self.stages.iter().rev())
    }

    /// `input` passed through `stages` with `step`, two buffers in turn.
    fn run<'a, 's>(
        input: &'a [u8],
        step: impl Fn(&dyn Encoding, &[u8], &mut Vec<u8>) -> Result<()>,
        stages: impl Iterator<Item = &'s Arc<dyn Encoding>>,
    ) -> Result<Cow<'a, [u8]>> {
        let mut current: Cow<'a, [u8]> = Cow::Borrowed(input);
        for stage in stages {
            let mut next = Vec::with_capacity(current.len());
            step(&**stage, &current, &mut next)?;
            current = Cow::Owned(next);
        }
        Ok(current)
    }
}

/// The ids of a pipeline as a header field: a count, then `MAX_ENCODINGS`
/// ids, unused ones zero.
pub(super) fn ids_field(pipeline: &Pipeline) -> Vec<u8> {
    let ids = pipeline.ids();
    let mut field = Vec::with_capacity(ENCODINGS_FIELD);
    field.extend_from_slice(&u32::try_from(ids.len()).unwrap_or(u32::MAX).to_le_bytes());
    for n in 0..MAX_ENCODINGS {
        field.extend_from_slice(&ids.get(n).map_or(0, |id| id.0).to_le_bytes());
    }
    field
}

/// The ids in a header field written by `ids_field`.
pub(super) fn read_ids_field(field: &[u8]) -> Option<Vec<EncodingId>> {
    let word =
        |at: usize| field.get(at..at.checked_add(ID_BYTES)?).and_then(|b| <[u8; ID_BYTES]>::try_from(b).ok()).map(u32::from_le_bytes);
    let count = usize::try_from(word(0)?).ok().filter(|&n| n <= MAX_ENCODINGS)?;
    (0..count).map(|n| word(ID_BYTES.checked_mul(n.checked_add(1)?)?).map(EncodingId)).collect()
}

/// Bytes of an id, and of the header field `ids_field` writes.
const ID_BYTES: usize = 4;
pub(super) const ENCODINGS_FIELD: usize = ID_BYTES * (MAX_ENCODINGS + 1);

// Unit tests: the order of a pipeline's stages, the context they get, and
// what a pipeline refuses (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;

    /// Appends its tag when encoding; when decoding, checks and removes it,
    /// and refuses input without it. The context is appended too, and
    /// checked.
    struct Tag(u8);

    impl Encoding for Tag {
        fn id(&self) -> EncodingId {
            EncodingId(u32::from(self.0))
        }
        fn name(&self) -> &'static str {
            "tag"
        }
        fn overhead(&self, _: &str) -> usize {
            9
        }
        fn encode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
            out.extend_from_slice(input);
            out.push(self.0);
            out.extend_from_slice(&context.to_le_bytes());
            Ok(())
        }
        fn decode(&self, context: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
            let (rest, ctx) = input.split_at(input.len().saturating_sub(8));
            let mine = rest.split_last().filter(|&(&tag, _)| tag == self.0).filter(|_| ctx == context.to_le_bytes());
            let (_, data) = mine.ok_or_else(|| SqlError::new(Code::Corrupt, "not mine"))?;
            out.extend_from_slice(data);
            Ok(())
        }
    }

    fn tag(n: u8) -> Arc<dyn Encoding> {
        Arc::new(Tag(n))
    }

    /// Verifies: [DESIGN] RULES.md D30, pipeline: writing applies the stages
    /// in order and reading in reverse, each with the context; the empty
    /// pipeline returns its input without copying it; a stage's refusal is
    /// the pipeline's, as is decoding under another context.
    #[test]
    fn stages_run_in_order_and_back_in_reverse() {
        let pipeline = Pipeline::new(vec![tag(1), tag(2)]).unwrap();
        let encoded = pipeline.encode(7, b"data").unwrap();
        let ctx = 7_u64.to_le_bytes();
        let expected: Vec<u8> = [&b"data"[..], &[1], &ctx, &[2], &ctx].concat();
        assert_eq!(&*encoded, &expected[..], "tag 1 first, then tag 2");
        assert_eq!(&*pipeline.decode(7, &encoded).unwrap(), b"data");
        assert_eq!(pipeline.decode(8, &encoded).err().map(|e| e.code), Some(Code::Corrupt), "another context");
        let reversed = Pipeline::new(vec![tag(2), tag(1)]).unwrap();
        assert_eq!(reversed.decode(7, &encoded).err().map(|e| e.code), Some(Code::Corrupt), "stages in the wrong order");
        let none = Pipeline::none();
        assert!(matches!(none.encode(0, b"x").unwrap(), Cow::Borrowed(_)) && none.is_empty());
        assert_eq!(pipeline.ids(), vec![EncodingId(1), EncodingId(2)]);
    }

    /// Verifies: [DESIGN] RULES.md D30, pipeline: more than `MAX_ENCODINGS`
    /// stages, or one id twice, is refused with SQLCODE -910; resolving a
    /// file's ids finds each among the encodings given and refuses one that
    /// is missing; the ids field records and reads back the ids, and a
    /// count past `MAX_ENCODINGS` is malformed.
    #[test]
    fn pipelines_are_checked_and_recorded() {
        let five: Vec<Arc<dyn Encoding>> = (1..=5).map(tag).collect();
        assert_eq!(Pipeline::new(five).err().map(|e| e.code), Some(Code::Api));
        assert_eq!(Pipeline::new(vec![tag(1), tag(1)]).err().map(|e| e.code), Some(Code::Api));
        let path = Path::new("f");
        let available = [tag(3), tag(4)];
        let resolved = Pipeline::resolve(&[EncodingId(4), EncodingId(3)], &available, path).unwrap();
        assert_eq!(resolved.ids(), vec![EncodingId(4), EncodingId(3)]);
        assert_eq!(Pipeline::resolve(&[EncodingId(9)], &available, path).err().map(|e| e.code), Some(Code::Api));
        let field = ids_field(&resolved);
        assert_eq!(field.len(), ENCODINGS_FIELD);
        assert_eq!(read_ids_field(&field), Some(vec![EncodingId(4), EncodingId(3)]));
        assert_eq!(read_ids_field(&ids_field(&Pipeline::none())), Some(vec![]));
        let mut bad = field.clone();
        bad[0] = 5;
        assert_eq!(read_ids_field(&bad), None);
        assert_eq!(read_ids_field(&field[..3]), None);
    }
}
