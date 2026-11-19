//! \[DESIGN\] Structured failures and notices (RULES.md D186, D187): what a
//! failure or a notice says beyond its SQLSTATE and message, its severity,
//! detail, hint and context, and the names of the schema, table, column,
//! data type and constraint it is about, as the server's `SqlError` names
//! them (`error::Subject`). An Error carries them in its tail where the
//! capability `diagnostics` was granted; a Notice always does.
//!
//! ```text
//! notice:     sqlstate, a string of 5; message; tail (severity, detail,
//!             hint, context, schema, table, column, data type, constraint)
//! ```

use turborel::bytes::{Put, Reader};
use turborel::error::{Result, SqlError, Subject};

use crate::tail::{SEVERITY, Tail, put_text};

/// \[DESIGN\] The severity of a statement's failure: the session goes on.
pub const ERROR: &str = "ERROR";
/// \[DESIGN\] The severity of a failure that ends the connection.
pub const FATAL: &str = "FATAL";
/// \[DESIGN\] The severity of a completion condition of class 01.
pub const WARNING: &str = "WARNING";
/// \[DESIGN\] The severity of a notice of the server's own.
pub const NOTICE: &str = "NOTICE";

/// What a failure or a notice says beyond its SQLSTATE and message: each
/// none if it says nothing of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Diagnostics {
    /// `ERROR`, `FATAL`, `WARNING` or `NOTICE`.
    pub severity: Option<String>,
    /// More of what happened than the message says, as the values of the
    /// key a constraint refused.
    pub detail: Option<String>,
    /// What may be done about it.
    pub hint: Option<String>,
    /// Where it was raised: the trigger or routine running.
    pub context: Option<String>,
    /// The schema of what it is about.
    pub schema: Option<String>,
    /// The table or view it is about.
    pub table: Option<String>,
    /// The column it is about.
    pub column: Option<String>,
    /// The data type of the value it is about.
    pub data_type: Option<String>,
    /// The constraint broken.
    pub constraint: Option<String>,
}

impl Diagnostics {
    /// Its fields, in the order of their tags from `SEVERITY`.
    fn strings(&self) -> [Option<&str>; 9] {
        [
            self.severity.as_deref(),
            self.detail.as_deref(),
            self.hint.as_deref(),
            self.context.as_deref(),
            self.schema.as_deref(),
            self.table.as_deref(),
            self.column.as_deref(),
            self.data_type.as_deref(),
            self.constraint.as_deref(),
        ]
    }

    /// Its field of tag `tag`, to be set.
    fn slot(&mut self, tag: u8) -> Option<&mut Option<String>> {
        let slots = [
            &mut self.severity,
            &mut self.detail,
            &mut self.hint,
            &mut self.context,
            &mut self.schema,
            &mut self.table,
            &mut self.column,
            &mut self.data_type,
            &mut self.constraint,
        ];
        let at = usize::from(tag.checked_sub(SEVERITY)?);
        slots.into_iter().nth(at)
    }

    /// The tags of its fields.
    pub(crate) fn tags() -> [u8; 9] {
        std::array::from_fn(|at| SEVERITY.saturating_add(u8::try_from(at).unwrap_or(u8::MAX)))
    }

    /// Writes its fields, each set one with its tag.
    pub(crate) fn put(&self, b: &mut Vec<u8>) {
        for (tag, field) in Diagnostics::tags().into_iter().zip(self.strings()) {
            put_text(b, tag, field);
        }
    }

    /// Reads the field of `tag`, one of `tags`.
    pub(crate) fn read(&mut self, tag: u8, r: &mut Reader<'_>) -> Result<()> {
        let value = r.str()?;
        if let Some(slot) = self.slot(tag) {
            *slot = Some(value);
        }
        Ok(())
    }

    /// What `e` says beyond its SQLSTATE and message, of severity
    /// `severity`: what its subject names.
    pub fn of(e: &SqlError, severity: &str) -> Diagnostics {
        let mut d = Diagnostics { severity: Some(severity.to_owned()), ..Diagnostics::default() };
        let Some(subject) = e.subject.as_deref() else { return d };
        let Subject { constraint, table, column, routine, trigger, .. } = subject;
        if let Some((schema, name)) = table {
            (d.schema, d.table) = (Some(schema.clone()), Some(name.clone()));
        }
        if let Some((schema, name)) = constraint {
            d.schema.get_or_insert_with(|| schema.clone());
            d.constraint = Some(name.clone());
        }
        d.column.clone_from(column);
        d.detail.clone_from(&subject.detail);
        d.hint.clone_from(&subject.hint);
        d.data_type.clone_from(&subject.data_type);
        d.context = match (trigger, routine) {
            (Some((schema, name)), _) => Some(format!("trigger {schema}.{name}")),
            (None, Some(r)) => Some(format!("routine {}.{}", r.name.0, r.name.1)),
            (None, None) => None,
        };
        d
    }
}

/// \[DESIGN\] A notice (RULES.md D187): a completion condition of class 01
/// a statement raised, or one of the server's own, sent before the
/// statement's completion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    /// Its SQLSTATE: five characters.
    pub sqlstate: String,
    /// What it says, in plain words.
    pub message: String,
    /// Its severity, and what it is about.
    pub diagnostics: Diagnostics,
}

impl Notice {
    /// Writes its body.
    pub(crate) fn put(&self, b: &mut Vec<u8>) {
        b.put_str(&self.sqlstate);
        b.put_str(&self.message);
        self.diagnostics.put(b);
    }

    /// Reads its body.
    pub(crate) fn read(r: &mut Reader<'_>) -> Result<Notice> {
        let (sqlstate, message) = (r.str()?, r.str()?);
        Ok(Notice { sqlstate, message, diagnostics: read_diagnostics(r)?.unwrap_or_default() })
    }
}

/// The diagnostics of a tail of diagnostics alone: none if it is empty.
pub(crate) fn read_diagnostics(r: &mut Reader<'_>) -> Result<Option<Diagnostics>> {
    let (mut tail, mut found) = (Tail::new(), None::<Diagnostics>);
    while let Some(tag) = tail.next(r, &Diagnostics::tags())? {
        found.get_or_insert_with(Diagnostics::default).read(tag, r)?;
    }
    Ok(found)
}
