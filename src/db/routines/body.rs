//! [ISO/IEC 9075-2:2023 §11.60 SR23, GR1, §4.35.2] The SQL routine body
//! of an SQL-invoked routine, bound as the routine is defined, and again
//! as a privilege it needs is revoked or what it reads is dropped.
//!
//! The body is bound under the owner of the routine's schema [GR3u], with
//! the names it writes with no schema of the schema it was defined with,
//! and sees the columns its descriptor's horizon gives. A function's
//! `<return statement>` is bound as its invocation binds it
//! (`binder/invocation/body.rs`); any other statement, of a procedure or
//! a function, as a triggered action's is, with the routine's parameters
//! its dynamic values (`params/routine.rs`).
//!
//! The statement is one an SQL routine body may contain: INSERT, a
//! searched UPDATE or DELETE, MERGE, a `<select statement: single row>`,
//! GET DIAGNOSTICS, CALL, a savepoint's statement [§9.18 GR8b iv 2], and
//! in a function's body RETURN. A schema, connection, session or dynamic
//! statement, COMMIT, ROLLBACK but to a savepoint, or one of a cursor, is
//! refused (IA078) [SR23f, SR23g], so SQLSTATE 2F003 is never raised
//! [§9.18 GR8b iii to vi]. So too a body that possibly reads
//! SQL-data of a routine of CONTAINS SQL (IA076), and one that possibly
//! modifies SQL-data of a routine of READS SQL DATA (IA075) [SR23c, d]:
//! so SQLSTATEs 2F002 and 2F004 are never raised [§9.18 GR6h, GR8b i, ii].
//!
//! \[DESIGN\] The owner's EXECUTE is grantable if every SELECT, INSERT,
//! UPDATE and DELETE it holds on each table and view the body names is,
//! and EXECUTE on each routine the body invokes [GR1].

use std::cell::RefCell;
use std::sync::Arc;

use super::super::triggers::{named, named_by_values};
use super::super::{State, rule};
use super::defaults;
use crate::ast::{RoutineDefinition, SqlStatement};
use crate::binder::{
    Context, Counting, DrawnFrom, Invoked, Uses, bind_function_body, definition_of, grantably_held, once_each, sql_statement,
};
use crate::catalog::{DataAccess, Indicated, QualifiedName, SqlRoutineDesc, TransitionNames, applicable, owner_of, routine_path, sole};
use crate::error::{Code, Result, SqlError};
use crate::params::{Params, RoutineScope};
use crate::value::Value;

/// What a routine's body, bound, names and does.
pub(in crate::db) struct BodyBound {
    /// The tables and views it names.
    pub(in crate::db) reads: Vec<QualifiedName>,
    /// The specific names of the routines it invokes.
    pub(in crate::db) invokes: Vec<QualifiedName>,
    /// [ISO/IEC 9075-14:2006 draft §4.2.6] The registered XML Schemas it
    /// indicates, and the registered XQuery modules it imports.
    pub(in crate::db) indicates: Indicated,
    /// Whether its XQuery expressions were typed statically as
    /// XQUERY.STATIC_TYPING asked [XQuery §5.2.3].
    pub(in crate::db) typed: bool,
    /// The sequence generators it draws from, each once.
    pub(in crate::db) sequences: Vec<QualifiedName>,
    /// Whether its owner's EXECUTE on the routine is grantable [GR1].
    pub(in crate::db) grantable: bool,
    /// The columns of tables and views it references, each once.
    pub(in crate::db) columns: Vec<(QualifiedName, String)>,
}

/// The order of what a body possibly does with SQL-data, least first.
pub(super) fn rank(a: DataAccess) -> u8 {
    match a {
        DataAccess::Contains => 0,
        DataAccess::Reads => 1,
        DataAccess::Modifies => 2,
    }
}

/// [ISO/IEC 9075-2:2023 §16.2 SR1] That `desc`, whose body is a `<return
/// statement>`, is a function.
fn returning(desc: &SqlRoutineDesc) -> Result<()> {
    if desc.function() {
        return Ok(());
    }
    Err(rule(format!("[ISO/IEC 9075-2:2023 §16.2 SR1] procedure {} contains a <return statement>: only a function's does", desc.name)))
}

