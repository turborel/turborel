//! \[EXTENSION\] Kind 37, an optimizer rule (RULES.md D275 to D277): a
//! rewrite of the plan form `turborel::plugin::plan` says, each pass the
//! planner makes of a statement one call.
//!
//! A rule is given the whole plan, each node with its operator, its
//! columns and their types, and the planner's estimates of its rows and
//! width, a scan's with its table's statistics. It answers the plan
//! rewritten, or none for no change. What the form keeps (a node or an
//! expression the form does not say) the rule may move, as a whole, to
//! another place where its columns are, but not open or copy. The
//! planner checks the plan it is given back, and a plan not valid skips
//! the rule for the statement: the rule's own contract is that the plan
//! it gives returns the same rows.

use turborel::plugin::bridge::rule::put_rewritten;
use turborel::plugin::plan::{Form, decode};

use super::dispatch::{read, written};
use super::registry::{Plugin, find};
use super::serve::Cx;
use super::{Answer, PluginError};

/// Kind 37, an optimizer rule.
pub trait Rule {
    /// `form` rewritten, or none if the rule leaves it.
    ///
    /// # Errors
    ///
    /// The rule's: the statement is planned as if the rule were not there.
    fn rewrite(&mut self, cx: &mut Cx<'_>, form: &Form) -> Answer<Option<Form>>;
}

/// The answer of a rule's REWRITE.
///
/// # Errors
///
/// The rule's, or SQLSTATE 38000 of an operation unknown.
pub(super) fn rewrite(plugin: &mut Plugin, cx: &mut Cx<'_>, (op, object): (u8, &str), r: &mut turborel::bytes::Reader<'_>) -> Answer<Vec<u8>> {
    if op != turborel::plugin::payload::RULE_REWRITE {
        return Err(PluginError::external(&format!("a rule has no operation {op}")));
    }
    let form = read(r, decode)?;
    let rewritten = find(&mut plugin.rules, object)?.rewrite(cx, &form)?;
    written(|o| put_rewritten(o, rewritten.as_ref()))
}
