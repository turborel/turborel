//! [ISO/IEC 9075-2:2023 §11.49, §11.50] CREATE TRIGGER and DROP TRIGGER.
//!
//! A trigger's action is bound as it is defined, under the owner of the
//! trigger's schema [§15.21 GR6b], with the transition variables and
//! tables its trigger gives: so its Syntax and Access Rules are checked
//! then, and what it names, each table, view and column, is kept with
//! the descriptor, for what is dropped with what it names
//! (`dependents.rs`).
//!
//! Its statements are those a triggered action may run: INSERT, a
//! searched UPDATE or DELETE, MERGE, a `<select statement: single row>`
//! and GET DIAGNOSTICS. A schema, transaction, connection, session or
//! dynamic statement, or one of a cursor, is refused as the trigger is
//! defined (IA046), so 0W000 is never raised as it runs [§15.21 GR6e iii];
//! so too a BEFORE trigger's statement that changes data (IA047, GR6h).

use std::sync::Arc;

use super::super::ddl::Dropped;
use super::super::definitions::Defined;
use super::super::generated::Inserted;
use super::super::merge::{Inserting, inserting};
use super::super::write::{INSERT_RULES, InsertInto, insert_target_in};
use super::super::{State, assignment::check_targets, rule};
use super::named::named;
use super::rerouted::bind_taken;
use super::run::{parsed, transitions_of};
use crate::ast::{InsertSource, InsertStatement, MergeStatement, Schema, SqlStatement, TriggerDefinition, TriggerEvent as Written};
use crate::binder::{
    Context, DrawnFrom, Invoked, ObjectColumn, Uses, WriteKind, bind_insert_query, bind_merge, bind_select, bind_when, bind_write,
    once_each, qualify,
};
use crate::catalog::{
    ActionTime, Horizon, Principal, QualifiedName, TransitionNames, TriggerDesc, TriggerEvent, ViewDesc, applicable, owner_of, sole,
};
use crate::error::{At, Code, Result, SqlError, Subject};
use crate::exec::TransitionRows;
use crate::params::Params;
use crate::txn::LockMode;
use crate::value::Value;
use crate::value::datetime::Now;

/// What a triggered action names: the tables and views, the columns, its
/// triggered action column set, and the sequence generators it draws from,
/// each once.
type Reads = (Vec<QualifiedName>, Vec<(QualifiedName, String)>, Vec<QualifiedName>);

