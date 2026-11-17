//! The kinds of type beyond the standard registered with a database
//! (RULES.md D53).

use std::collections::BTreeMap;
use std::sync::Arc;

use super::SqlType;
use crate::error::{Code, Result, SqlError};

/// The kinds of type beyond the standard registered with a database
/// (RULES.md D53), by name: cheap to clone, as each snapshot of the
/// definitions, each transaction's undo and each spill holds them.
#[derive(Debug, Clone, Default)]
pub struct TypeRegistry {
    /// The kinds, by name; none, reserving nothing, of a registry of no
    /// kind, as each transaction's and list's begins.
    kinds: Option<Arc<BTreeMap<String, Kind>>>,
}

/// A kind of type beyond the standard, by a thin reference, so a
/// `DataType` of it stays small: the hot paths copy and compare types.
pub type Kind = &'static &'static dyn SqlType;

impl TypeRegistry {
    /// The kind named `name`, if one is registered.
    pub fn get(&self, name: &str) -> Option<&'static dyn SqlType> {
        self.kinds.as_ref()?.get(name).map(|k| **k)
    }

    /// The kind named `name`, if one is registered, as a type of it holds
    /// it.
    pub fn kind(&self, name: &str) -> Option<Kind> {
        self.kinds.as_ref()?.get(name).copied()
    }

    /// Each kind registered that declares settings (RULES.md D36).
    pub(crate) fn configurables(&self) -> impl Iterator<Item = &dyn crate::settings::Configurable> {
        self.kinds.iter().flat_map(|kinds| kinds.values()).filter_map(|k| k.configurable())
    }

    /// The names registered, in order.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.kinds.iter().flat_map(|kinds| kinds.keys()).map(String::as_str)
    }

    /// The kinds built in, SQL/MM Full-Text's FULLTEXT and FT_PATTERN
    /// (RULES.md D87), which a registration of a type of their names is
    /// refused beside.
    pub(crate) fn builtin() -> TypeRegistry {
        let kinds = crate::fulltext::types::types().map(|kind: Kind| (kind.name().to_owned(), kind));
        TypeRegistry { kinds: Some(Arc::new(kinds.into_iter().collect())) }
    }

    /// Registers `kind`.
    ///
    /// # Errors
    ///
    /// SQLCODE -910 if a kind of its name is registered already.
    pub(crate) fn add(&mut self, kind: &'static dyn SqlType) -> Result<()> {
        if self.kind(kind.name()).is_some() {
            return Err(SqlError::new(Code::Api, format!("[DESIGN] a type {} is registered already", kind.name())));
        }
        let held: Kind = Box::leak(Box::new(kind));
        Arc::make_mut(self.kinds.get_or_insert_with(Arc::default)).insert(kind.name().to_owned(), held);
        Ok(())
    }
}