/// [ISO/IEC 9075-2:2023 §11.60 SR23f, SR23g, §16.2 SR1] That `stmt` is one
/// the body of `desc` may be (IA078).
fn allowed(desc: &SqlRoutineDesc, stmt: &SqlStatement) -> Result<()> {
    let may = match stmt {
        SqlStatement::Insert(_)
        | SqlStatement::Merge(_)
        | SqlStatement::UpdateSearched { .. }
        | SqlStatement::DeleteSearched { .. }
        | SqlStatement::Select { .. }
        | SqlStatement::GetDiagnostics(_)
        | SqlStatement::Call(_)
        // [ISO/IEC 9075-4:2006 draft §11.60 SR23] each statement within is
        // one a body may contain, as the parser read it.
        | SqlStatement::Control(_)
        | SqlStatement::Signal(_)
        // [ISO/IEC 9075-2:2023 §9.18 GR8b iv 2] SAVEPOINT, RELEASE
        // SAVEPOINT and ROLLBACK TO SAVEPOINT, with Feature T272.
        | SqlStatement::Savepoint(_)
        // [ISO/IEC 9075-2:2023 §11.60 SR23f] (IA078) a statement of a PTF
        // data area alone, as the parser read it, of a PTF component
        // procedure.
        | SqlStatement::Dynamic(_) => true,
        SqlStatement::Return(_) => return returning(desc),
        SqlStatement::Close(_)
        | SqlStatement::SetCollation(_)
            | SqlStatement::SetTransformGroup(_)
        | SqlStatement::Commit
        | SqlStatement::DeletePositioned { .. }
        | SqlStatement::Truncate { .. }
        | SqlStatement::AlterTable { .. }
        | SqlStatement::Drop { .. }
        | SqlStatement::Schema { .. }
        | SqlStatement::Fetch { .. }
        | SqlStatement::Execute(_)
            | SqlStatement::ExecuteGql(_)
        | SqlStatement::DeclareTable(_)
        | SqlStatement::Open(_)
        | SqlStatement::Rollback
        | SqlStatement::SetTransaction(_)
        | SqlStatement::SetCatalog(_)
        | SqlStatement::SetSchema(_)
        | SqlStatement::SetTimeZone(_)
        | SqlStatement::Locator { .. }
        | SqlStatement::SetSessionAuthorization(_)
        | SqlStatement::SetRole(_)
        | SqlStatement::Connection(_)
        | SqlStatement::SetSetting { .. }
        | SqlStatement::SetDatabase { .. }
        | SqlStatement::Index(_)
        | SqlStatement::User(_)
        | SqlStatement::Served(_)
        | SqlStatement::SetConstraints { .. }
        | SqlStatement::UpdatePositioned { .. } => false,
    };
    if may {
        return Ok(());
    }
    Err(rule(
        "[ISO/IEC 9075-2:2023 §11.60 SR23f, SR23g; ISO/IEC 9075-4:2006 draft §11.60 SR23] an SQL routine body is an INSERT, UPDATE, DELETE, MERGE, SELECT INTO, CALL, GET DIAGNOSTICS, SQL control statement, SIGNAL, RESIGNAL, savepoint's statement, a statement of a PTF data area or a function's RETURN (IA078)",
    ))
}

/// What the statement `stmt` itself possibly does with SQL-data, of the
/// routines it invokes and the tables it names aside.
pub(super) fn written(stmt: &SqlStatement) -> DataAccess {
    let modifies = matches!(
        stmt,
        SqlStatement::Insert(_)
            | SqlStatement::Merge(_)
            | SqlStatement::UpdateSearched { .. }
            | SqlStatement::DeleteSearched { .. }
            | SqlStatement::UpdatePositioned { .. }
            | SqlStatement::DeletePositioned { .. }
    );
    if modifies {
        DataAccess::Modifies
    } else if matches!(stmt, SqlStatement::Select { .. } | SqlStatement::Open(_) | SqlStatement::Fetch { .. }) {
        DataAccess::Reads
    } else {
        DataAccess::Contains
    }
}

