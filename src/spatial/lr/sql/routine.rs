//! A routine of §15 as SQL invokes it: its name, the types of its
//! parameters (SELF first of a method), its result, its null rule and
//! whether it reads the catalogs (`sql/catalog.rs`).

use super::args::{Args, P};
use crate::error::Result;
use crate::value::Value;

/// A routine of §15.
#[derive(Debug, Clone, Copy)]
pub struct Routine {
    /// Its name, as an identifier folds to upper case.
    pub name: &'static str,
    /// Its parameters, SELF first of a method.
    pub params: &'static [P],
    /// Its result.
    pub result: P,
    /// RETURNS NULL ON NULL INPUT, as it declares it.
    pub null_call: bool,
    /// Whether it reads the LRMs and linear elements of the catalogs.
    pub catalog: bool,
    /// What it does, of its arguments.
    pub run: fn(&Args<'_>) -> Result<Value>,
}

/// A routine RETURNS NULL ON NULL INPUT.
pub const fn null_call(name: &'static str, params: &'static [P], result: P, run: fn(&Args<'_>) -> Result<Value>) -> Routine {
    Routine { name, params, result, null_call: true, catalog: false, run }
}

/// A routine CALLED ON NULL INPUT.
pub const fn called(name: &'static str, params: &'static [P], result: P, run: fn(&Args<'_>) -> Result<Value>) -> Routine {
    Routine { name, params, result, null_call: false, catalog: false, run }
}

impl Routine {
    /// It, reading the catalogs.
    #[must_use]
    pub const fn reading(self) -> Routine {
        Routine { catalog: true, ..self }
    }

    /// Whether it takes arguments of the declared types `types`.
    pub fn takes(&self, types: &[crate::value::DataType]) -> bool {
        self.params.len() == types.len() && self.params.iter().zip(types).all(|(p, t)| p.takes(t))
    }

    /// Whether it may be invoked of the values `values`, the null value
    /// taken anywhere.
    pub fn holds(&self, values: &[Value]) -> bool {
        self.params.len() == values.len() && self.params.iter().zip(values).all(|(p, v)| v.is_null() || p.holds(v))
    }
}
