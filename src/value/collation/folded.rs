//! \[EXTENSION\] The collations of one character at a time: `UCS_BASIC` and
//! `SQL_TEXT` by Unicode scalar value [ISO/IEC 9075-2:2023 §4.3.6], and
//! `CASE_INSENSITIVE` by each character's simple case folding.

use std::sync::Arc;

use super::pad::padded;
use super::table::Table;
use super::{CASE_INSENSITIVE, Collation, Pad};

/// The version of what scalar value order rests on: nothing but itself.
const SCALAR_VERSION: &str = "1";

/// A collation by Unicode scalar value, of its name.
pub(super) struct Scalar(pub(super) &'static str);

impl Collation for Scalar {
    fn name(&self) -> &str {
        self.0
    }

    fn version(&self) -> &str {
        SCALAR_VERSION
    }

    fn sort_key(&self, text: &str, pad: Pad, out: &mut Vec<u8>) {
        match pad {
            Pad::Space => padded(text.chars(), out),
            Pad::None => out.extend_from_slice(text.as_bytes()),
        }
    }

    fn fold(&self, c: char) -> Option<char> {
        Some(c)
    }

    fn weighs(&self, _: char) -> bool {
        true
    }
}

/// `CASE_INSENSITIVE`, whose folds are `Table`'s, of Unicode 13.0 for good
/// and not the toolchain's, so a new compiler cannot reorder an index.
pub(super) struct Folded(pub(super) Arc<Table>);

impl Collation for Folded {
    fn name(&self) -> &str {
        CASE_INSENSITIVE
    }

    fn version(&self) -> &str {
        super::table::VERSION
    }

    fn sort_key(&self, text: &str, pad: Pad, out: &mut Vec<u8>) {
        let folded = text.chars().map(|c| self.0.fold(c));
        match pad {
            Pad::Space => padded(folded, out),
            Pad::None => {
                let mut buffer = [0; 4];
                folded.for_each(|c| out.extend_from_slice(c.encode_utf8(&mut buffer).as_bytes()));
            }
        }
    }

    fn fold(&self, c: char) -> Option<char> {
        Some(self.0.fold(c))
    }

    fn weighs(&self, _: char) -> bool {
        true
    }
}
