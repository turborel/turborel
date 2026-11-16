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
//! GET DIAGNOSTICS, CALL, and in a function's body RETURN. A schema,
//! transaction, connection, session or dynamic statement, or one of a
//! cursor, is refused (IA078) [SR23f, SR23g], so SQLSTATE 2F003 is never
//! raised [§9.18 GR8b iii to vi]. So too a body that possibly reads
//! SQL-data of a routine of CONTAINS SQL (IA076), and one that possibly
//! modifies SQL-data of a routine of READS SQL DATA (IA075) [SR23c, d]:
//! so SQLSTATEs 2F002 and 2F004 are never raised [§9.18 GR6h, GR8b i, ii].
//!
//! \[DESIGN\] The owner's EXECUTE is grantable if every SELECT, INSERT,
//! UPDATE and DELETE it holds on each table and view the body names is,
//! and EXECUTE on each routine the body invokes [GR1].

use std::cell::RefCell;
use std::sync::Arc;

use super::super::{State, rule, triggers::named};
use crate::ast::{RoutineDefinition, SqlStatement};
use crate::binder::{Context, Invoked, Uses, bind_function_body};
use crate::catalog::{DataAccess, QualifiedName, SqlRoutineDesc, TransitionNames, applicable, owner_of, sole};
use crate::error::{Code, Result, SqlError};
use crate::params::{Params, RoutineScope};
use crate::value::Value;

/// What a routine's body, bound, names and does.
pub(super) struct BodyBound {
    /// The tables and views it names.
    pub(super) reads: Vec<QualifiedName>,
    /// The specific names of the routines it invokes.
    pub(super) invokes: Vec<QualifiedName>,
    /// Whether its owner's EXECUTE on the routine is grantable [GR1].
    pub(super) grantable: bool,
}

/// The order of what a body possibly does with SQL-data, least first.
fn rank(a: DataAccess) -> u8 {
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
        | SqlStatement::Call(_) => true,
        SqlStatement::Return(_) => return returning(desc),
        SqlStatement::Close(_)
        | SqlStatement::SetCollation(_)
        | SqlStatement::Commit
        | SqlStatement::DeletePositioned { .. }
        | SqlStatement::Truncate { .. }
        | SqlStatement::AlterTable { .. }
        | SqlStatement::Drop { .. }
        | SqlStatement::Schema { .. }
        | SqlStatement::Fetch { .. }
        | SqlStatement::Dynamic(_)
        | SqlStatement::Execute(_)
        | SqlStatement::DeclareTable(_)
        | SqlStatement::Open(_)
        | SqlStatement::Rollback
        | SqlStatement::SetTransaction(_)
        | SqlStatement::SetCatalog(_)
        | SqlStatement::SetSchema(_)
        | SqlStatement::SetTimeZone(_)
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
        "[ISO/IEC 9075-2:2023 §11.60 SR23f, SR23g] an SQL routine body is an INSERT, UPDATE, DELETE, MERGE, SELECT INTO, CALL, GET DIAGNOSTICS or a function's RETURN (IA078)",
    ))
}

/// What the statement `stmt` itself possibly does with SQL-data, of the
/// routines it invokes and the tables it names aside.
fn written(stmt: &SqlStatement) -> DataAccess {
    let modifies = matches!(
        stmt,
        SqlStatement::Insert(_) | SqlStatement::Merge(_) | SqlStatement::UpdateSearched { .. } | SqlStatement::DeleteSearched { .. }
    );
    if modifies {
        DataAccess::Modifies
    } else if matches!(stmt, SqlStatement::Select { .. }) {
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
    pub(super) fn bind_routine_body(&self, desc: &SqlRoutineDesc, definition: &RoutineDefinition) -> Result<BodyBound> {
        let stmt = &*definition.body;
        allowed(desc, stmt)?;
        let owner = owner_of(&*self.catalog, &desc.name.auth).to_owned();
        let (uses, invoked): (Uses, Invoked) = (Uses::default(), RefCell::new(Vec::new()));
        let scope = RoutineScope {
            name: desc.name.clone(),
            parameters: desc.parameters.iter().map(|p| (p.name.clone(), p.data_type.clone(), p.mode)).collect(),
            horizon: desc.horizon.clone(),
        };
        let params = Params::of_routine(vec![Value::Null; desc.parameters.len()], Arc::new(scope));
        let cx = Context {
            schema: &desc.schema,
            horizon: &desc.horizon,
            uses: Some(&uses),
            invoked: Some(&invoked),
            ..Context::new(self, &owner, &params)
        };
        let mut access = written(stmt);
        if let (true, SqlStatement::Return(_)) = (desc.function(), stmt) {
            access = bind_function_body(cx, desc, definition)?.access;
        } else {
            self.bind_triggered(cx, stmt, (&owner, &params))?;
        }
        let reads = named(std::slice::from_ref(stmt), None, (&desc.schema, &TransitionNames::default()));
        let invocations = invoked.into_inner();
        for r in invocations.iter().filter_map(|s| self.catalog.routine(s)) {
            access = if rank(r.access) > rank(access) { r.access } else { access };
        }
        if !reads.is_empty() && rank(access) < rank(DataAccess::Reads) {
            access = DataAccess::Reads;
        }
        within(desc, access)?;
        let grantable = self.execute_grantable(&owner, &reads, &invocations);
        Ok(BodyBound { reads, invokes: invocations, grantable })
    }

    /// [ISO/IEC 9075-2:2023 §11.60 GR1] Whether `owner` holds grantable
    /// every privilege the body that names `reads` and invokes `invokes`
    /// may use, as the module says.
    fn execute_grantable(&self, owner: &str, reads: &[QualifiedName], invokes: &[QualifiedName]) -> bool {
        let tables = reads.iter().all(|t| {
            let (held, grantable) = applicable(&*self.catalog, sole(&*self.catalog, owner), t);
            (!held.select || grantable.select)
                && (!held.delete || grantable.delete)
                && held.insert.is_subset(&grantable.insert)
                && held.update.is_subset(&grantable.update)
        });
        tables && invokes.iter().filter_map(|s| self.catalog.routine(s)).all(|r| r.execute_of(sole(&*self.catalog, owner)).1)
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
