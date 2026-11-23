//! \[EXTENSION\] VECTOR's constructors and text forms (design document
//! §5.2): TO_VECTOR and its alias VECTOR, from a character string or
//! character large object (or a vector, converted), against an optional
//! dimension count, format and layout; FROM_VECTOR, a vector's text as a
//! character string, and FROM_VECTOR_CLOB, as a character large object.
//! The design document's `FROM_VECTOR(v RETURNING CLOB FORMAT SPARSE)`
//! is the binder's to turn into `FROM_VECTOR_CLOB(v, 'SPARSE')`.

use super::{Arg, CLOB_TYPE, TEXT_TYPE, integer_at, text_at, vector_at};
use crate::error::{Result, SqlState};
use crate::value::lob::Lob;
use crate::value::{DataType, MAX_CHARACTER_LENGTH, Value};
use crate::vector::kind::{conform, text_of};
use crate::vector::params::{FORMAT_PARAM, Shape, param_word, typed};
use crate::vector::{Element, Layout, data_type, mismatch, text};

/// A constructor or text form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Convert {
    /// TO_VECTOR(text [, dims [, format [, layout]]]).
    ToVector,
    /// VECTOR(text [, dims [, format [, layout]]]), TO_VECTOR's alias.
    Vector,
    /// FROM_VECTOR(v [, layout]), as a character string.
    FromVector,
    /// FROM_VECTOR_CLOB(v [, layout]), as a character large object.
    FromVectorClob,
}

impl Convert {
    /// Every constructor and text form.
    pub const ALL: [Convert; 4] = [Convert::ToVector, Convert::Vector, Convert::FromVector, Convert::FromVectorClob];

    /// Its name.
    pub fn name(self) -> &'static str {
        match self {
            Convert::ToVector => "TO_VECTOR",
            Convert::Vector => crate::vector::NAME,
            Convert::FromVector => "FROM_VECTOR",
            Convert::FromVectorClob => "FROM_VECTOR_CLOB",
        }
    }

    pub(super) fn arity(self) -> (usize, usize) {
        match self {
            Convert::ToVector | Convert::Vector => (1, 4),
            Convert::FromVector | Convert::FromVectorClob => (1, 2),
        }
    }

    pub(super) fn arg(self, at: usize) -> Option<Arg> {
        let making = matches!(self, Convert::ToVector | Convert::Vector);
        match (at, making) {
            (0, true) => Some(Arg::Source),
            (0, false) => Some(Arg::Vector),
            (1, true) => Some(Arg::Integer),
            (1, false) | (2 | 3, true) => Some(Arg::Text),
            _ => None,
        }
    }

    pub(super) fn output(self) -> DataType {
        match self {
            Convert::ToVector | Convert::Vector => data_type(),
            Convert::FromVector => TEXT_TYPE,
            Convert::FromVectorClob => CLOB_TYPE,
        }
    }

    pub(super) fn call(self, args: &[Value]) -> Result<Value> {
        match self {
            Convert::ToVector | Convert::Vector => to_vector(args),
            Convert::FromVector | Convert::FromVectorClob => {
                let v = vector_at(args, 0)?;
                let layout = match text_at(args, 1)? {
                    Some(name) => Layout::named(name).ok_or_else(|| mismatch(&format!("unknown vector layout {name}")))?,
                    None => v.layout(),
                };
                let written = text::write_as(v, layout);
                if matches!(self, Convert::FromVectorClob) {
                    return Ok(Value::Lob(Lob::of_text(&written)));
                }
                if written.chars().count() > usize::try_from(MAX_CHARACTER_LENGTH).unwrap_or(usize::MAX) {
                    let long = mismatch("the vector's text is longer than a character string; use FROM_VECTOR_CLOB");
                    return Err(long.with_state(SqlState::STRING_DATA_RIGHT_TRUNCATION));
                }
                Ok(Value::Char(written.into()))
            }
        }
    }
}

/// The shape TO_VECTOR's arguments after the first ask for.
fn asked(args: &[Value]) -> Result<Shape> {
    let dims = match integer_at(args, 1)? {
        None | Some(0) => None,
        Some(d) => Some(u32::try_from(d).map_err(|_| crate::vector::out_of_range(&format!("{d} is not a dimension count")))?),
    };
    let element = match text_at(args, 2)? {
        None => None,
        Some(name) => {
            let code = param_word(FORMAT_PARAM, name).ok_or_else(|| mismatch(&format!("unknown vector format {name}")))?;
            u8::try_from(code).ok().and_then(Element::of_code)
        }
    };
    let layout = match text_at(args, 3)? {
        None => None,
        Some(name) => Some(Layout::named(name).ok_or_else(|| mismatch(&format!("unknown vector layout {name}")))?),
    };
    Ok(Shape { dims, element, layout })
}

fn to_vector(args: &[Value]) -> Result<Value> {
    let shape = asked(args)?;
    let first = args.first().unwrap_or(&Value::Null);
    if let Some(written) = text_of(first)? {
        return text::vector(written.trim_end_matches(' '), shape)?.value();
    }
    let t = typed(shape).map_err(|e| mismatch(&e.msg))?;
    conform(first.clone(), &t)
}
