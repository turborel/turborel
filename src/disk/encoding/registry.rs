//! \[EXTENSION\] The encodings a database has registered (RULES.md D30),
//! shared with the engines that pass their tables' data through those a
//! table's options name (RULES.md D41); and data sealed: passed through a
//! pipeline and headed by its ids, so it is read back through the same
//! encodings whatever the table's options are by then.
//!
//! ```text
//! sealed:  count u8 | an id for each encoding, u32, little-endian | the
//!          bytes, passed through those encodings in order
//! ```
//!
//! Split from disk/encoding.rs (RULES.md R-SIZE-2).

use std::borrow::Cow;
use std::sync::{Arc, RwLock};

use super::{Bound, Encoding, EncodingId, ID_BYTES, MAX_ENCODINGS, Pipeline};
use crate::error::{Code, Result, SqlError};
use crate::sync::{read, write};

/// \[EXTENSION\] The encodings registered with a database
/// (`Database::register_encoding`), each found by its name or its id.
#[derive(Default)]
pub struct Encodings {
    registered: RwLock<Vec<Arc<dyn Encoding>>>,
}

impl std::fmt::Debug for Encodings {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let names: Vec<&str> = self.all().unwrap_or_default().iter().map(|e| e.name()).collect();
        f.debug_tuple("Encodings").field(&names).finish()
    }
}

fn api(message: String) -> SqlError {
    SqlError::new(Code::Api, message)
}

fn corrupt(why: &str) -> SqlError {
    SqlError::new(Code::Corrupt, format!("[DESIGN] sealed data is malformed: {why}"))
}

impl Encodings {
    /// Registers `encoding`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one of its name or id is registered.
    pub(crate) fn add(&self, encoding: Arc<dyn Encoding>) -> Result<()> {
        let mut registered = write(&self.registered)?;
        let name = encoding.name();
        if registered.iter().any(|e| e.name().eq_ignore_ascii_case(name) || e.id() == encoding.id()) {
            return Err(api(format!("[DESIGN] an encoding of the name {name}, or of its id, is registered")));
        }
        registered.push(encoding);
        Ok(())
    }

    /// Every encoding registered.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the registry was poisoned.
    pub fn all(&self) -> Result<Vec<Arc<dyn Encoding>>> {
        Ok(read(&self.registered)?.clone())
    }

    /// The encodings `names` names, by commas, each `NAME` or
    /// `NAME:ARGUMENT`, as they are registered, with their arguments as
    /// written; the first not registered, if one is not.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the registry was poisoned, or an encoding refuses its
    /// argument (`Encoding::accepts`).
    pub fn canonical(&self, names: &str) -> Result<std::result::Result<String, String>> {
        let registered = read(&self.registered)?;
        let mut out = Vec::new();
        for (name, argument) in written(names) {
            let Some(found) = registered.iter().find(|e| e.name().eq_ignore_ascii_case(name)) else { return Ok(Err(name.to_owned())) };
            found.accepts(argument)?;
            out.push(if argument.is_empty() { found.name().to_owned() } else { format!("{}:{argument}", found.name()) });
        }
        Ok(Ok(out.join(",")))
    }

    /// The pipeline of the encodings `names` names, as `canonical` reads
    /// them, in order, each bound to its argument.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if one is not registered, refuses its argument, or the
    /// pipeline is refused (`Pipeline::new`).
    pub fn named(&self, names: &str) -> Result<Pipeline> {
        let registered = read(&self.registered)?;
        let mut stages = Vec::new();
        for (name, argument) in written(names) {
            let found = registered.iter().find(|e| e.name().eq_ignore_ascii_case(name));
            let found = found
                .ok_or_else(|| api(format!("[DESIGN] no encoding {name} is registered: register it before the database is opened")))?;
            stages.push(Bound::bind(Arc::clone(found), argument)?);
        }
        Pipeline::new(stages)
    }

    /// Every encoding registered, those of `pipeline` as it binds them: the
    /// encodings a file written through `pipeline`, or none, is read and
    /// written through.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if the registry was poisoned.
    pub fn for_files(&self, pipeline: &Pipeline) -> Result<Vec<Arc<dyn Encoding>>> {
        let all = self.all()?;
        Ok(all.into_iter().map(|e| pipeline.stages().iter().find(|s| s.id() == e.id()).map_or(e, Arc::clone)).collect())
    }

    /// The pipeline of the encodings of `ids`, in order.
    fn of_ids(&self, ids: &[EncodingId]) -> Result<Pipeline> {
        let registered = read(&self.registered)?;
        let stages = ids
            .iter()
            .map(|id| {
                let found = registered.iter().find(|e| e.id() == *id).map(Arc::clone);
                found.ok_or_else(|| api(format!("[DESIGN] data is encoded with encoding {}, which is not registered", id.0)))
            })
            .collect::<Result<Vec<_>>>()?;
        Pipeline::new(stages)
    }

