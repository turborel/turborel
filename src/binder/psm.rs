//! [ISO/IEC 9075-4:2006 draft §11.2, Clause 14, §17.2, §17.3] An SQL
//! control statement bound as a program (`logical/program.rs`, RULES.md
//! D76).
//!
//! The variables of each compound statement take slots of the frame after
//! those of the variables declared before them, so no slot is used by two
//! variables in scope at once. Each statement within is bound in the
//! scope its place gives: the routine's parameters, or the trigger's
//! transition variables, then the variables and cursors of each compound
//! statement it is in, the innermost last (`params/routine.rs`). A value,
//! condition or query is a leaf, a subquery of no table whose references
//! to variables and parameters are outer references, slots of the frame;
//! so a column of a table the query reads hides a variable of its name
//! [Part 4 §6.2 SR3]. A statement the session runs keeps its syntax and
//! its scope, bound as it runs.

mod handlers;
mod inline;
mod steps;

use std::collections::BTreeSet;
use std::sync::Arc;

use super::{Binder, Context, OutColumn, Scope, rule};
use crate::ast::{
    self, CompoundStatement, ControlStatement, DefaultOption, LocalDeclaration, QueryExpression, RowElement, SqlStatement, ValueExpression,
    ValueSpecification, ValuesRow,
};
use crate::catalog::ColumnDefault;
use crate::error::{At, Result, SqlState};
use crate::logical::{Block, Contained, Cursor, Declared, Expr, Initial, Leaf, LogicalPlan, Program, Step, Subquery};
use crate::params::{Params, RoutineScope, ScopedVariable, Transitions};
use crate::value::{DataType, Value};

/// A leaf has no set function of the frame: it is of one row.
const ONE_ROW: &str =
    "[ISO/IEC 9075-4:2006 draft §14.5] a value of an SQL control statement is of no table, so no set function is of its frame";

/// \[DESIGN\] What a program is bound over: its frame's first slots, and
/// the scope of the routine or triggered action it is the body of.
pub(crate) struct Frame {
    /// How many slots come before the variables': the routine's parameters
    /// or the action's transition variables.
    pub(crate) base: usize,
    /// The routine's parameters, of no variables yet.
    pub(crate) scope: Arc<RoutineScope>,
    /// Of a triggered action, its transition variables and tables.
    pub(crate) transitions: Option<Arc<Transitions>>,
    /// Of a function, its result type, which RETURN's value takes.
    pub(crate) returns: Option<DataType>,
    /// Whether it is the body of a function, or of a procedure one invokes,
    /// which runs where no SQL-session may run its statements: so each
    /// CALL is bound with it (`psm/inline.rs`).
    pub(crate) inline: bool,
}

/// A label in scope: its name, if written, and its number.
struct Labelled {
    name: Option<String>,
    number: usize,
}

/// What binding a program knows.
struct ProgramBinder<'a> {
    cx: Context<'a>,
    frame: Frame,
    /// The scope of the statement being bound.
    scope: Arc<RoutineScope>,
    labels: Vec<Labelled>,
    next_label: usize,
    /// The type of each variable's slot, after the frame's base.
    variables: Vec<DataType>,
    cursors: Vec<Cursor>,
    /// The columns of each cursor's query, by its number.
    cursor_columns: Vec<Vec<OutColumn>>,
    /// The local cursors in scope, by name, the innermost last.
    named: Vec<(String, usize)>,
    /// The conditions in scope, the innermost last, each with its SQLSTATE
    /// value, if it is defined for one [§14.3 SR3].
    conditions: Vec<(String, Option<SqlState>)>,
}

/// [ISO/IEC 9075-4:2006 draft Clause 14] The control statement `statement`
/// bound in `cx` over `frame`, as a program.
///
/// # Errors
///
/// SQLCODE -103 if it breaks a Syntax Rule; a value's or query's own,
/// bound.
pub(crate) fn bind_program(cx: Context<'_>, statement: &SqlStatement, frame: Frame) -> Result<Program> {
    let scope = Arc::clone(&frame.scope);
    let mut b = ProgramBinder {
        cx,
        frame,
        scope,
        labels: Vec::new(),
        next_label: 0,
        variables: Vec::new(),
        cursors: Vec::new(),
        cursor_columns: Vec::new(),
        named: Vec::new(),
        conditions: Vec::new(),
    };
    let body = b.statement(statement)?;
    Ok(Program { base: b.frame.base, variables: b.variables, cursors: b.cursors, body, scope: b.frame.scope })
}

