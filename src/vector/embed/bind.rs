//! \[EXTENSION\] A model of the catalog bound to its plug-in (§12.1).
//!
//! The database reads VERSION, BATCH_SIZE, ON_ERROR and MAX_RETRIES
//! itself and gives the plug-in every other option, FROM FILE's path as
//! FILE (`Embedder::configured`). DIMENSIONS, FORMAT and LAYOUT, if
//! written, are what the configured model's vectors must be (VEC-0031).
//! A model is bound once for what it was made of, and again only once
//! that changes or its plug-in is registered. A model whose plug-in is not
//! registered, as recovery makes the catalog again before the host
//! registers it, is bound to a stand-in of the shape it was made of,
//! which refuses to embed (VEC-0030), so what names it is defined
//! again.

use std::sync::Arc;

use super::{BoundModel, DEFAULT_BATCH, Embedder, Embedders, ModelShape, OnError};
use crate::catalog::{ModelDesc, ModelOption};
use crate::error::{Code, Result, SqlError};
use crate::sync::lock;
use crate::vector::{Element, Layout};

/// The options the database reads itself.
pub const DATABASE_OPTIONS: [&str; 5] = [VERSION, BATCH_SIZE, ON_ERROR, MAX_RETRIES, SHAPE];
/// The option of the model's version (R12.5).
pub const VERSION: &str = "VERSION";
/// The option of the most texts given the plug-in at once (R12.3).
pub const BATCH_SIZE: &str = "BATCH_SIZE";
/// The option of what a failure does (R12.4).
pub const ON_ERROR: &str = "ON_ERROR";
/// The option of how many times RETRY asks again.
pub const MAX_RETRIES: &str = "MAX_RETRIES";
/// The option of the vectors' dimensions.
pub const DIMENSIONS: &str = "DIMENSIONS";
/// The option of their element format.
pub const FORMAT: &str = "FORMAT";
/// The option of their layout.
pub const LAYOUT: &str = "LAYOUT";
/// The option of the shape a model was made of, as its dimensions, element
/// format and layout, written by the database in the definition it logs,
/// so recovery makes it again before the host registers its plug-in.
pub const SHAPE: &str = "SHAPE";
/// The option FROM FILE is given the plug-in as.
pub const FILE: &str = "FILE";
/// \[DESIGN\] MAX_RETRIES left out.
const DEFAULT_RETRIES: u32 = 3;
/// A plug-in configured, and the version, batch size and ON_ERROR the
/// database read of its options.
pub type Configured = (Arc<dyn Embedder>, (u32, usize, OnError));
/// \[DESIGN\] The most bindings kept: one each of as many models.
const MOST_BOUND: usize = 256;

fn refused(detail: impl std::fmt::Display) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[EXTENSION] {detail}"))
}

/// The whole number `option` gives.
///
/// # Errors
///
/// SQLCODE -103 if it is not one of 0 to 2^32 - 1.
pub fn number_option(option: &ModelOption) -> Result<u32> {
    let given = (!option.quoted).then(|| option.value.parse::<u32>().ok()).flatten();
    given.ok_or_else(|| refused(format!("option {} is a whole number, not {}", option.name, option.value)))
}

/// The shape `options` declare of a model of `shape`, if each of them is
/// DIMENSIONS, FORMAT or LAYOUT and agrees with it: what a plug-in that
/// takes no options of its own takes.
///
/// # Errors
///
/// SQLCODE -103 for another option; VEC-0031, -103, for one that does
/// not agree.
pub fn configure_shape(shape: ModelShape, options: &[ModelOption]) -> Result<ModelShape> {
    for o in options {
        if !shape_option(o, shape)? {
            return Err(refused(format!("the model takes no option {}", o.name)));
        }
    }
    Ok(shape)
}

/// Whether `o` is DIMENSIONS, FORMAT or LAYOUT, if it agrees with `shape`.
///
/// # Errors
///
/// VEC-0031, SQLCODE -103, if it does not agree.
fn shape_option(o: &ModelOption, shape: ModelShape) -> Result<bool> {
    let agrees = match o.name.as_str() {
        DIMENSIONS => number_option(o)? == shape.dims,
        FORMAT => Element::named(&o.value.to_ascii_uppercase()) == Some(shape.element),
        LAYOUT => Layout::named(&o.value.to_ascii_uppercase()) == Some(shape.layout),
        _ => return Ok(false),
    };
    if agrees {
        return Ok(true);
    }
    let made = format!("VECTOR({}, {}, {})", shape.dims, shape.element.name(), shape.layout.name());
    Err(refused(format!("VEC-0031 the model's vectors are {made}, not of {} {}", o.name, o.value)))
}