    /// `sealed`, written by `Pipeline::seal` with `context`, passed back
    /// through the encodings it names: borrowed if it names none.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if it is malformed, or an encoding refuses it; -910 if
    /// it names one not registered.
    pub fn unseal<'a>(&self, context: u64, sealed: &'a [u8]) -> Result<Cow<'a, [u8]>> {
        let (&count, rest) = sealed.split_first().ok_or_else(|| corrupt("it is empty"))?;
        let count = usize::from(count);
        if count > MAX_ENCODINGS {
            return Err(corrupt("it names too many encodings"));
        }
        let (ids, body) = rest.split_at_checked(count.saturating_mul(ID_BYTES)).ok_or_else(|| corrupt("it is too short"))?;
        if count == 0 {
            return Ok(Cow::Borrowed(body));
        }
        let ids: Vec<EncodingId> = ids.as_chunks::<ID_BYTES>().0.iter().map(|b| EncodingId(u32::from_le_bytes(*b))).collect();
        let decoded = self.of_ids(&ids)?.decode(context, body)?;
        Ok(Cow::Owned(decoded.into_owned()))
    }
}

/// The encodings `names` names, by commas: each name, and its argument
/// after a colon, or none.
fn written(names: &str) -> impl Iterator<Item = (&str, &str)> {
    names
        .split(',')
        .map(str::trim)
        .filter(|n| !n.is_empty())
        .map(|n| n.split_once(':').map_or((n, ""), |(name, arg)| (name.trim(), arg.trim())))
}

impl Pipeline {
    /// Appends `plain` sealed to `out`: the pipeline's ids, then `plain`
    /// passed through it with `context`; read back by `Encodings::unseal`.
    ///
    /// # Errors
    ///
    /// An encoding's.
    pub fn seal(&self, context: u64, plain: &[u8], out: &mut Vec<u8>) -> Result<()> {
        let ids = self.ids();
        out.push(u8::try_from(ids.len()).unwrap_or(u8::MAX));
        for id in &ids {
            out.extend_from_slice(&id.0.to_le_bytes());
        }
        out.extend_from_slice(&self.encode(context, plain)?);
        Ok(())
    }
}

// Unit tests of sealing (RULES.md R-TEST-1, R-TEST-2).
#[cfg(test)]
mod tests {
    use super::*;

    /// Reverses its input; refuses to decode input that begins with 0.
    struct Reverse;

    impl Encoding for Reverse {
        fn id(&self) -> EncodingId {
            EncodingId(7)
        }
        fn name(&self) -> &'static str {
            "Reverse"
        }
        fn overhead(&self, _: &str) -> usize {
            0
        }
        fn encode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
            out.extend(input.iter().rev());
            Ok(())
        }
        fn decode(&self, _: u64, input: &[u8], out: &mut Vec<u8>) -> Result<()> {
            if input.first() == Some(&0) {
                return Err(corrupt("not mine"));
            }
            out.extend(input.iter().rev());
            Ok(())
        }
    }

    /// Verifies: [EXTENSION] RULES.md D30, D41, sealing: data sealed
    /// through a pipeline names its encodings and is read back through
    /// them, whatever pipeline is current; sealed through none it is
    /// borrowed back as it is; names resolve in any case, and one not
    /// registered is refused (SQLCODE -910), as is a name or id given twice.
    #[test]
    fn sealed_data_names_its_encodings() {
        let encodings = Encodings::default();
        encodings.add(Arc::new(Reverse)).unwrap();
        assert_eq!(encodings.add(Arc::new(Reverse)).err().map(|e| e.code), Some(Code::Api));
        let pipeline = encodings.named(" reverse ").unwrap();
        let mut sealed = Vec::new();
        pipeline.seal(3, b"abc", &mut sealed).unwrap();
        assert_eq!(sealed, [&[1_u8][..], &7_u32.to_le_bytes(), b"cba"].concat());
        assert_eq!(&*encodings.unseal(3, &sealed).unwrap(), b"abc");
        let mut plain = Vec::new();
        Pipeline::none().seal(3, b"abc", &mut plain).unwrap();
        assert!(matches!(encodings.unseal(3, &plain).unwrap(), Cow::Borrowed(b"abc")));
        assert_eq!(encodings.canonical("REVERSE,").unwrap(), Ok("Reverse".to_owned()));
        assert_eq!(encodings.canonical("reverse,lz9").unwrap(), Err("lz9".to_owned()));
        assert_eq!(encodings.named("lz9").err().map(|e| e.code), Some(Code::Api));
        assert_eq!(encodings.all().unwrap().len(), 1);
    }

    /// Verifies: [EXTENSION] RULES.md D30, sealing, robustness: sealed data
    /// empty, naming more than `MAX_ENCODINGS` or more ids than it holds,
    /// or refused by its encoding, is malformed (SQLCODE -916); one naming
    /// an encoding not registered is refused (SQLCODE -910).
    #[test]
    fn malformed_sealed_data_is_refused() {
        let encodings = Encodings::default();
        encodings.add(Arc::new(Reverse)).unwrap();
        let code = |bytes: &[u8]| encodings.unseal(0, bytes).err().map(|e| e.code);
        assert_eq!(code(b""), Some(Code::Corrupt));
        assert_eq!(code(&[5]), Some(Code::Corrupt));
        assert_eq!(code(&[1, 7, 0]), Some(Code::Corrupt));
        assert_eq!(code(&[1, 7, 0, 0, 0, 0]), Some(Code::Corrupt));
        assert_eq!(code(&[1, 8, 0, 0, 0, 1]), Some(Code::Api));
    }
}
