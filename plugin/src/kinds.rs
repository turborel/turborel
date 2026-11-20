//! \[EXTENSION\] The trait of each kind of object a plug-in serves
//! (RULES.md D169, D220), but storage engines' and index methods'
//! (`storage.rs`). Each is called once a row, a value or a string of the
//! batch a CALL carries; what a call keeps between calls, an exchange, a
//! target or an upload, the SDK keeps by the number it gives the database.

use turborel::catalog::PartitionBound;
use turborel::fulltext::rank::{Corpus, InDocument, TermStats};
use turborel::partition::Restriction;
use turborel::value::Value;

use super::{Answer, Cx, Made, Told, Trained, Training};

/// Kind 1, a scalar function: a row's arguments, its value.
pub trait Scalar {
    /// The value of `args`, one row's.
    ///
    /// # Errors
    ///
    /// The function's.
    fn call(&mut self, cx: &mut Cx<'_>, args: &[Value]) -> Answer<Value>;
}

/// Kind 2, an aggregate function: its state its own bytes, which the
/// database keeps, empty at first.
pub trait Aggregate {
    /// `value` taken into `state`.
    ///
    /// # Errors
    ///
    /// The function's.
    fn step(&mut self, state: &mut Vec<u8>, value: &Value) -> Answer<()>;
    /// `other`, another group's state, taken into `state`.
    ///
    /// # Errors
    ///
    /// The function's.
    fn merge(&mut self, state: &mut Vec<u8>, other: &[u8]) -> Answer<()>;
    /// The result of `state`.
    ///
    /// # Errors
    ///
    /// The function's.
    fn finish(&mut self, state: &[u8]) -> Answer<Value>;
}

/// Kind 3, a table function: its rows of its arguments, and the queries it
/// asks the database to read first, whose rows it is then given.
pub trait TableFunction {
    /// The queries it reads, of `args`, a null where the argument is not
    /// known yet. Default: none.
    ///
    /// # Errors
    ///
    /// The function's.
    fn reads(&mut self, _args: &[Value]) -> Answer<Vec<String>> {
        Ok(Vec::new())
    }
    /// Its rows of `args`, given the rows of each query it reads.
    ///
    /// # Errors
    ///
    /// The function's.
    fn rows(&mut self, cx: &mut Cx<'_>, args: &[Value], read: &[Vec<Vec<Value>>]) -> Answer<Vec<Vec<Value>>>;
}

/// Kind 4, a procedure: run with its arguments, its SQL through `cx`.
pub trait Procedure {
    /// Runs it.
    ///
    /// # Errors
    ///
    /// The procedure's, or its SQL's.
    fn call(&mut self, cx: &mut Cx<'_>, args: &[Value]) -> Answer<()>;
}

/// Kind 5, a vector metric: the distance of two vectors' coordinates.
pub trait Metric {
    /// The distance from `query` to `candidate`, none for null.
    ///
    /// # Errors
    ///
    /// The metric's.
    fn distance(&mut self, query: &[f64], candidate: &[f64]) -> Answer<Option<f64>>;
}

/// Kind 6, an embedding model: a text's vector.
pub trait Embedder {
    /// The coordinates of `text`'s vector.
    ///
    /// # Errors
    ///
    /// The model's.
    fn embed(&mut self, text: &str) -> Answer<Vec<f64>>;
}

/// Who connects, as an authenticator is told.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Who {
    /// The user named.
    pub user: String,
    /// The peer's address.
    pub peer: String,
    /// Whether the connection is local.
    pub local: bool,
    /// Whether it is encrypted.
    pub secure: bool,
}

/// What a step of an exchange answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Bytes for the client, whose answer is the next step's.
    Challenge(Vec<u8>),
    /// The user is proven.
    Accept,
    /// The user is not, why.
    Refuse(String),
}

/// One exchange of an authenticator.
pub trait Exchange {
    /// The step answering the client's `bytes`: empty at the first.
    ///
    /// # Errors
    ///
    /// The authenticator's.
    fn step(&mut self, bytes: &[u8]) -> Answer<Step>;
}

/// Kind 7, an authenticator.
pub trait Authenticator {
    /// An exchange for `who`.
    ///
    /// # Errors
    ///
    /// The authenticator's.
    fn begin(&mut self, who: &Who) -> Answer<Box<dyn Exchange>>;
}

/// An object being written to a backup target.
pub trait Upload {
    /// `bytes` appended.
    ///
    /// # Errors
    ///
    /// The target's.
    fn write(&mut self, bytes: &[u8]) -> Answer<()>;
    /// The object written whole, durable.
    ///
    /// # Errors
    ///
    /// The target's.
    fn finish(self: Box<Self>) -> Answer<()>;
}

/// A backup target opened at a location.
pub trait TargetStore {
    /// An upload of `object` begun.
    ///
    /// # Errors
    ///
    /// The target's.
    fn begin(&mut self, object: &str) -> Answer<Box<dyn Upload>>;
    /// At most `max` bytes of `object` from `offset`, empty past its end.
    ///
    /// # Errors
    ///
    /// The target's.
    fn read(&mut self, object: &str, offset: u64, max: u32) -> Answer<Vec<u8>>;
    /// The objects whose names begin `prefix`.
    ///
    /// # Errors
    ///
    /// The target's.
    fn list(&mut self, prefix: &str) -> Answer<Vec<String>>;
    /// `object` removed.
    ///
    /// # Errors
    ///
    /// The target's.
    fn remove(&mut self, object: &str) -> Answer<()>;
}

/// Kind 8, a backup target's scheme.
pub trait Target {
    /// The target at `location`.
    ///
    /// # Errors
    ///
    /// The scheme's.
    fn open(&mut self, location: &str) -> Answer<Box<dyn TargetStore>>;
}

/// Kind 9, a partitioning strategy.
pub trait PartitionStrategy {
    /// That a key of `types`, their names, is divided by it.
    ///
    /// # Errors
    ///
    /// The strategy's, SQLSTATE 42P16 by custom.
    fn check_key(&mut self, types: &[String]) -> Answer<()>;
    /// `bound`, of a key of `types`, checked and normalized.
    ///
    /// # Errors
    ///
    /// The strategy's.
    fn bound(&mut self, types: &[String], bound: PartitionBound) -> Answer<PartitionBound>;
    /// Whether `a` and `b` hold a key both.
    ///
    /// # Errors
    ///
    /// The strategy's.
    fn overlaps(&mut self, a: &PartitionBound, b: &PartitionBound) -> Answer<bool>;
    /// Whether `bound` holds `key`.
    ///
    /// # Errors
    ///
    /// The strategy's.
    fn contains(&mut self, bound: &PartitionBound, key: &[Value]) -> Answer<bool>;
    /// The bound of `bounds` holding `key`. Default: the first that does.
    ///
    /// # Errors
    ///
    /// As `contains`.
    fn route(&mut self, bounds: &[PartitionBound], key: &[Value]) -> Answer<Option<usize>> {
        for (at, b) in bounds.iter().enumerate() {
            if self.contains(b, key)? {
                return Ok(Some(at));
            }
        }
        Ok(None)
    }
    /// Whether `bound` may hold a row meeting `restrictions`, one a key's
    /// column. Default: yes.
    ///
    /// # Errors
    ///
    /// The strategy's.
    fn admits(&mut self, _bound: &PartitionBound, _restrictions: &[Option<Restriction>]) -> Answer<bool> {
        Ok(true)
    }
}

/// Kind 10, a ranking of full-text search: as `turborel::fulltext::rank::
/// Ranking`, deterministic.
pub trait Ranking {
    /// The weight of `term` in a document, `in_doc`, of `corpus`.
    fn weight(&mut self, corpus: &Corpus, term: &TermStats, in_doc: &InDocument) -> f64;
    /// The most `weight` gives `term` in any document of `corpus`.
    fn bound(&mut self, corpus: &Corpus, term: &TermStats) -> f64;
}

/// Kind 11, a vector quantizer: a product codebook trained of a sample.
pub trait Quantizer {
    /// The codebook of `training`.
    ///
    /// # Errors
    ///
    /// The quantizer's: SQLSTATE 22023 of a code length it cannot make.
    fn train(&mut self, training: &Training) -> Answer<Trained>;
}

/// Kind 12, a collation: a string's sort key, bytes the database compares.
pub trait Collation {
    /// The sort key of `text`, its trailing spaces weighed if not `pad`.
    ///
    /// # Errors
    ///
    /// The collation's.
    fn sort_key(&mut self, text: &str, pad: bool) -> Answer<Vec<u8>>;
}

/// Kind 39, a language of full-text search (RULES.md D328): what its
/// manifest SERVES of cutting text into words, stemming words and its stop
/// words, each called once a document or a word of the batch a CALL
/// carries. A part it does not serve is the built-in language's it is
/// LIKE, so it implements only what it serves; deterministic.
pub trait Language {
    /// The words of `text`, each the offset of its first character and its
    /// characters, in the order of their offsets. By default no word.
    ///
    /// # Errors
    ///
    /// The language's.
    fn segment(&mut self, _text: &str) -> Answer<Vec<(u32, u32)>> {
        Ok(Vec::new())
    }
    /// The stem of `word`, normalized as the database normalizes its
    /// words. By default the word.
    ///
    /// # Errors
    ///
    /// The language's.
    fn stem(&mut self, word: &str) -> Answer<String> {
        Ok(word.to_owned())
    }
    /// Its stop words. By default none.
    ///
    /// # Errors
    ///
    /// The language's.
    fn stop_words(&mut self) -> Answer<Vec<String>> {
        Ok(Vec::new())
    }
}

/// Kind 13, a system table of INFORMATION_SCHEMA.
pub trait SystemTable {
    /// Its rows as `user` sees them, the database's owner if `owner`.
    ///
    /// # Errors
    ///
    /// The table's.
    fn rows(&mut self, user: &str, owner: bool) -> Answer<Vec<Vec<Value>>>;
}

/// Kind 14, an XQuery function of atomic values.
pub trait XQueryFunction {
    /// Its value of `args`, each its parameter's SQL type, null of an empty
    /// sequence; null gives an empty sequence.
    ///
    /// # Errors
    ///
    /// The function's.
    fn call(&mut self, args: &[Value]) -> Answer<Value>;
}

/// Kind 15, a type: its values made of their text and binary forms. Each
/// value made is three byte strings, a key the database compares, orders
/// and indexes by as bytes, the binary form and the text form, so the
/// program is asked only as a value is made (RULES.md D223).
///
/// ```
/// use turborel_plugin::{Answer, Harness, Made, Plugin, PluginError, Type};
///
/// /// A temperature in whole kelvins, written `273K`.
/// struct Kelvin;
///
/// impl Type for Kelvin {
///     fn input(&mut self, text: &str) -> Answer<Made> {
///         let n: u32 = text.strip_suffix('K').and_then(|n| n.parse().ok()).ok_or_else(|| PluginError::new("22P02", text))?;
///         Ok(Made { key: n.to_be_bytes().to_vec(), binary: n.to_le_bytes().to_vec(), text: format!("{n}K") })
///     }
///     fn receive(&mut self, bytes: &[u8]) -> Answer<Made> {
///         let n = u32::from_le_bytes(bytes.try_into().map_err(|_| PluginError::new("22P03", "four bytes"))?);
///         self.input(&format!("{n}K"))
///     }
/// }
///
/// let mut h = Harness::new(Plugin::new("1.0.0").data_type("KELVIN", Kelvin));
/// assert_eq!(h.input("KELVIN", "273K").unwrap().key, [0, 0, 1, 17]);
/// assert_eq!(h.input("KELVIN", "hot").unwrap_err().state, "22P02");
/// ```
pub trait Type {
    /// The value of `text`.
    ///
    /// # Errors
    ///
    /// SQLSTATE 22P02 by custom, of a text no value's.
    fn input(&mut self, text: &str) -> Answer<Made>;
    /// The value of the binary form `bytes`.
    ///
    /// # Errors
    ///
    /// As `input`.
    fn receive(&mut self, bytes: &[u8]) -> Answer<Made>;
}

/// Kind 18, what makes users' verifiers.
pub trait Credentials {
    /// The verifier of `user`'s `password`, kept by the database.
    ///
    /// # Errors
    ///
    /// The plug-in's.
    fn verifier(&mut self, user: &str, password: &str) -> Answer<Vec<u8>>;
}

/// Kind 19, a sink of the database's events.
pub trait Diagnostics {
    /// `events`, and `dropped`, those let go since the last call.
    ///
    /// # Errors
    ///
    /// The sink's: the events are counted dropped.
    fn events(&mut self, events: &[Told], dropped: u64) -> Answer<()>;
}