impl State {
    /// [ISO/IEC 9075-2:2023 §11.60 SR23, GR1] The body `definition` holds
    /// of the routine `desc`, bound as the module says.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it is no statement a body may be, or does more with
    /// SQL-data than its SQL-data access indication allows; its own,
    /// bound, under the routine's owner.
    pub(in crate::db) fn bind_routine_body(&self, desc: &SqlRoutineDesc, definition: &RoutineDefinition) -> Result<BodyBound> {
        self.bind_routine_body_counting(desc, definition, None)
    }

    /// `bind_routine_body`, its columns those `counting` counts, if not
    /// each it references.
    ///
    /// # Errors
    ///
    /// As `bind_routine_body`.
    fn bind_routine_body_counting(
        &self,
        desc: &SqlRoutineDesc,
        definition: &RoutineDefinition,
        counting: Option<Counting>,
    ) -> Result<BodyBound> {
        if let Some(ptf) = &desc.ptf {
            return self.bind_ptf_body(desc, ptf);
        }
        let stmt = sql_statement(definition)?;
        allowed(desc, stmt)?;
        let owner = owner_of(&*self.catalog, &desc.name.auth).to_owned();
        let (uses, invoked, drawn): (Uses, Invoked, DrawnFrom) = (Uses::default(), Invoked::default(), RefCell::new(Vec::new()));
        let scope = RoutineScope::of(desc, routine_path(&*self.catalog, desc));
        let params = Params::of_routine(vec![Value::Null; desc.parameters.len()], Arc::new(scope));
        let cx = Context {
            schema: &desc.schema,
            horizon: &desc.horizon,
            uses: Some(&uses),
            counting: counting.map(|c| (c, false)),
            invoked: Some(&invoked),
            drawn: Some(&drawn),
            ..Context::new(self, &owner, &params)
        };
        let mut access = super::program::written(stmt);
        if let (true, SqlStatement::Return(_)) = (desc.function(), stmt) {
            access = bind_function_body(cx, desc, definition)?.access;
        } else if let SqlStatement::Control(_) | SqlStatement::Signal(_) = stmt {
            let returns = desc.function().then(|| desc.returns.clone()).flatten();
            self.bind_program_checked(cx, stmt, (&owner, &params), returns)?;
            // As each invocation binds it, its CALLs with it.
            if desc.function() {
                bind_function_body(Context { uses: None, invoked: None, drawn: None, ..cx }, desc, definition)?;
            }
        } else {
            self.bind_triggered(cx, stmt, (&owner, &params))?;
        }
        let defaulted = Invoked::default();
        self.bind_defaults(Context { invoked: Some(&defaulted), ..cx }, desc, definition)?;
        let mut reads = named(std::slice::from_ref(stmt), None, (&desc.schema, &TransitionNames::default()));
        let indicates = invoked.take_indicated();
        let typed = invoked.take_typed();
        let mut invocations = invoked.into_inner();
        for r in invocations.iter().filter_map(|s| self.catalog.routine(s)) {
            access = if rank(r.access) > rank(access) { r.access } else { access };
        }
        if !reads.is_empty() && rank(access) < rank(DataAccess::Reads) {
            access = DataAccess::Reads;
        }
        within(desc, access)?;
        defaults_named((definition, &desc.schema), defaulted, (&mut reads, &mut invocations));
        let grantable = self.execute_grantable((&owner, desc.module_name()), &reads, &invocations);
        let mut sequences = drawn.into_inner();
        sequences.sort();
        sequences.dedup();
        let columns = once_each(uses.into_inner());
        Ok(BodyBound { reads, invokes: invocations, indicates, typed, sequences, grantable, columns })
    }

    /// The columns of tables and views the body of the routine `r`
    /// references, each once, as it binds now: those an SQL-server module's
    /// routines reference [ISO/IEC 9075-4:2006 draft §19.1].
    ///
    /// # Errors
    ///
    /// The body does not bind.
    pub(crate) fn routine_columns(&self, r: &SqlRoutineDesc) -> Result<Vec<(QualifiedName, String)>> {
        Ok(self.bind_routine_body(r, &definition_of(r)?)?.columns)
    }

