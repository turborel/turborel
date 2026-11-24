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
//! its SQL parameters and variables slots each execution fills, as does
//! the one a SELECT ... INTO reads as its delta table (RULES.md D792), and
//! the SELECT ... INTO's query over that table its own (D793); each leaf
//! that is one probe of a unique key keeps its physical plan, and each
//! other leaf over the frame's slots learns one (`kept/leaves.rs`,
//! RULES.md D408, D414). Each local cursor keeps its specification and
//! plan as a prepared cursor does, opened from them (RULES.md D416).

mod leaves;

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use super::super::prepared::{DeltaQuery, Key, Plans};
use super::super::{Session, State};
use super::program::Compiled;
use crate::ast::{RoutineDefinition, SqlStatement};
use crate::binder::{definition_of, sql_statement};
use crate::catalog::{QualifiedName, SqlRoutineDesc};
use crate::debugger::{Kind, Origin};
use crate::error::Result;
use crate::logical::{Contained, Cursor, Program};
use crate::sync::lock;
use leaves::KeptLeaves;
pub(in crate::db) use leaves::{KeptRuns, LeafPlans, point_of};

/// The procedures a session keeps the bodies of, by specific name, and
/// the prepared CALLs it runs straight (`direct.rs`).
#[derive(Debug, Default)]
pub(in crate::db) struct KeptRoutines(HashMap<QualifiedName, Arc<KeptRoutine>>, pub(super) super::direct::KeptCalls);

/// A procedure's text, read, and its body's program once bound.
#[derive(Debug)]
pub(in crate::db) struct KeptRoutine {
    text: String,
    module: Option<String>,
    definition: RoutineDefinition,
    program: Mutex<Option<(Key, KeptProgram)>>,
    /// \[DESIGN\] The routine, as a debugger names it (RULES.md D1004).
    pub(in crate::db) origin: Origin,
}

/// A program kept, and the plans its statements keep, by their place
/// among its statements (`Program::contained`).
#[derive(Debug, Clone)]
pub(in crate::db) struct KeptProgram {
    pub(in crate::db) program: Arc<Program>,
    plans: Arc<[Plans]>,
    /// The query of each, if a SELECT ... INTO of a delta table, its plans
    /// and the rows it reads (RULES.md D793).
    queries: Arc<[DeltaQuery]>,
    /// The plans its local cursors keep, by their number (RULES.md D416).
    cursors: Arc<[Plans]>,
    /// The plans its leaves keep (`kept/leaves.rs`).
    pub(in crate::db) leaves: Arc<KeptLeaves>,
    /// The address of each of its statements, with its place among them,
    /// in the order of the addresses: a statement run is found by its
    /// address, not by walking the program each time.
    places: Arc<[(usize, usize)]>,
    /// Its body compiled as it is first run so (RULES.md D841); none if
    /// it is not compiled.
    compiled: Arc<OnceLock<Option<Compiled>>>,
}

impl KeptProgram {
    /// `program`, its statements keeping no plan yet.
    pub(in crate::db) fn of(program: Program) -> KeptProgram {
        let plans = program.contained().iter().map(|_| Plans::default()).collect();
        let queries = program.contained().iter().map(|_| DeltaQuery::default()).collect();
        let cursors = program.cursors.iter().map(|_| Plans::default()).collect();
        let program = Arc::new(program);
        // The program's statements stay where they are while it is shared.
        let mut places: Vec<(usize, usize)> = program.contained().iter().enumerate().map(|(at, c)| (c.address(), at)).collect();
        places.sort_unstable();
        KeptProgram { program, plans, queries, cursors, leaves: Arc::default(), places: places.into(), compiled: Arc::default() }
    }

    /// Its body compiled, compiled the first time it is asked for; none if
    /// it is not compiled (RULES.md D841).
    pub(in crate::db) fn compiled(&self) -> Option<&Compiled> {
        self.compiled.get_or_init(|| Compiled::of(&self.program)).as_ref()
    }

    /// The plans the local cursor `c` of its program keeps, if it is one of
    /// its own.
    pub(in crate::db) fn cursor_plans(&self, c: &Cursor) -> Option<&Plans> {
        let at = self.program.cursors.iter().position(|own| std::ptr::eq(own, c))?;
        self.cursors.get(at)
    }

    /// The plans the statement `c` of its program keeps, if it is one of
    /// its own, and those its query over a delta table keeps, if it has
    /// one (RULES.md D793).
    pub(in crate::db) fn plans_of(&self, c: &Contained) -> Option<(&Plans, &DeltaQuery)> {
        let address = c.address();
        let found = self.places.binary_search_by_key(&address, |&(a, _)| a).ok()?;
        let &(_, at) = self.places.get(found)?;
        self.plans.get(at).zip(self.queries.get(at))
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

    /// Its program, if it keeps one with what holds now for `session` in
    /// `st`. It binds none.
    pub(in crate::db) fn current(&self, st: &State, session: &Session) -> Option<KeptProgram> {
        let kept = lock(&self.program).ok()?;
        let (with, program) = kept.as_ref()?;
        with.holds(st, session).then(|| program.clone())
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
        let kind = if desc.function() { Kind::Function } else { Kind::Procedure };
        let origin = Origin { kind, name: desc.specific.clone(), shown: desc.name.clone() };
        let (text, module) = (desc.text.clone(), desc.module.clone());
        let kept = Arc::new(KeptRoutine { text, module, definition, program: Mutex::new(None), origin });
        self.inner.borrow_mut().routines.0.insert(desc.specific.clone(), Arc::clone(&kept));
        Ok(kept)
    }
}
