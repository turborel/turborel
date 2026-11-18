//! What a binding notes of what it references, where its `Context` says:
//! the columns of named tables and views [ISO/IEC 9075:1992 §21.3.12,
//! §21.3.18], those of each select list item, the subject routines it
//! invokes [ISO/IEC 9075-2:2023 §4.35.2] and the registered XML Schemas it
//! indicates [ISO/IEC 9075-14:2006 draft §4.2.6], and the sequence
//! generators it draws from [§11.74 SR3], each for a definition to keep.

use std::cell::{Ref, RefCell, RefMut};

use crate::catalog::{Indicated, QualifiedName};

/// The columns a query references, each as its table or view and column
/// name, in the order referenced, once or more.
pub type Uses = RefCell<Vec<(QualifiedName, String)>>;

/// Which of the columns a statement references `Uses` notes, if not each
/// one: those the Definition Schema's base table of usage of what holds
/// the statement has a row of, by its Description 1. Each notes the
/// columns an `<insert column list>` names and the object columns of an
/// UPDATE's `<set clause>`s.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Counting {
    /// [ISO/IEC 9075-11:2006 draft §6.35 Description 1] An SQL routine's:
    /// those in the search condition of a searched UPDATE or DELETE, and in
    /// an `<update source>`.
    Routine,
    /// [ISO/IEC 9075-11:2006 draft §6.54 Description 1] A trigger's: those
    /// and those in any `<search condition>` of its definition.
    Trigger,
}

/// The columns each item of a select list references, as `Uses` notes
/// them, its subqueries' too.
pub type Selected = RefCell<Vec<Vec<(QualifiedName, String)>>>;

/// The specific names of the SQL-invoked routines a statement invokes; the
/// registered XML Schemas its `<XML valid according to what>`s and types
/// of XMLSCHEMA indicate [ISO/IEC 9075-14:2006 draft §4.2.6]; and the
/// registered XQuery modules its queries import (RULES.md D106), which a
/// definition binds as it is created, as it does its subject routines.
#[derive(Debug, Default)]
pub struct Invoked {
    /// The routines' specific names, in the order invoked, once or more.
    routines: RefCell<Vec<QualifiedName>>,
    /// The registered XML Schemas' names, once or more.
    xml_schemas: RefCell<Vec<QualifiedName>>,
    /// \[EXTENSION\] RULES.md D106 The registered XQuery modules' names
    /// its queries import, once or more.
    xquery_modules: RefCell<Vec<QualifiedName>>,
    /// Whether an XQuery expression was typed statically as
    /// XQUERY.STATIC_TYPING asked [XQuery §5.2.3].
    typed: std::cell::Cell<bool>,
}

impl Invoked {
    /// Of the routines `routines` noted already, and no registered XML
    /// Schema.
    #[must_use]
    pub fn new(routines: Vec<QualifiedName>) -> Invoked {
        Invoked {
            routines: RefCell::new(routines),
            xml_schemas: RefCell::default(),
            xquery_modules: RefCell::default(),
            typed: std::cell::Cell::new(false),
        }
    }

    /// The routines noted.
    pub fn borrow(&self) -> Ref<'_, Vec<QualifiedName>> {
        self.routines.borrow()
    }

    /// The routines noted, to note more.
    pub fn borrow_mut(&self) -> RefMut<'_, Vec<QualifiedName>> {
        self.routines.borrow_mut()
    }

    /// The routines noted, taken.
    pub fn take(&self) -> Vec<QualifiedName> {
        self.routines.take()
    }

    /// The routines noted.
    #[must_use]
    pub fn into_inner(self) -> Vec<QualifiedName> {
        self.routines.into_inner()
    }

    /// Notes the registered XML Schema `name` indicated.
    pub fn indicated(&self, name: &QualifiedName) {
        self.xml_schemas.borrow_mut().push(name.clone());
    }

    /// \[EXTENSION\] Notes the registered XQuery module `name` imported
    /// (RULES.md D106).
    pub fn imported(&self, name: &QualifiedName) {
        self.xquery_modules.borrow_mut().push(name.clone());
    }

    /// The registered XML Schemas and XQuery modules noted, taken.
    pub fn take_indicated(&self) -> Indicated {
        Indicated { xml_schemas: self.xml_schemas.take(), xquery_modules: self.xquery_modules.take() }
    }

    /// Notes that an XQuery expression was typed statically as
    /// XQUERY.STATIC_TYPING asked [XQuery §5.2.3].
    pub fn typed(&self) {
        self.typed.set(true);
    }

    /// Whether one was, taken.
    pub fn take_typed(&self) -> bool {
        self.typed.take()
    }
}

/// [ISO/IEC 9075-2:2023 §6.14, §11.74 SR3] The names of the sequence
/// generators a statement's NEXT VALUE FOR draws from, once or more.
pub type DrawnFrom = RefCell<Vec<QualifiedName>>;

impl<'a> super::Binder<'a> {
    /// Where it notes a column it references now: its `Uses`, unless its
    /// `Counting` does not count a column where it binds now.
    pub(super) fn noted_uses(&self) -> Option<&'a Uses> {
        self.uses.filter(|_| self.counting.is_none() || self.counted.get())
    }

    /// What `bind` gives, bound where its `Counting` counts a column.
    pub(super) fn counted<T>(&self, bind: impl FnOnce() -> T) -> T {
        let before = self.counted.replace(true);
        let bound = bind();
        self.counted.set(before);
        bound
    }

    /// Notes the column `column` of the table or view `table` an `<insert
    /// column list>` or an UPDATE's `<set clause>` names, which each
    /// `Counting` counts [ISO/IEC 9075-11:2006 draft §6.35 Description 1c,
    /// 1d, §6.54 Description 1d, 1e].
    pub(super) fn note_named(&self, table: &QualifiedName, column: &str) {
        if let Some(uses) = self.uses {
            uses.borrow_mut().push((table.clone(), column.to_owned()));
        }
    }
}