impl ProgramBinder<'_> {
    /// The `<SQL procedure statement>` `s`, bound in the scope where it is.
    fn statement(&mut self, s: &SqlStatement) -> Result<Step> {
        if let SqlStatement::Control(c) = s {
            return self.control(c);
        }
        if let SqlStatement::Signal(signal) = s {
            return self.signal(signal);
        }
        // [EXTENSION] of a delta table, run by the session, which runs its
        // statement first (RULES.md D183).
        if let SqlStatement::Select { query, targets } = s
            && !matches!(query.table_expression.from.as_slice(), [ast::TableReference::Delta(_)])
        {
            return self.select_into(query, targets);
        }
        if let SqlStatement::Return(r) = s {
            return self.returned(r);
        }
        if let (true, SqlStatement::Call(call)) = (self.frame.inline, s) {
            return self.inline_call(call);
        }
        if let Some(step) = self.cursor_step(s)? {
            return Ok(step);
        }
        Ok(Step::Statement(Box::new(self.contained(s.clone()))))
    }

    /// `statement`, run by the session in the scope here.
    fn contained(&self, statement: SqlStatement) -> Contained {
        Contained::new(statement, Arc::clone(&self.scope))
    }

    /// The SQL control statement `c`, bound.
    fn control(&mut self, c: &ControlStatement) -> Result<Step> {
        match c {
            ControlStatement::Compound(c) => self.block(c),
            ControlStatement::Assignment(a) => self.assignment(a),
            ControlStatement::If(i) => self.if_statement(i),
            ControlStatement::Case(c) => self.case_statement(c),
            ControlStatement::Iterated(l) => self.looped(l),
            ControlStatement::For(f) => self.for_each(f),
            ControlStatement::Leave(l) => Ok(Step::Leave(self.label_number(&l.name, "§14.9 SR1")?)),
            ControlStatement::Iterate(l) => Ok(Step::Iterate(self.label_number(&l.name, "§14.8 SR1")?)),
        }
    }

    /// The number of the innermost label of name `name` in scope; the
    /// parser has checked there is one, as `rule_of` of ITERATE or LEAVE
    /// says [§14.8 SR1, §14.9 SR1].
    fn label_number(&self, name: &str, rule_of: &str) -> Result<usize> {
        let found = self.labels.iter().rev().find(|l| l.name.as_deref() == Some(name));
        found.map(|l| l.number).ok_or_else(|| rule(format!("[ISO/IEC 9075-4:2006 draft {rule_of}] {name} names no statement this is in")))
    }

    /// A new label of name `name`, if written, in scope from now: its
    /// number.
    fn push_label(&mut self, name: Option<&str>) -> usize {
        let number = self.next_label;
        self.next_label = self.next_label.saturating_add(1);
        self.labels.push(Labelled { name: name.map(str::to_owned), number });
        number
    }

    /// [§14.1] The compound statement `c`: its variables, conditions and
    /// cursors in scope of its handlers and statements, its label too.
    fn block(&mut self, c: &CompoundStatement) -> Result<Step> {
        let label_name = c.label.as_ref().map(|l| l.name.as_str());
        let saved = (Arc::clone(&self.scope), self.labels.len(), self.named.len(), self.conditions.len());
        let result = self.block_within(c, label_name);
        self.scope = saved.0;
        self.labels.truncate(saved.1);
        self.named.truncate(saved.2);
        self.conditions.truncate(saved.3);
        result
    }

    /// `block`'s work, what it brings into scope taken out after.
    fn block_within(&mut self, c: &CompoundStatement, label_name: Option<&str>) -> Result<Step> {
        let mut scope = (*self.scope).clone();
        // [ISO/IEC 9075-14:2006 draft §15.1 SR1, SR2] its XML options in
        // scope of what it holds, its declarations' defaults too.
        scope.xml.extend(c.options.iter().cloned());
        self.scope = Arc::new(scope.clone());
        let mut variables = Vec::new();
        for declaration in &c.declarations {
            match declaration {
                LocalDeclaration::Variable(v) => self.declare(v, label_name, (&mut variables, &mut scope))?,
                LocalDeclaration::Condition(d) => {
                    let state = d.sqlstate.as_ref().and_then(|(s, _)| SqlState::parse(s));
                    self.conditions.push((d.name.name.clone(), state));
                }
            }
        }
        let mut cursors = Vec::new();
        for local in &c.cursors {
            cursors.push(self.declare_cursor(local, &mut scope)?);
        }
        self.scope = Arc::new(scope);
        let label = self.push_label(label_name);
        let handlers = self.handlers(&c.handlers)?;
        let statements = self.statements(&c.statements)?;
        Ok(Step::Block(Box::new(Block { label, atomic: c.atomic, variables, cursors, handlers, statements })))
    }

    /// [§14.4] The variables `v` declares, of the compound statement
    /// labelled `label`: each a new slot, its type and initial value.
    fn declare(
        &mut self,
        v: &ast::VariableDeclaration,
        label: Option<&str>,
        (out, scope): (&mut Vec<Declared>, &mut RoutineScope),
    ) -> Result<()> {
        let (data_type, domain) = super::xml::resolved_type(&self.cx, &v.data_type, self.cx.user.cell)?;
        let at = v.names.first().map_or(0, |n| n.at);
        let initial = match (v.default.as_ref(), domain.and_then(|d| d.default.clone())) {
            (None, Some(d)) => self.domain_initial(&d, &data_type, at)?,
            (default, _) => self.initial(default, &data_type, at)?,
        };
        for name in &v.names {
            let slot = self.frame.base.saturating_add(self.variables.len());
            self.variables.push(data_type.clone());
            scope.variables.push(ScopedVariable {
                label: label.map(str::to_owned),
                name: name.name.clone(),
                data_type: data_type.clone(),
                slot,
            });
            out.push(Declared { slot, data_type: data_type.clone(), initial: initial.clone() });
        }
        Ok(())
    }

    /// [§14.4 GR1] The initial value of a variable of type `t` whose
    /// `<default option>` is `default`, if written at `at`: of the type, by
    /// store assignment [ISO/IEC 9075-2:2023 §11.5 SR3, §9.2].
    fn initial(&self, default: Option<&DefaultOption>, t: &DataType, at: usize) -> Result<Initial> {
        let value = match default {
            None | Some(DefaultOption::Null) => return Ok(Initial::Null),
            Some(DefaultOption::Empty(kind)) => {
                if kind.empty_of(t).is_none() {
                    return Err(rule(format!(
                        "[ISO/IEC 9075-2:2023 §11.5 SR4e] {} is the default only of a collection type of its kind, not {t}",
                        kind.empty_written()
                    )));
                }
                return Ok(Initial::Empty);
            }
            Some(DefaultOption::Literal(l)) => ValueExpression::Value(ValueSpecification::Literal(l.clone())),
            Some(DefaultOption::User(f)) => ValueExpression::Value(ValueSpecification::User(*f)),
            Some(DefaultOption::Datetime(f)) => ValueExpression::Datetime(*f),
        };
        let (leaf, source) = self.value(&value, at)?;
        let Some(leaf) = self.assigned_leaf(leaf, std::slice::from_ref(&source), &[t])? else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §11.5 SR3, §9.2 SR2] a default of {source} is no value of {t}")).or_at(at));
        };
        Ok(Initial::Value(leaf))
    }

    /// [§14.4 GR1] The initial value of a variable of type `t` of a domain
    /// whose default is `d`, of no `<default clause>` of its own.
    fn domain_initial(&self, d: &ColumnDefault, t: &DataType, at: usize) -> Result<Initial> {
        let value = match d {
            ColumnDefault::Value(Value::Null) => return Ok(Initial::Null),
            ColumnDefault::Value(v) => {
                let plan = LogicalPlan::Values { rows: vec![vec![Expr::Const(v.clone())]], types: vec![t.clone()] };
                return Ok(Initial::Value(Leaf { query: Arc::new(Subquery::new(plan, Vec::new())), at }));
            }
            ColumnDefault::User(f) => ValueExpression::Value(ValueSpecification::User(*f)),
            ColumnDefault::Datetime(moment, p) => ValueExpression::Datetime(ast::DatetimeFunction { moment: *moment, precision: Some(*p) }),
        };
        Ok(Initial::Value(self.value(&value, at)?.0))
    }

    /// [§12.1] The local cursor `local`, its name in scope `scope`: its
    /// number.
    fn declare_cursor(&mut self, local: &ast::LocalCursor, scope: &mut RoutineScope) -> Result<usize> {
        let query = QueryExpression::clone(&local.specification.query);
        let (leaf, columns) = self.leaf(query, local.name.at)?;
        let number = self.cursors.len();
        let key = cursor_key(&local.name.name, number);
        scope.cursors.push((local.name.name.clone(), key.clone()));
        let cursor = Cursor {
            name: local.name.name.clone(),
            key,
            query: leaf,
            text: local.text.clone(),
            insensitive: local.insensitive,
            scroll: local.scroll,
            scope: Arc::new(scope.clone()),
        };
        self.cursors.push(cursor);
        self.cursor_columns.push(columns);
        self.named.push((local.name.name.clone(), number));
        Ok(number)
    }

    /// The number of the local cursor `name` in scope, if one is.
    fn local_cursor(&self, name: &str) -> Option<usize> {
        self.named.iter().rev().find(|(n, _)| n == name).map(|(_, k)| *k)
    }

    /// The parameters a leaf is bound with: the frame's slots, those of the
    /// variables declared so far among them, as slots to be filled.
    fn params(&self) -> Params {
        let width = self.frame.base.saturating_add(self.variables.len());
        let params = Params::of_routine(vec![Value::Null; width], Arc::clone(&self.scope));
        let params = match &self.frame.transitions {
            Some(t) => params.with_transitions(Arc::clone(t)),
            None => params,
        };
        params.keeping()
    }

    /// The query `query`, written at `at`, bound as a leaf, with its columns.
    fn leaf(&self, query: QueryExpression, at: usize) -> Result<(Leaf, Vec<OutColumn>)> {
        self.leaf_in(query, at, false)
    }

    /// `leaf`, a table value constructor that `hosts` a function that
    /// possibly modifies SQL-data if so: a value of the program, which no
    /// rule keeps from invoking one [ISO/IEC 9075-2:2023 §4.35.2], but a
    /// simple CASE statement's operand [§14.6 SR1a].
    fn leaf_in(&self, query: QueryExpression, at: usize, hosts: bool) -> Result<(Leaf, Vec<OutColumn>)> {
        let params = self.params();
        let cx = Context { params: &params, ..self.cx };
        let binder = Binder::new(cx, None, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
        let s = ast::Subquery { query, at };
        let (bound, columns) = binder.subquery_hosting(&s, Scope::Rows(ONE_ROW), hosts).or_at(at)?;
        Ok((Leaf { query: bound, at }, columns))
    }

    /// [ISO/IEC 9075-4:2006 draft §14.5 SR5] Of the structured type `of`,
    /// the position and type of its attribute `name`, whose mutator an
    /// assignment invokes.
    fn mutator_step(&self, of: &crate::value::types::UdtName, name: &str) -> Result<(usize, DataType)> {
        let params = self.params();
        let cx = Context { params: &params, ..self.cx };
        let binder = Binder::new(cx, None, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
        binder.mutator_of(of, name)
    }

    /// [ISO/IEC 9075-2:2023 §9.2 GR2] `leaf`, of a row of the types
    /// `types`, each value assigned to a site of the type in its place of
    /// `targets`, by an appropriate user-defined cast where it is not
    /// assignable (`Binder::assigned_query`); none if one is neither.
    fn assigned_leaf(&self, leaf: Leaf, types: &[DataType], targets: &[&DataType]) -> Result<Option<Leaf>> {
        if types.len() != targets.len() || types.iter().zip(targets).all(|(t, target)| super::assignable(target, t)) {
            return Ok(Some(leaf));
        }
        let params = self.params();
        let cx = Context { params: &params, ..self.cx };
        let binder = Binder::new(cx, None, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
        Ok(binder.assigned_query(&leaf.query, types, targets)?.map(|query| Leaf { query, at: leaf.at }))
    }

    /// [ISO/IEC 9075-2:2023 §9.2 GR2] The values `row`, of the types
    /// `types`, each assigned to a site of the type in its place of
    /// `targets` (`Binder::assigned`); none if one is not assignable.
    fn cast_row(&self, row: Vec<Expr>, types: &[DataType], targets: &[DataType]) -> Result<Option<Vec<Expr>>> {
        let params = self.params();
        let cx = Context { params: &params, ..self.cx };
        let binder = Binder::new(cx, None, (Vec::new(), Vec::new(), Vec::new()), BTreeSet::new());
        let mut out = Vec::with_capacity(row.len());
        for ((e, t), target) in row.into_iter().zip(types).zip(targets) {
            let Some(e) = binder.assigned((e, t), target)? else { return Ok(None) };
            out.push(e);
        }
        Ok(Some(out))
    }

    /// The value `v`, written at `at`, bound as a leaf, and its type.
    fn value(&self, v: &ValueExpression, at: usize) -> Result<(Leaf, DataType)> {
        let query = QueryExpression::Values(vec![ValuesRow { elements: vec![RowElement::Value(v.clone(), at)], at }]);
        let (leaf, columns) = self.leaf_in(query, at, true)?;
        let [column] = columns.as_slice() else {
            return Err(rule("[ISO/IEC 9075-4:2006 draft §14.5] a value is of degree one").or_at(at));
        };
        Ok((leaf, column.data_type.clone()))
    }

    /// The search condition `c`, written at `at`, bound as a leaf of its
    /// truth value.
    fn condition(&self, c: &ast::SearchCondition, at: usize) -> Result<Leaf> {
        Ok(self.value(&ValueExpression::Boolean(Box::new(c.clone())), at)?.0)
    }
}

/// The program's key of the local cursor `name`, numbered `number`: no
/// `<cursor name>`, as none holds a NUL, though an error shows it as
/// `name`. Each execution of the program names it apart
/// (`params::local_key`).
pub(super) fn cursor_key(name: &str, number: usize) -> String {
    format!("{name}\u{0}{number}")
}
