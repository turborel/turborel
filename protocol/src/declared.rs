//! \[DESIGN\] Declared types (RULES.md D396): with `declared-types`, a
//! Description's and a Prepared's columns carry the type each was declared
//! with, in their tail, as a client cannot tell it from the type's tag.
//! One tag holds several declared types. SMALLINT, INTEGER, BIGINT,
//! DECIMAL and NUMERIC are all exact numeric (tag 2), REAL, DOUBLE
//! PRECISION and FLOAT all approximate numeric (tag 3), and a distinct
//! type goes as its source type. The values sent are as ever.
//!
//! ```text
//! 19 declared   count u32, then of each column in order its type's name,
//!               as the Information Schema's DATA_TYPE gives it, and its
//!               type as SQL writes it, strings
//! ```

use turborel::bytes::{Put, Reader};
use turborel::error::Result;
use turborel::value::DataType;

use crate::Column;
use crate::fields::{counted, put_count};
use crate::tail::{DECLARED, Tail};

/// \[DESIGN\] RULES.md D396: the type a column was declared with, beside
/// the tag its values are written by.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Declared {
    /// Its name, as the Information Schema's DATA_TYPE gives it [ISO/IEC
    /// 9075:1992 §6.1, §21.3.5]: INTEGER, SMALLINT, BIGINT, DECIMAL,
    /// NUMERIC, REAL, DOUBLE PRECISION, FLOAT, USER-DEFINED of a distinct
    /// type, or the name of any other type.
    pub name: String,
    /// The whole type as SQL writes it: INTEGER, DECIMAL(9,2), a
    /// distinct type's schema and name, or INTEGER ARRAY\[3\].
    pub written: String,
}

impl Declared {
    /// The declared type of `t`, as its descriptor keeps it.
    pub fn of(t: &DataType) -> Declared {
        Declared { name: t.kind().descriptor(t).name, written: t.to_string() }
    }
}

/// Writes the declared type of each of `columns`, if any column has one;
/// one without is written as its data type says.
pub(crate) fn put_declared(b: &mut Vec<u8>, columns: &[Column]) {
    if columns.iter().all(|c| c.declared.is_none()) {
        return;
    }
    b.put_u8(DECLARED);
    put_count(b, columns.len());
    for c in columns {
        let declared = c.declared.clone().unwrap_or_else(|| Declared::of(&c.data_type));
        b.put_str(&declared.name);
        b.put_str(&declared.written);
    }
}

/// The tail of a Description or a Prepared, its declared types given to
/// `columns`: one for each, or none.
pub(crate) fn declared(r: &mut Reader<'_>, columns: &mut [Column]) -> Result<()> {
    let mut tail = Tail::new();
    while tail.next(r, &[DECLARED])?.is_some() {
        let all = counted(r, |r| Ok(Declared { name: r.str()?, written: r.str()? }))?;
        if all.len() != columns.len() {
            return Err(r.corrupt("its declared types are not one for each column"));
        }
        for (c, d) in columns.iter_mut().zip(all) {
            c.declared = Some(d);
        }
    }
    Ok(())
}