    /// [ISO/IEC 9075-11:2006 draft §6.35 Description 1] The columns the
    /// body of the routine `r` references in the search condition of a
    /// searched UPDATE or DELETE or in an `<update source>`, and those an
    /// `<insert column list>` or a `<set clause>` names, each once, as it
    /// binds now.
    ///
    /// # Errors
    ///
    /// The body does not bind.
    pub(crate) fn routine_usage_columns(&self, r: &SqlRoutineDesc) -> Result<Vec<(QualifiedName, String)>> {
        Ok(self.bind_routine_body_counting(r, &definition_of(r)?, Some(Counting::Routine))?.columns)
    }

    /// [ISO/IEC 9075-2:2023 §11.60 GR1; ISO/IEC 9075-4:2006 draft §9.21
    /// GR2] Whether `owner` holds grantable every privilege the body that
    /// names `reads` and invokes `invokes` may use, as the module says: of
    /// a routine of the module `module`, the routines of the module it
    /// invokes aside, whose privilege is the one decided.
    pub(in crate::db) fn execute_grantable(
        &self,
        (owner, module): (&str, Option<QualifiedName>),
        reads: &[QualifiedName],
        invokes: &[QualifiedName],
    ) -> bool {
        let tables = reads.iter().all(|t| {
            let (held, grantable) = applicable(&*self.catalog, sole(&*self.catalog, owner), t);
            (!held.select || grantable.select)
                && (!held.delete || grantable.delete)
                && held.insert.is_subset(&grantable.insert)
                && held.update.is_subset(&grantable.update)
        });
        let others = invokes.iter().filter_map(|s| self.catalog.routine(s)).filter(|r| module.is_none() || r.module_name() != module);
        tables && others.into_iter().all(|r| grantably_held(self, r, sole(&*self.catalog, owner)))
    }
}

/// [ISO/IEC 9075-2:2023 §11.60 SR23c, SR23d] That a body that possibly
/// does `access` is within the SQL-data access indication of `desc`
/// (IA075, IA076).
fn within(desc: &SqlRoutineDesc, access: DataAccess) -> Result<()> {
    let refused = |rule_of: &str, does: &str, declared: &str, ia: &str| {
        let msg = format!(
            "[ISO/IEC 9075-2:2023 §11.60 {rule_of}] the body of {} possibly {does} SQL-data, and it is {declared} ({ia})",
            desc.name
        );
        Err(SqlError::new(Code::SyntaxRule, msg))
    };
    match (desc.access, access) {
        (DataAccess::Contains, DataAccess::Reads) => refused("SR23d", "reads", "CONTAINS SQL", "IA076"),
        (DataAccess::Contains, DataAccess::Modifies) => refused("SR23d", "modifies", "CONTAINS SQL", "IA076"),
        (DataAccess::Reads, DataAccess::Modifies) => refused("SR23c", "modifies", "READS SQL DATA", "IA075"),
        (DataAccess::Contains, DataAccess::Contains)
        | (DataAccess::Reads, DataAccess::Contains | DataAccess::Reads)
        | (DataAccess::Modifies, DataAccess::Contains | DataAccess::Reads | DataAccess::Modifies) => Ok(()),
    }
}

/// [ISO/IEC 9075-2:2023 §11.60 SR16e] Adds to `reads` the tables and views
/// the parameter defaults of `definition`, of schema `schema`, name, and
/// to `invocations` the routines `defaulted` noted they invoke, each once:
/// what a CALL evaluates, not the body, so no part of its access.
fn defaults_named(
    (definition, schema): (&RoutineDefinition, &str),
    defaulted: Invoked,
    (reads, invocations): (&mut Vec<QualifiedName>, &mut Vec<QualifiedName>),
) {
    for t in named_by_values(defaults::default_values(definition), schema) {
        if !reads.contains(&t) {
            reads.push(t);
        }
    }
    for s in defaulted.into_inner() {
        if !invocations.contains(&s) {
            invocations.push(s);
        }
    }
}
