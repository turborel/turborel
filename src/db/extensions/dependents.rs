//! \[EXTENSION\] What keeps an extension from being dropped (RULES.md
//! D228): what the database stores by its objects. A table of its storage
//! engine, an index of its index method, a column of its type or by its
//! collation, a collation made from its collation, and an index naming
//! its ranking or quantizer each keep it, with RESTRICT and with CASCADE
//! alike: their rows and entries are the program's, or read by its
//! objects, so none is dropped but by a statement naming it.

use super::super::State;
use super::refused;
use crate::catalog::{ColumnDesc, IndexDesc};
use crate::error::Result;
use crate::plugin::{Declared, Installed, ObjectKind};
use crate::value::{DataType, Value};

/// Whether `t` is, or holds, the type `name`.
fn of_type(t: &DataType, name: &str) -> bool {
    if let DataType::Extension(e) = t {
        return e.kind.name() == name;
    }
    if let DataType::Array(a) = t {
        return of_type(&a.element, name);
    }
    if let DataType::Multiset(m) = t {
        return of_type(&m.element, name);
    }
    if let DataType::Row(r) = t {
        return r.fields.iter().any(|f| of_type(&f.data_type, name));
    }
    false
}

/// Whether column `c` depends on `d`.
fn column_needs(c: &ColumnDesc, d: &Declared, collations: &[String]) -> bool {
    match d.kind {
        ObjectKind::Type => of_type(&c.data_type, &d.name),
        ObjectKind::Collation => c.collation.as_ref().is_some_and(|n| collations.contains(&n.ident)),
        ObjectKind::Scalar
        | ObjectKind::Aggregate
        | ObjectKind::Table
        | ObjectKind::Procedure
        | ObjectKind::Authenticator
        | ObjectKind::Embedding
        | ObjectKind::Metric
        | ObjectKind::Partition
        | ObjectKind::Target
        | ObjectKind::Ranking
        | ObjectKind::Quantizer
        | ObjectKind::SystemTable
        | ObjectKind::XQuery
        | ObjectKind::Engine
        | ObjectKind::IndexMethod
        | ObjectKind::Credentials
        | ObjectKind::Diagnostics
        | ObjectKind::Rule => false,
    }
}

/// Whether index `ix` depends on `d`.
fn index_needs(ix: &IndexDesc, d: &Declared) -> bool {
    let named = |v: &Value| if let Value::Char(s) = v { s.trim().eq_ignore_ascii_case(&d.name) } else { false };
    match d.kind {
        ObjectKind::IndexMethod => ix.method == d.name,
        ObjectKind::Ranking | ObjectKind::Quantizer => ix.options.iter().any(|(_, v)| named(v)),
        ObjectKind::Scalar
        | ObjectKind::Aggregate
        | ObjectKind::Table
        | ObjectKind::Procedure
        | ObjectKind::Authenticator
        | ObjectKind::Embedding
        | ObjectKind::Metric
        | ObjectKind::Partition
        | ObjectKind::Target
        | ObjectKind::Collation
        | ObjectKind::SystemTable
        | ObjectKind::XQuery
        | ObjectKind::Type
        | ObjectKind::Engine
        | ObjectKind::Credentials
        | ObjectKind::Diagnostics
        | ObjectKind::Rule => false,
    }
}

impl State {
    /// That nothing the database stores depends on an object of
    /// `installed`.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 naming the first that does.
    pub(super) fn unneeded(&self, installed: &Installed) -> Result<()> {
        let name = &installed.manifest.name;
        let first = |what: String| Err(refused(format!("{what}, so extension {name} is not dropped: drop it first")));
        for (d, _) in &installed.objects {
            let mut collations = vec![d.name.clone()];
            collations.extend(self.catalog.collations().into_iter().filter(|c| c.base == d.name).map(|c| c.name.ident.clone()));
            if let (ObjectKind::Collation, Some(made)) = (d.kind, collations.get(1)) {
                return first(format!("collation {made} is made from its collation {}", d.name));
            }
            for t in self.catalog.tables() {
                if d.kind == ObjectKind::Engine && t.engine == d.name {
                    return first(format!("table {} is kept by its storage engine {}", t.name, d.name));
                }
                if let Some(c) = t.columns.iter().find(|c| column_needs(c, d, &collations)) {
                    return first(format!("column {}.{} needs its {} {}", t.name, c.name, d.kind.word(), d.name));
                }
                if let Some(ix) = self.catalog.indexes(&t.name).into_iter().find(|ix| index_needs(ix, d)) {
                    return first(format!("index {} needs its {} {}", ix.name, d.kind.word(), d.name));
                }
            }
        }
        Ok(())
    }
}
