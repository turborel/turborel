//! The constructors of attributes, text, comments and processing
//! instructions evaluated [XQuery §3.7.2, §3.7.3.2, §3.7.3.4 to
//! §3.7.3.6]: each node made stands alone, the root of a tree of its own.

use super::super::super::error::{ErrorCode, error};
use super::super::super::plan::{NameOp, Op, PiOp};
use super::super::{Evaluator, Focus, Frame, Seq};
use super::{id_value, joined, names};
use crate::error::Result;
use crate::xml::{Builder, Specified, Version, XMLNS_NAMESPACE, is_ncname, is_space, is_xml_id};

/// [XQuery §3.7.3.2] The name of an attribute that is no attribute.
const XMLNS: &str = "xmlns";
/// [XQuery §3.7.3.5] What a processing instruction's target is not, in any
/// case.
const XML: &str = "xml";
/// [XQuery §3.7.3.5] What a processing instruction's content holds not.
const PI_END: &str = "?>";
/// [XQuery §3.7.3.6] What a comment holds not, nor ends with half of.
const TWO_HYPHENS: &str = "--";

impl Evaluator<'_> {
    /// A computed attribute constructor [XQuery §3.7.3.2]: its name not
    /// `xmlns` nor in its namespace \[err:XQDY0044\], its value its content's
    /// strings a space apart, of `xml:id` collapsed.
    pub(in super::super) fn computed_attribute(
        &self,
        name: &NameOp,
        content: Option<&Op>,
        frame: &mut Frame,
        focus: &Focus,
    ) -> Result<Seq> {
        let mut name = match name {
            NameOp::Fixed(name) => name.clone(),
            NameOp::Computed(op, names) => {
                let mut computed = names::computed(&self.eval(op, frame, focus)?, names)?;
                if computed.prefix.is_none() {
                    // [XQuery §3.7.3.2] an unprefixed attribute name is in no
                    // namespace.
                    computed.uri = None;
                }
                computed
            }
        };
        if name.uri.as_deref() == Some(XMLNS_NAMESPACE) || (name.uri.is_none() && &*name.local == XMLNS) {
            return Err(error(ErrorCode::XQDY0044, "a computed attribute is named xmlns, or in its namespace [XQuery §3.7.3.2]"));
        }
        let value = match content {
            Some(op) => joined(&self.eval(op, frame, focus)?)?,
            None => String::new(),
        };
        if name.uri.is_none() {
            name.prefix = None;
        }
        let value = id_value(&name, value);
        let is_id = is_xml_id(&name);
        let mut builder = Builder::parentless();
        builder.attribute(&Specified { name, value, is_id, is_idrefs: false })?;
        self.built(builder)
    }

    /// `text` [XQuery §3.7.3.4]: no node of no value, else a text node of
    /// its strings a space apart.
    pub(in super::super) fn text(&self, content: &Op, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let value = self.eval(content, frame, focus)?;
        if value.is_empty() {
            return Ok(Vec::new());
        }
        let mut builder = Builder::parentless();
        builder.text(&joined(&value)?)?;
        self.built(builder)
    }

    /// A computed comment constructor [XQuery §3.7.3.6]: its content's
    /// strings a space apart, holding no two hyphens together nor ending
    /// with one \[err:XQDY0072\].
    pub(in super::super) fn comment(&self, content: &Op, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let value = joined(&self.eval(content, frame, focus)?)?;
        if value.contains(TWO_HYPHENS) || value.ends_with('-') {
            return Err(error(ErrorCode::XQDY0072, "a computed comment holds two hyphens together or ends with one [XQuery §3.7.3.6]"));
        }
        self.direct_comment(&value)
    }

    /// A comment of `text` [XQuery §3.7.2].
    pub(in super::super) fn direct_comment(&self, text: &str) -> Result<Seq> {
        let mut builder = Builder::parentless();
        builder.comment(text)?;
        self.built(builder)
    }

    /// A processing instruction constructor [XQuery §3.7.2, §3.7.3.5]: a
    /// computed target a string or untyped value that is an NCName
    /// \[err:XQDY0041\], not `xml` in any case \[err:XQDY0064\]; a computed
    /// content's strings a space apart, holding no "?>" \[err:XQDY0026\],
    /// its leading white space removed.
    pub(in super::super) fn pi(&self, pi: &PiOp, frame: &mut Frame, focus: &Focus) -> Result<Seq> {
        let target = match &pi.target {
            Ok(target) => target.to_string(),
            Err(op) => {
                let value = self.eval(op, frame, focus)?;
                let target = joined(&value)?;
                let atomic = matches!(value.as_slice(), [_]);
                if !atomic || !is_ncname(Version::V11, target.trim_matches(is_space)) {
                    let why = format!("'{target}' is no NCName a processing instruction is named [XQuery §3.7.3.5]");
                    return Err(error(ErrorCode::XQDY0041, &why));
                }
                target.trim_matches(is_space).to_owned()
            }
        };
        if target.eq_ignore_ascii_case(XML) {
            return Err(error(ErrorCode::XQDY0064, "a processing instruction is named XML, in some case [XQuery §3.7.3.5]"));
        }
        let content = match &pi.content {
            Ok(content) => content.to_string(),
            Err(op) => {
                let value = match op {
                    Some(op) => joined(&self.eval(op, frame, focus)?)?,
                    None => String::new(),
                };
                if value.contains(PI_END) {
                    return Err(error(ErrorCode::XQDY0026, "a processing instruction's content holds '?>' [XQuery §3.7.3.5]"));
                }
                value.trim_start_matches(is_space).to_owned()
            }
        };
        let mut builder = Builder::parentless();
        builder.processing_instruction(&target, &content)?;
        self.built(builder)
    }
}
