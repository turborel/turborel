//! XQuery's computed text, comment and processing instruction
//! constructors [XQuery §3.7.3.4 to §3.7.3.6], as XMLTEXT, XMLCOMMENT and
//! XMLPI evaluate them [ISO/IEC 9075-14:2006 draft §6.19, §6.11, §6.17],
//! and the concatenation of two XML values [§10.14], as XMLCONCAT and
//! XMLAGG take it.
//!
//! Each node made stands alone, the root of a tree of its own with no
//! parent [XQueryDM §6.6.1, §6.5.1, §6.7.1].

use super::chars::{Version, is_char, is_space};
use super::sequence::{Item, Node, Sequence};
use super::tree::Builder;
use crate::error::{Code, Result, SqlError, SqlState};

/// The node a `build` adds to a tree of no node yet, as a sequence of it.
fn alone(build: impl FnOnce(&mut Builder) -> Result<()>) -> Result<Sequence> {
    let mut builder = Builder::parentless();
    build(&mut builder)?;
    Ok(Sequence::new(vec![Item::Node(Node::root(builder.finish()?))]))
}

/// [§6.19 GR6, GR7] The text node of `content`, alone, which a
/// zero-length string makes too [XQuery §3.7.3.4].
///
/// # Errors
///
/// SQLCODE -900 if `content` is longer than a tree holds.
pub fn text(content: &str) -> Result<Sequence> {
    alone(|b| b.text(content))
}

/// [§6.11 GR3 to GR5] The comment node of `content`, alone: if
/// `'<!--' || content || '-->'` is a Comment [XML 1.1 §2.5 \[15\]], of
/// characters [XML 1.1 §2.2 \[2\]] with no two hyphens together and no
/// hyphen last.
///
/// # Errors
///
/// SQLSTATE 2200S, invalid comment, if it is not; SQLCODE -900 if it is
/// longer than a tree holds.
pub fn comment(content: &str) -> Result<Sequence> {
    if content.contains("--") || content.ends_with('-') || !content.chars().all(|c| is_char(Version::V11, c)) {
        return Err(SqlError::new(
            Code::Data,
            "[ISO/IEC 9075-14:2006 draft §6.11 GR3] the comment holds two hyphens together, ends with one, or holds a character XML does not [XML 1.1 §2.5 [15]]",
        )
        .with_state(SqlState::INVALID_COMMENT));
    }
    alone(|b| b.comment(content))
}

/// [§6.17 GR4, GR9] The processing instruction of `target`, an NCName
/// other than `xml` in any case [SR2, SR3], and `content`, alone: if
/// `'<?' || target || ' ' || content || '?>'` is a PI [XML 1.1 §2.6
/// \[16\]], `content` of characters [XML 1.1 §2.2 \[2\]] and no `?>`;
/// its leading white space removed [XQuery §3.7.3.5].
///
/// # Errors
///
/// SQLSTATE 2200T, invalid processing instruction, if it is not; SQLCODE
/// -900 if it is longer than a tree holds.
pub fn processing_instruction(target: &str, content: &str) -> Result<Sequence> {
    if content.contains("?>") || !content.chars().all(|c| is_char(Version::V11, c)) {
        return Err(SqlError::new(
            Code::Data,
            format!(
                "[ISO/IEC 9075-14:2006 draft §6.17 GR4] the processing instruction {target} holds '?>' or a character XML does not [XML 1.1 §2.6 [16]]"
            ),
        )
        .with_state(SqlState::INVALID_PROCESSING_INSTRUCTION));
    }
    alone(|b| b.processing_instruction(target, content.trim_start_matches(is_space)))
}

/// [§10.14] The concatenation of XML values `values`, in order, each
/// `None` if null: every item of each value but the null value, in order;
/// `None`, the null value, if every value is.
#[must_use]
pub fn concatenate<'a>(values: impl IntoIterator<Item = Option<&'a Sequence>>) -> Option<Sequence> {
    let mut items: Option<Vec<Item>> = None;
    for value in values.into_iter().flatten() {
        items.get_or_insert_with(Vec::new).extend(value.items().iter().cloned());
    }
    items.map(Sequence::new)
}

#[cfg(test)]
mod tests;