impl State {
    /// [ISO/IEC 9075-2:2023 §11.49] CREATE TRIGGER `t`, its names without
    /// a schema in `schema`: the trigger's descriptor added [GR5], and of
    /// an INSTEAD OF trigger the privileges of GR6.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 for a Syntax Rule it breaks [SR2 to SR6, SR13 to SR15];
    /// -104 if the schema's owner lacks TRIGGER on the table, or SELECT
    /// of a trigger with transition names [AR2, AR3]; its action's, bound.
    pub(in crate::db) fn create_trigger(&mut self, schema: &str, t: &TriggerDefinition) -> Result<Defined> {
        let name = qualify(&t.name, schema);
        let table = qualify(&t.table, schema);
        if self.catalog.trigger(&name).is_some() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.49 SR4] schema {} has a trigger {} already", name.auth, name.ident))
                .or_at(t.name.at));
        }
        self.subject_table(t, &table).or_at(t.table.at)?;
        let event = self.trigger_event(t, &table)?;
        let owner = owner_of(&*self.catalog, &name.auth).to_owned();
        let names = TransitionNames {
            old_row: t.referencing.old_row.as_ref().map(|(n, _)| n.clone()),
            new_row: t.referencing.new_row.as_ref().map(|(n, _)| n.clone()),
            old_table: t.referencing.old_table.as_ref().map(|(n, _)| n.clone()),
            new_table: t.referencing.new_table.as_ref().map(|(n, _)| n.clone()),
        };
        self.trigger_privileges(&owner, &table, &names).or_at(t.table.at)?;
        let created = self.catalog.triggers().iter().map(|d| d.created).max().unwrap_or(0).saturating_add(1);
        let mut desc = TriggerDesc {
            name: name.clone(),
            table: table.clone(),
            time: t.time,
            level: t.level,
            event,
            names,
            text: t.text.clone(),
            schema: schema.to_owned(),
            reads: Vec::new(),
            horizon: Horizon::default(),
            columns: Vec::new(),
            sequences: Vec::new(),
            created,
        };
        let invoked = Invoked::default();
        let (reads, columns, sequences) = self.bind_action(&desc, t, (&owner, &invoked))?;
        let seen: Vec<QualifiedName> = reads.iter().chain([&table]).cloned().collect();
        // [ISO/IEC 9075-2:2023 §4.35.2] and its subject routines from now on.
        desc.horizon = Horizon::of(&*self.catalog, &seen).invoking(invoked.take());
        (desc.reads, desc.columns, desc.sequences) = (reads, columns, sequences);
        let locks = desc.reads.iter().map(|r| (r.clone(), LockMode::Shared)).chain([(table, LockMode::Exclusive)]).collect();
        let mut catalog = self.catalog.duplicate();
        let instead = desc.instead().then(|| desc.clone());
        catalog.put_trigger(desc);
        self.catalog = Arc::from(catalog);
        if let Some(given) = instead {
            self.grant_instead(&given, &owner);
        }
        Ok(Defined { locks, ..Defined::default() })
    }

    /// The triggers of `<schema definition>` `s`, made once its tables,
    /// views, grants and assertions are, in the order written.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if one is named in another schema [SR1a]; as
    /// `create_trigger` says.
    pub(in crate::db) fn schema_triggers(&mut self, s: &Schema) -> Result<()> {
        for t in &s.triggers {
            if let Some(other) = t.name.auth.as_ref().filter(|o| **o != s.name) {
                let msg = format!("[ISO/IEC 9075-2:2023 §11.49 SR1a] trigger {} is in schema {other}, not {}", t.name.ident, s.name);
                return Err(rule(msg).or_at(t.name.at));
            }
            self.create_trigger(&s.name, t)?;
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.49 SR2, SR3, SR5, SR15c, SR15d] That the
    /// subject table `table` of `t` exists and may have its trigger.
    fn subject_table(&self, t: &TriggerDefinition, table: &QualifiedName) -> Result<()> {
        let base = self.catalog.table(table);
        let view = self.catalog.view(table);
        if t.time != ActionTime::InsteadOf {
            return match base {
                Some(d) if d.temporary.is_none() => Ok(()),
                Some(_) | None if view.is_some() => {
                    Err(rule(format!("[ISO/IEC 9075-2:2023 §11.49 SR5b] {table} is a view: only an INSTEAD OF trigger is on a view")))
                }
                Some(_) => Err(rule(format!("[ISO/IEC 9075-2:2023 §11.49 SR5b] {table} is a temporary table"))),
                None => Err(rule(format!("[ISO/IEC 9075-2:2023 §11.49 SR3] there is no table {table}"))),
            };
        }
        let Some(v) = view else {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.49 SR5a] {table} is no view: an INSTEAD OF trigger is on a view");
            return Err(rule(msg));
        };
        if v.check_option.is_some() {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.49 SR15c] view {table} is WITH CHECK OPTION")));
        }
        // A view WITH CHECK OPTION is updatable, so its generally underlying
        // tables are the chain of views each reads, down to a base table.
        let under = |w: &ViewDesc| {
            let mut at = w.updates.as_ref().map(|(u, _)| u.clone());
            while let Some(u) = at {
                if u == *table {
                    return true;
                }
                at = self.catalog.view(&u).and_then(|next| next.updates.as_ref().map(|(n, _)| n.clone()));
            }
            false
        };
        let checked = self.catalog.views().into_iter().find(|w| w.check_option.is_some() && under(w));
        if let Some(w) = checked {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.49 SR15d] view {} WITH CHECK OPTION reads view {table}", w.name)));
        }
        let event = |e: &TriggerEvent| std::mem::discriminant(e) == std::mem::discriminant(&event_of(&t.event));
        if let Some(other) = self.catalog.triggers_on(table).iter().find(|o| o.instead() && event(&o.event)) {
            let msg =
                format!("[ISO/IEC 9075-2:2023 §11.49 SR5a] view {table} has INSTEAD OF trigger {} for that event already", other.name);
            return Err(rule(msg));
        }
        Ok(())
    }

    /// [ISO/IEC 9075-2:2023 §11.49 SR6] The trigger event of `t`, its
    /// trigger column list's columns distinct columns of `table`.
    fn trigger_event(&self, t: &TriggerDefinition, table: &QualifiedName) -> Result<TriggerEvent> {
        let Written::Update(list) = &t.event else { return Ok(event_of(&t.event)) };
        let columns = self.catalog.table(table).map(|d| d.columns.clone()).unwrap_or_default();
        let mut names: Vec<String> = Vec::with_capacity(list.len());
        for (c, at) in list {
            if names.contains(c) {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.49 SR6a] column {c} is in the trigger column list twice")).or_at(*at));
            }
            if !columns.iter().any(|d| d.name == *c) {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.49 SR6b] {table} has no column {c}")).or_at(*at));
            }
            names.push(c.clone());
        }
        Ok(TriggerEvent::Update(names))
    }

    /// [ISO/IEC 9075-2:2023 §11.49 AR2, AR3] That `owner` holds TRIGGER on
    /// `table`, and SELECT if `names` gives a transition name.
    fn trigger_privileges(&self, owner: &str, table: &QualifiedName, names: &TransitionNames) -> Result<()> {
        let (held, _) = applicable(&*self.catalog, sole(&*self.catalog, owner), table);
        let lacks = |what: &str, rule: &str| {
            let msg = format!("[ISO/IEC 9075-2:2023 §11.49 {rule}] {owner}, the owner of the trigger's schema, lacks {what} on {table}");
            Err(SqlError::new(Code::Privilege, msg).about(Subject::table(table.parts())))
        };
        if !held.trigger {
            return lacks("TRIGGER", "AR2");
        }
        if *names != TransitionNames::default() && !held.select {
            return lacks("SELECT", "AR3");
        }
        Ok(())
    }

    /// The action of `t`, whose descriptor `desc` is, its WHEN condition
    /// and its statements bound under `owner`, as the module says, the
    /// routines they invoke noted in `invoked`: what it names.
    fn bind_action(&self, desc: &TriggerDesc, t: &TriggerDefinition, (owner, invoked): (&str, &Invoked)) -> Result<Reads> {
        let transitions = Arc::new(transitions_of(self, desc)?);
        let width = transitions.columns.len().saturating_mul(2);
        let params = Params::transitional(vec![Value::Null; width], transitions);
        let (uses, drawn) = (Uses::default(), DrawnFrom::default());
        let cx = || Context {
            schema: &desc.schema,
            uses: Some(&uses),
            invoked: Some(invoked),
            drawn: Some(&drawn),
            ..Context::new(self, owner, &params)
        };
        if let Some(when) = &t.when {
            bind_when(cx(), when)?;
        }
        for stmt in &t.action {
            allowed(self, stmt, (t.time, &desc.schema))?;
            self.bind_triggered(cx(), stmt, (owner, &params))?;
        }
        let reads = named(&t.action, t.when.as_ref(), (&desc.schema, &desc.names));
        let mut sequences = drawn.into_inner();
        sequences.sort();
        sequences.dedup();
        Ok((reads, once_each(uses.into_inner()), sequences))
    }

    /// The INSERT `ins` of a triggered action bound in `cx` as its owner
    /// `owner` would with `params`.
    fn bind_triggered_insert(&self, cx: Context<'_>, ins: &InsertStatement, (owner, params): (&str, &Params)) -> Result<()> {
        let given = Inserted::Source(&ins.source);
        let into = InsertInto {
            table: &ins.table,
            columns: ins.columns.as_deref(),
            given: (given, ins.overriding),
            rules: &INSERT_RULES,
            horizon: params.horizon(),
        };
        let target = insert_target_in(self, &into, (Principal::sole(self.catalog.roles(), owner), cx.schema, Now::default()), None)?;
        if let InsertSource::Query(q) = &ins.source {
            bind_insert_query(cx, q, &target.objects())?;
        }
        Ok(())
    }

    /// The MERGE `m` of a triggered action bound in `cx` as its owner
    /// `owner` would with `params`.
    fn bind_triggered_merge(&self, cx: Context<'_>, m: &MergeStatement, (owner, params): (&str, &Params)) -> Result<()> {
        let (targets, held) = inserting(self, m, ((Principal::sole(self.catalog.roles(), owner), cx.schema, Now::default()), params))?;
        let objects: Vec<Vec<ObjectColumn>> = targets.iter().map(Inserting::objects).collect();
        // Of a view its triggers take whole, over its rows, none read.
        let none = held.then(|| TransitionRows::new(self.list_space()));
        bind_merge(cx, m, &objects, none.as_ref()).map(drop)
    }

    /// `stmt` of a triggered action, or of the body of an SQL-invoked
    /// procedure, bound in `cx` as its owner `owner` would with `params`.
    pub(in crate::db) fn bind_triggered(&self, cx: Context<'_>, stmt: &SqlStatement, (owner, params): (&str, &Params)) -> Result<()> {
        // A change of a view only its INSTEAD OF triggers change.
        if bind_taken(cx, stmt)? {
            return Ok(());
        }
        match stmt {
            SqlStatement::Insert(ins) => self.bind_triggered_insert(cx, ins, (owner, params)),
            SqlStatement::Merge(m) => self.bind_triggered_merge(cx, m, (owner, params)),
            SqlStatement::UpdateSearched { .. } | SqlStatement::DeleteSearched { .. } => bind_searched(cx, stmt),
            SqlStatement::Select { query, targets } => {
                let bound = bind_select(cx, query)?;
                check_targets(&bound.columns, targets, params, "§8.10 SR3", "§8.10 SR4")
            }
            SqlStatement::GetDiagnostics(_) => Ok(()),
            SqlStatement::Control(_) | SqlStatement::Signal(_) => self.bind_program_checked(cx, stmt, (owner, params), None),
            // [ISO/IEC 9075-4:2006 draft §12.5, §12.6] of a local cursor,
            // which alone a statement within a compound statement names.
            SqlStatement::UpdatePositioned { table, set, .. } => bind_write(cx, WriteKind::UpdatePositioned, table, set, None).map(drop),
            SqlStatement::DeletePositioned { table, .. } => bind_write(cx, WriteKind::DeletePositioned, table, &[], None).map(drop),
            // [ISO/IEC 9075-2:2023 §11.49 SR13] a <call statement> is an
            // <SQL procedure statement> a triggered action may contain.
            SqlStatement::Call(call) => Self::bind_call(cx, call).map(drop),
            // [ISO/IEC 9075-2:2023 §11.60 SR23f] (IA078) of a procedure's
            // body, the statements of a PTF component procedure, on the
            // names of its PTF data area: its operands are its own SQL
            // variables, parameters and literals, found as it was read.
            SqlStatement::Dynamic(_) if params.routine_scope().is_some_and(|s| !s.name.ident.is_empty()) => Ok(()),
            SqlStatement::Return(_)
            | SqlStatement::Close(_)
            | SqlStatement::SetCollation(_)
            | SqlStatement::Commit
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
            | SqlStatement::Locator { .. }
            | SqlStatement::SetSessionAuthorization(_)
            | SqlStatement::SetRole(_)
            | SqlStatement::Connection(_)
            | SqlStatement::SetSetting { .. }
            | SqlStatement::SetDatabase { .. }
            | SqlStatement::Index(_)
            | SqlStatement::User(_)
            | SqlStatement::Served(_)
            | SqlStatement::SetConstraints { .. } => Err(refused()),
        }
    }

    /// [ISO/IEC 9075-2:2023 §12.7 SR3b] That trigger `t` may still be
    /// defined as it is: its owner holds the privileges it needs.
    ///
    /// # Errors
    ///
    /// As `create_trigger`'s, of its privileges and its action.
    pub(in crate::db) fn trigger_binds(&self, t: &TriggerDesc) -> Result<()> {
        let definition = parsed(&t.text)?;
        let owner = owner_of(&*self.catalog, &t.name.auth).to_owned();
        self.trigger_privileges(&owner, &t.table, &t.names)?;
        self.bind_action(t, &definition, (&owner, &Invoked::default())).map(drop)
    }

    /// [ISO/IEC 9075-2:2023 §11.50] DROP TRIGGER `qn`: its descriptor
    /// destroyed [GR2], and of an INSTEAD OF trigger the privileges it gave
    /// revoked [GR1].
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if there is no such trigger [SR1].
    pub(in crate::db) fn drop_trigger(&mut self, qn: &QualifiedName) -> Result<Dropped> {
        let Some(desc) = self.catalog.trigger(qn).cloned() else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.50 SR1] there is no trigger {qn}")));
        };
        let mut catalog = self.catalog.duplicate();
        catalog.remove_trigger(qn);
        self.catalog = Arc::from(catalog);
        let mut dropped = Dropped { names: vec![qn.clone(), desc.table.clone()], ..Dropped::default() };
        // Of a view dropped, its privileges went with it.
        if desc.instead() && self.catalog.view(&desc.table).is_some() {
            dropped.absorb(self.revoke_instead(&desc)?);
        }
        Ok(dropped)
    }
}

