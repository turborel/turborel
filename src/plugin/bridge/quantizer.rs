//! \[EXTENSION\] A plug-in's quantizer (RULES.md D221), a `Quantizer`
//! whose training is one CALL of the whole sample, answered by a product
//! codebook: centroids in subspaces, which the database encodes, decodes
//! and measures codes by itself, as PQ's (`vector/pq.rs`). So a vector
//! index's build and search make no call, a codebook kept is read without
//! the program, and a failing program fails only the training.

use crate::error::Result;
use crate::plugin::payload::QUANTIZER_TRAIN;
use crate::plugin::payload::quantizer::{Training, put_training, trained};
use crate::vector::quantizer::{Codebook, Quantizer};
use crate::vector::{ProductQuantizer, product_codebook};

use super::{Link, answer};

/// A plug-in's quantizer.
#[derive(Debug)]
pub struct PluginQuantizer {
    link: Link,
}

impl PluginQuantizer {
    /// The quantizer `link` names.
    pub fn new(link: Link) -> PluginQuantizer {
        PluginQuantizer { link }
    }
}

impl Quantizer for PluginQuantizer {
    fn name(&self) -> &str {
        &self.link.object
    }
    fn train(
        &self,
        sample: &mut dyn Iterator<Item = Result<Vec<f32>>>,
        dims: u32,
        code_bytes: usize,
        seed: u64,
    ) -> Result<Box<dyn Codebook>> {
        let sample = crate::vector::quantizer::collect(sample, dims)?;
        let code_bytes = u32::try_from(code_bytes).unwrap_or(u32::MAX);
        let mut payload = Vec::new();
        put_training(&mut payload, &Training { dims, code_bytes, seed, sample });
        let got = answer(&self.link.plain(QUANTIZER_TRAIN, payload)?, trained)?;
        let asked = usize::try_from(code_bytes).unwrap_or(usize::MAX);
        if code_bytes != 0 && got.widths.len() != asked {
            return Err(crate::plugin::errors::violation(&format!("it trained {} subspaces, not {code_bytes}", got.widths.len())));
        }
        let widths: Vec<usize> = got.widths.iter().map(|w| usize::try_from(*w).unwrap_or(usize::MAX)).collect();
        product_codebook(dims, usize::try_from(got.k).unwrap_or(usize::MAX), &widths, got.centroids)
    }
    fn read(&self, bytes: &[u8]) -> Result<Box<dyn Codebook>> {
        ProductQuantizer.read(bytes)
    }
}
