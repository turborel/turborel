//! \[EXTENSION\] A plug-in's optimizer rule (RULES.md D275, D276): a
//! rewrite of the plan form (`plan.rs`), one REWRITE a pass, answered by
//! the form rewritten or no change. Its PHASE says when it runs, on the
//! plan as bound (BOUND) or after the built-in rules, before joins are
//! ordered (REWRITTEN, the default), and its PASSES how many REWRITEs a
//! statement makes of it at most (1 unless written).

use crate::bytes::Put;
use crate::error::Result;
use crate::plugin::manifest::{BOUND, Declared};
use crate::plugin::payload::RULE_REWRITE;
use crate::plugin::plan::{Form, decode, encode};

use super::{Link, answer, leaked};

/// When a rule runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// On the plan as bound, before the built-in rules.
    Bound,
    /// After the built-in rules, before joins are ordered.
    Rewritten,
}

/// A plug-in's optimizer rule.
#[derive(Debug)]
pub struct PluginRule {
    link: Link,
    name: &'static str,
    phase: Phase,
    passes: u32,
}

impl PluginRule {
    /// The rule `d` declares, called by `link`.
    pub fn new(d: &Declared, link: Link) -> PluginRule {
        let phase = if d.phase.as_deref() == Some(BOUND) { Phase::Bound } else { Phase::Rewritten };
        let name = leaked(&link.named());
        PluginRule { link, name, phase, passes: d.passes.unwrap_or(1).max(1) }
    }

    /// Its name as EXPLAIN shows it: its extension's dotted with its own.
    pub fn name(&self) -> &'static str {
        self.name
    }

    /// When it runs.
    pub fn phase(&self) -> Phase {
        self.phase
    }

    /// The passes it makes at most.
    pub fn passes(&self) -> u32 {
        self.passes
    }

    /// `form` rewritten, or none if the rule leaves it.
    ///
    /// # Errors
    ///
    /// The program's, the pool's, or SQLSTATE 38T03 of an answer that is
    /// no plan form.
    pub fn rewrite(&self, form: &Form) -> Result<Option<Form>> {
        let got = self.link.plain(RULE_REWRITE, encode(form)?)?;
        answer(&got, |r| if r.u8()? == 0 { Ok(None) } else { decode(r).map(Some) })
    }
}

/// The answer of a rule that rewrote `form`, as a program writes it.
///
/// # Errors
///
/// As `encode`.
pub fn put_rewritten(out: &mut Vec<u8>, form: Option<&Form>) -> Result<()> {
    out.put_u8(u8::from(form.is_some()));
    if let Some(f) = form {
        out.extend_from_slice(&encode(f)?);
    }
    Ok(())
}