/// The trigger event of `e`, as its descriptor has it, its column list
/// left empty.
fn event_of(e: &Written) -> TriggerEvent {
    match e {
        Written::Insert => TriggerEvent::Insert,
        Written::Delete => TriggerEvent::Delete,
        Written::Update(_) => TriggerEvent::Update(Vec::new()),
    }
}

/// [ISO/IEC 9075-2:2023 §11.49 SR13, SR14a] That `stmt`, its names in
/// schema `schema`, is one a triggered action of a trigger of action time
/// `time` may run (IA046, IA047): of a BEFORE trigger, no CALL of a
/// procedure that possibly modifies SQL-data either.
fn allowed(st: &State, stmt: &SqlStatement, (time, schema): (ActionTime, &str)) -> Result<()> {
    let changes = matches!(
        stmt,
        SqlStatement::Insert(_) | SqlStatement::Merge(_) | SqlStatement::UpdateSearched { .. } | SqlStatement::DeleteSearched { .. }
    ) || matches!(stmt, SqlStatement::Call(call) if st.call_modifies(call, schema));
    if changes && time == ActionTime::Before {
        let msg = "[ISO/IEC 9075-2:2023 §11.49 SR14a] the triggered action of a BEFORE trigger changes no SQL-data (IA047)";
        return Err(rule(msg));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §11.49 SR13] A statement no triggered action may
/// contain (IA046).
fn refused() -> SqlError {
    SqlError::new(
        Code::SyntaxRule,
        "[ISO/IEC 9075-2:2023 §11.49 SR13] a triggered action contains only INSERT, UPDATE, DELETE, MERGE, SELECT INTO, CALL and GET DIAGNOSTICS (IA046)",
    )
}

/// `stmt`, a searched UPDATE or DELETE, bound as `bind_triggered` binds it.
fn bind_searched(cx: Context<'_>, stmt: &SqlStatement) -> Result<()> {
    if let SqlStatement::UpdateSearched { table, set, where_clause } = stmt {
        return bind_write(cx, WriteKind::UpdateSearched, table, set, where_clause.as_ref()).map(drop);
    }
    if let SqlStatement::DeleteSearched { table, where_clause } = stmt {
        return bind_write(cx, WriteKind::DeleteSearched, table, &[], where_clause.as_ref()).map(drop);
    }
    Ok(())
}