/// What the database reads of `options`: VERSION, if given; BATCH_SIZE,
/// else `batch`; ON_ERROR, else `on_error`.
///
/// # Errors
///
/// SQLCODE -103 for a value it refuses.
fn own_options(options: &[ModelOption], batch: usize, on_error: OnError) -> Result<(Option<u32>, usize, OnError)> {
    let find = |name: &str| options.iter().find(|o| o.name == name);
    let version = find(VERSION).map(number_option).transpose()?;
    let batch = match find(BATCH_SIZE).map(number_option).transpose()? {
        Some(0) => return Err(refused(format!("{BATCH_SIZE} is 1 or more"))),
        Some(n) => usize::try_from(n).unwrap_or(usize::MAX),
        None => batch,
    };
    let retries = find(MAX_RETRIES).map(number_option).transpose()?.unwrap_or(DEFAULT_RETRIES);
    let on_error = match find(ON_ERROR).map(|o| o.value.to_ascii_uppercase()).as_deref() {
        None => on_error,
        Some("FAIL") => OnError::Fail,
        Some("NULL") => OnError::Null,
        Some("RETRY") => OnError::Retry(retries),
        Some(other) => return Err(refused(format!("{ON_ERROR} is 'FAIL', 'NULL' or 'RETRY', not {other}"))),
    };
    Ok((version, batch, on_error))
}

/// A fingerprint of a model's octets, so a binding is known again
/// without keeping them twice.
fn fingerprint(blob: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    blob.hash(&mut hasher);
    hasher.finish()
}

/// The stand-in of the plug-in `name`, not registered, of a model of
/// `shape`.
struct Unregistered {
    name: String,
    shape: ModelShape,
}

impl Embedder for Unregistered {
    fn name(&self) -> &str {
        &self.name
    }
    fn shape(&self) -> ModelShape {
        self.shape
    }
    fn embed(&self, _texts: &[&str]) -> Result<Vec<crate::vector::Vector>> {
        Err(refused(format!("VEC-0030 no model plug-in {} is registered", self.name)))
    }
}

impl Embedders {
    /// The plug-in `plugin` configured by `options` and FROM FILE's `file`,
    /// as a model of the catalog made of them is, with its version, batch
    /// and failure as the options say.
    ///
    /// # Errors
    ///
    /// VEC-0030, SQLCODE -103, if no plug-in `plugin` is registered; -103
    /// for an option the database or the plug-in refuses; VEC-0031 if the
    /// model's vectors are not of the DIMENSIONS, FORMAT and LAYOUT given.
    pub fn configure(&self, plugin: &str, (file, blob): (Option<&str>, Option<&[u8]>), options: &[ModelOption]) -> Result<Configured> {
        let found = self.named(plugin)?.ok_or_else(|| refused(format!("VEC-0030 no model plug-in {plugin} is registered")))?;
        let mut given: Vec<ModelOption> = options.iter().filter(|o| !DATABASE_OPTIONS.contains(&o.name.as_str())).cloned().collect();
        if let Some(path) = file {
            given.push(ModelOption { name: FILE.to_owned(), value: path.to_owned(), quoted: true });
        }
        let embedder = match blob {
            Some(blob) => found.loaded(blob, &given)?,
            None => found.configured(&given)?,
        };
        let embedder = embedder.unwrap_or(found);
        for o in &given {
            shape_option(o, embedder.shape())?;
        }
        let (version, batch, on_error) = own_options(options, embedder.batch().max(1), embedder.on_error())?;
        let version = version.unwrap_or_else(|| embedder.version());
        Ok((embedder, (version, batch, on_error)))
    }

    /// The model `desc` describes, bound to its plug-in: as it was bound
    /// last, if it has not changed since.
    ///
    /// # Errors
    ///
    /// As `configure`; VEC-0031 if the model's vectors are no longer of
    /// the shape it was made of; SQLCODE -910 if the bindings were
    /// poisoned.
    pub fn bind(&self, desc: &ModelDesc) -> Result<Arc<BoundModel>> {
        let registered = self.named(&desc.plugin)?.is_some();
        let blob = desc.blob.as_ref().map(|b| (b.len(), fingerprint(b)));
        let key = format!("{:?}|{:?}|{:?}|{blob:?}|{:?}|{:?}|{registered}", desc.name, desc.plugin, desc.file, desc.options, desc.version);
        if let Some((_, bound)) = lock(&self.bound)?.iter().find(|(k, _)| *k == key) {
            return Ok(Arc::clone(bound));
        }
        let made = ModelShape { dims: desc.dims, element: desc.element, layout: desc.layout };
        let (embedder, settings) = if registered {
            self.configure(&desc.plugin, (desc.file.as_deref(), desc.blob.as_deref()), &desc.options)?
        } else {
            let stand_in: Arc<dyn Embedder> = Arc::new(Unregistered { name: desc.plugin.clone(), shape: made });
            let (version, batch, on_error) = own_options(&desc.options, DEFAULT_BATCH, OnError::Fail)?;
            (stand_in, (version.unwrap_or(0), batch, on_error))
        };
        let shape = embedder.shape();
        if shape != made {
            return Err(refused(format!(
                "VEC-0031 the plug-in {} no longer gives vectors of the shape model {} was made of",
                desc.plugin, desc.name
            )));
        }
        let bound = Arc::new(BoundModel::new(desc.name.clone(), embedder, settings));
        let mut all = lock(&self.bound)?;
        all.retain(|(k, b)| !k.starts_with(&format!("{:?}|", desc.name)) || Arc::strong_count(b) > 1);
        if all.len() >= MOST_BOUND {
            all.remove(0);
        }
        all.push((key, Arc::clone(&bound)));
        Ok(bound)
    }
}
