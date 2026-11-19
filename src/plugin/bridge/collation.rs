//! \[EXTENSION\] A plug-in's collation (RULES.md D221), a `Collation`
//! whose strings' sort keys are made a batch of strings at a time, one
//! CALL (`sort_keys`), and compared by the database as bytes: no
//! comparison is a call. A projection's sort keys of a batch of rows are
//! one call (`exec/batch/project.rs`); a key asked of one string alone is
//! a call of its own.
//!
//! As its keys are calls, which may fail and are never made under a
//! lock, no index keys by it, and LIKE, TRIM and XQuery, which compare a
//! character or a substring at a time, refuse it (SQLCODE -103).

use crate::error::Result;
use crate::plugin::payload::COLLATION_KEYS;
use crate::plugin::payload::collation::{answered, put_keys};
use crate::value::collation::{Collation, Pad};

use super::{Link, answer, counted};

/// A plug-in's collation.
#[derive(Debug)]
pub struct PluginCollation {
    link: Link,
    version: String,
    batch: usize,
}

/// The strings a CALL carries when the manifest says no BATCH.
pub const DEFAULT_BATCH: usize = 1024;

impl PluginCollation {
    /// The collation `link` names, of `version`, `batch` strings a call.
    pub fn new(link: Link, version: String, batch: Option<u32>) -> PluginCollation {
        let batch = batch.and_then(|b| usize::try_from(b).ok()).unwrap_or(DEFAULT_BATCH).max(1);
        PluginCollation { link, version, batch }
    }
}

impl Collation for PluginCollation {
    fn name(&self) -> &str {
        &self.link.object
    }
    fn version(&self) -> &str {
        &self.version
    }
    fn sort_key(&self, text: &str, pad: Pad, out: &mut Vec<u8>) {
        // Reached only where a failure cannot be told: those places refuse
        // a plug-in's collation first (`character_wise`), so a failure
        // leaves the key empty there and nowhere else.
        if let Ok(mut keys) = self.sort_keys(&[text], pad) {
            out.append(keys.first_mut().unwrap_or(&mut Vec::new()));
        }
    }
    fn fold(&self, _c: char) -> Option<char> {
        None
    }
    fn weighs(&self, _c: char) -> bool {
        false
    }
    fn level_weights(&self, text: &str, level: usize, out: &mut Vec<u8>) {
        if level == 1 {
            self.sort_key(text, Pad::None, out);
        }
    }
    fn safe_cut(&self, _text: &str) -> Option<usize> {
        None
    }
    fn units(&self, _text: &str) -> Option<Vec<crate::value::collation::Unit>> {
        None
    }
    fn sort_keys(&self, texts: &[&str], pad: Pad) -> Result<Vec<Vec<u8>>> {
        let mut all = Vec::with_capacity(texts.len());
        for chunk in texts.chunks(self.batch) {
            let mut payload = Vec::new();
            put_keys(&mut payload, pad, chunk);
            let got = self.link.plain(COLLATION_KEYS, payload)?;
            all.extend(counted(answer(&got, answered)?, chunk.len())?);
        }
        Ok(all)
    }
    fn batches(&self) -> bool {
        true
    }
}
