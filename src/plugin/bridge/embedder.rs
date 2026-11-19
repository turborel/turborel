//! \[EXTENSION\] A plug-in's embedding model (RULES.md D169), an
//! `Embedder` whose batch of texts is one CALL, answered by a vector each,
//! which is taken in the model's format and layout. Its time is bounded by
//! PLUGINS.CALL_TIMEOUT_MS, as a model's own time is its to bound.

use crate::bytes::Put;
use crate::error::Result;
use crate::plugin::errors::violation;
use crate::plugin::manifest::Declared;
use crate::plugin::payload::EMBED;
use crate::plugin::values::values;
use crate::vector::embed::{Embedder, ModelShape};
use crate::vector::{Element, Layout, Vector, vector_of};

use super::{Link, answer, counted};

/// The texts a CALL carries when the manifest says no BATCH.
pub const DEFAULT_BATCH: usize = 64;

/// A plug-in's embedding model.
#[derive(Debug)]
pub struct PluginEmbedder {
    link: Link,
    shape: ModelShape,
    deterministic: bool,
    batch: usize,
}

impl PluginEmbedder {
    /// The model `d` declares.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T06 of no DIMENSIONS, or an ELEMENT no format's.
    pub fn new(d: &Declared, link: Link) -> Result<PluginEmbedder> {
        let dims = d.dimensions.ok_or_else(|| crate::plugin::errors::invalid_manifest("an EMBEDDING MODEL needs DIMENSIONS"))?;
        let element = d.element.as_deref().and_then(Element::named).unwrap_or(Element::Float32);
        let batch = d.batch.and_then(|b| usize::try_from(b).ok()).unwrap_or(DEFAULT_BATCH);
        let shape = ModelShape { dims, element, layout: Layout::Dense };
        Ok(PluginEmbedder { link, shape, deterministic: d.deterministic, batch: batch.max(1) })
    }
}

impl Embedder for PluginEmbedder {
    fn name(&self) -> &str {
        &self.link.object
    }
    fn shape(&self) -> ModelShape {
        self.shape
    }
    fn deterministic(&self) -> bool {
        self.deterministic
    }
    fn batch(&self) -> usize {
        self.batch
    }
    fn embed(&self, texts: &[&str]) -> Result<Vec<Vector>> {
        let mut payload = Vec::new();
        payload.put_u32(u32::try_from(texts.len()).unwrap_or(u32::MAX));
        for t in texts {
            payload.put_str(t);
        }
        let got = self.link.plain(EMBED, payload)?;
        let made = counted(answer(&got, values)?, texts.len())?;
        made.iter()
            .map(|v| {
                let v = vector_of(v).ok_or_else(|| violation("an embedding is a vector"))?;
                if v.dims != self.shape.dims {
                    return Err(violation(&format!("an embedding has {} dimensions, not {}", v.dims, self.shape.dims)));
                }
                Vector::of(v).converted(self.shape.element)
            })
            .collect()
    }
}
