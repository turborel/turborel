//! \[DESIGN\] A procedure's body kept between the CALLs a session makes
//! of it (RULES.md D406).
//!
//! The text of a procedure is read once while it stays the same, not at
//! each CALL. Its body, a control statement, is bound once as a program
//! and kept with what it was bound with, as a prepared statement's plan
//! is kept (`prepared/plans.rs`, `Key`): a CALL that finds any of it
//! changed binds the body again. A binding that reads the statement's
//! instant holds it, and is not kept. Each INSERT, searched UPDATE and
//! searched DELETE of a program kept keeps its own plan the same way,
//! its SQL parameters and variables slots each execution fills; each leaf
//! that is one probe of a unique key keeps its physical plan, and each
//! other leaf over the frame's slots learns one (`kept/leaves.rs`,
//! RULES.md D408, D414). Each local cursor keeps its specification and
//! plan as a prepared cursor does, opened from them (RULES.md D416).

mod leaves;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use super::super::prepared::{Key, Plans};
use super::super::{Session, State};
use crate::ast::{RoutineDefinition, SqlStatement};
use crate::binder::{definition_of, sql_statement};
use crate::catalog::{QualifiedName, SqlRoutineDesc};
use crate::error::Result;
use crate::logical::{Contained, Cursor, Program};
use crate::sync::lock;
use leaves::KeptLeaves;
pub(in crate::db) use leaves::KeptRuns;

/// The procedures a session keeps the bodies of, by specific name.
#[derive(Debug, Default)]
pub(in crate::db) struct KeptRoutines(HashMap<QualifiedName, Arc<KeptRoutine>>);

/// A procedure's text, read, and its body's program once bound.
#[derive(Debug)]
pub(in crate::db) struct KeptRoutine {
    text: String,
    module: Option<String>,
    definition: RoutineDefinition,
    program: Mutex<Option<(Key, KeptProgram)>>,
}

/// A program kept, and the plans its statements keep, by their place
/// among its statements (`Program::contained`).
#[derive(Debug, Clone)]
pub(in crate::db) struct KeptProgram {
    pub(in crate::db) program: Arc<Program>,
    plans: Arc<[Plans]>,
    /// The plans its local cursors keep, by their number (RULES.md D416).
    cursors: Arc<[Plans]>,
    /// The plans its leaves keep (`kept/leaves.rs`).
    pub(in crate::db) leaves: Arc<KeptLeaves>,
}

impl KeptProgram {
    /// `program`, its statements keeping no plan yet.
    pub(in crate::db) fn of(program: Program) -> KeptProgram {
        let plans = program.contained().iter().map(|_| Plans::default()).collect();
        let cursors = program.cursors.iter().map(|_| Plans::default()).collect();
        KeptProgram { program: Arc::new(program), plans, cursors, leaves: Arc::default() }
    }

    /// The plans the local cursor `c` of its program keeps, if it is one of
    /// its own.
    pub(in crate::db) fn cursor_plans(&self, c: &Cursor) -> Option<&Plans> {
        let at = self.program.cursors.iter().position(|own| std::ptr::eq(own, c))?;
        self.cursors.get(at)
    }

    /// The plans the statement `c` of its program keeps, if it is one of
    /// its own.
    pub(in crate::db) fn plans_of(&self, c: &Contained) -> Option<&Plans> {
        let at = self.program.contained().iter().position(|own| std::ptr::eq(*own, c))?;
        self.plans.get(at)
    }
}

impl KeptRoutine {
    /// Its body's statement.
    ///
    /// # Errors
    ///
    /// As `binder::sql_statement`.
    pub(in crate::db) fn body(&self) -> Result<&SqlStatement> {
        sql_statement(&self.definition)
    }

    /// Whether its body runs at a savepoint level of its own.
    pub(in crate::db) fn new_savepoint_level(&self) -> bool {
        self.definition.new_savepoint_level
    }

    /// Its program, kept with what holds now for `session` in `st`; else
    /// as `bind` binds it, and whether that binding holds a value it read,
    /// kept if it holds none.
    ///
    /// # Errors
    ///
    /// The binding's; SQLCODE -910 if a lock is poisoned.
    pub(in crate::db) fn program(
        &self,
        (st, session): (&State, &Session),
        bind: impl FnOnce() -> Result<(Program, bool)>,
    ) -> Result<KeptProgram> {
        if let Some((_, kept)) = lock(&self.program)?.as_ref().filter(|(key, _)| key.holds(st, session)) {
            return Ok(kept.clone());
        }
        let (program, held) = bind()?;
        let kept = KeptProgram::of(program);
        if !held {
            *lock(&self.program)? = Some((Key::now(st, session), kept.clone()));
        }
        Ok(kept)
    }
}

impl Session {
    /// The procedure `desc`, its text read: as kept while its text stays
    /// the same, else read again and kept.
    ///
    /// # Errors
    ///
    /// As `binder::definition_of`.
    pub(in crate::db) fn kept_routine(&self, desc: &SqlRoutineDesc) -> Result<Arc<KeptRoutine>> {
        let found = self.inner.borrow().routines.0.get(&desc.specific).cloned();
        if let Some(kept) = found.filter(|k| k.text == desc.text && k.module == desc.module) {
            return Ok(kept);
        }
        let definition = definition_of(desc)?;
        let kept = Arc::new(KeptRoutine { text: desc.text.clone(), module: desc.module.clone(), definition, program: Mutex::new(None) });
        self.inner.borrow_mut().routines.0.insert(desc.specific.clone(), Arc::clone(&kept));
        Ok(kept)
    }
}
