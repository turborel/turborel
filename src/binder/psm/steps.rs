//! [ISO/IEC 9075-4:2006 draft §14.5 to §14.13, §17.2, §17.3] The
//! assignment, CASE, IF, iterated and FOR statements, SIGNAL and RESIGNAL,
//! and the statements of a program that read a query: SELECT INTO, RETURN
//! and the statements of a local cursor.

use std::sync::Arc;

use super::{ProgramBinder, cursor_key};
use crate::ast::{
    self, AssignmentSource, AssignmentTarget, CaseExpression, CaseWhen, FetchOrientation, Iteration, Literal, QueryExpression,
    QuerySpecification, ReturnStatement, SignalStatement, SignalValue, SqlStatement, StatementList, ValueExpression, ValueSpecification,
    ValuesRow,
};
use crate::binder::{OutColumn, assignable, rule};
use crate::db::{assignment_type, check_targets};
use crate::error::{At, Result, SqlState};
use crate::logical::{
    Assign, Branches, Cursor, CursorAction, CursorStep, Expr, ForEach, Into, Leaf, LogicalPlan, Looped, Modified, Repeating, Signal, Step,
    Subquery, Target,
};
use crate::params::ScopedVariable;
use crate::value::DataType;

impl ProgramBinder<'_> {
    /// The statements `list`, each bound, where each begins noted
    /// (RULES.md D1003).
    pub(super) fn statements(&mut self, list: &StatementList) -> Result<Vec<Step>> {
        list.placed()
            .map(|(at, s)| {
                self.places.push(at);
                self.statement(s)
            })
            .collect()
    }

    /// [§14.5] The assignment statement `a`: its source a row of a value
    /// for each target [SR1], each assignable to its target [SR6, SR8].
    pub(super) fn assignment(&self, a: &ast::Assignment) -> Result<Step> {
        let targets = a.targets.iter().map(|t| self.target(t)).collect::<Result<Vec<Target>>>()?;
        let (source, types) = match &a.source {
            AssignmentSource::Null(_) => (None, Vec::new()),
            AssignmentSource::Value(v) => {
                let (leaf, t) = self.value(v, a.at)?;
                (Some(leaf), vec![t])
            }
            AssignmentSource::Row(row, at) => {
                let (leaf, types) = self.assigned_row(row, targets.len(), *at)?;
                (Some(leaf), types)
            }
        };
        if source.is_some() && types.len() != targets.len() {
            let msg =
                format!("[ISO/IEC 9075-4:2006 draft §14.5 SR1b] {} targets are assigned a row of degree {}", targets.len(), types.len());
            return Err(rule(msg).or_at(a.at));
        }
        let sites: Vec<&DataType> = targets.iter().map(|t| &t.data_type).collect();
        let source = if let Some(leaf) = source {
            let Some(cast) = self.assigned_leaf(leaf, &types, &sites)? else {
                // The first target whose value has neither its type nor a cast.
                let first = targets.iter().zip(&types).find(|(t, given)| !assignable(&t.data_type, given));
                let (what, at) = first.map_or((String::new(), a.at), |(t, given)| {
                    (format!(" of {given} cannot be stored in {} ({})", t.spec.name, t.data_type), t.spec.at)
                });
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §9.2 SR2] a value{what}")).or_at(at));
            };
            Some(cast)
        } else {
            None
        };
        Ok(Step::Assign(Box::new(Assign { targets, source, scope: Arc::clone(&self.scope), at: a.at })))
    }

    /// [§14.5 SR1] The `<assigned row>` `row` of `degree` targets: one row
    /// subquery's query, or the row of its values; and their types.
    fn assigned_row(&self, row: &[ValueExpression], degree: usize, at: usize) -> Result<(Leaf, Vec<DataType>)> {
        if let ([ValueExpression::Subquery(s)], true) = (row, degree > 1) {
            let (leaf, columns) = self.leaf(s.query.clone(), at)?;
            return Ok((leaf, columns.into_iter().map(|c| c.data_type).collect()));
        }
        let elements = row.iter().map(|v| ast::RowElement::Value(v.clone(), at)).collect();
        let (leaf, columns) = self.leaf_in(QueryExpression::Values(vec![ValuesRow { elements, at }]), at, true)?;
        Ok((leaf, columns.into_iter().map(|c| c.data_type).collect()))
    }

    /// [§14.5] The `<assignment target>` `t`, and the declared type of what
    /// it assigns: of a `<modified field reference>`, its last field's
    /// [SR6] (Feature T051).
    fn target(&self, t: &AssignmentTarget) -> Result<Target> {
        let params = self.params();
        let mut data_type = assignment_type(&params, &t.target).or_at(t.target.at)?;
        let mut fields = Vec::new();
        for field in &t.fields {
            // [SR4, SR5] of a structured type, its attribute, which its
            // mutator sets.
            if let DataType::Structured(s) = &data_type {
                let (index, attribute) = self.mutator_step(&s.name, &field.name).or_at(field.at)?;
                fields.push(Modified::Attribute(index));
                data_type = attribute;
                continue;
            }
            let DataType::Row(row) = data_type.represented() else {
                let msg = format!(
                    "[ISO/IEC 9075-2:2023 §6.25 SR1] {} is of {data_type}, no row type, so it has no field {}",
                    t.target.name, field.name
                );
                return Err(rule(msg).or_at(field.at));
            };
            let Some((k, f)) = row.fields.iter().enumerate().find(|(_, f)| f.name == field.name) else {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §6.25 SR2] {data_type} has no field {}", field.name)).or_at(field.at));
            };
            fields.push(Modified::Field(k));
            data_type = f.data_type.clone();
        }
        if !t.fields.is_empty() && t.target.element.is_some() {
            return Err(rule("[ISO/IEC 9075-4:2006 draft §14.5] a field of an array element is not assigned").or_at(t.target.at));
        }
        Ok(Target { spec: t.target.clone(), fields, data_type })
    }

    /// [§14.7] The IF statement `i`.
    pub(super) fn if_statement(&mut self, i: &ast::IfStatement) -> Result<Step> {
        let mut branches = Vec::new();
        for (condition, list) in &i.branches {
            branches.push((self.condition(condition, i.at)?, self.statements(list)?));
        }
        let otherwise = i.otherwise.as_ref().map(|list| self.statements(list)).transpose()?;
        Ok(Step::If(Box::new(Branches { arms: branches, otherwise, at: i.at })))
    }

    /// [§14.6] The CASE statement `c`: of a simple one, each WHEN the
    /// condition that the operand is a value of its `<when operand list>`,
    /// as a CASE expression of TRUE and FALSE reads it (Features P004,
    /// P008).
    pub(super) fn case_statement(&mut self, c: &ast::CaseStatement) -> Result<Step> {
        let mut branches = Vec::new();
        for (when, list) in &c.whens {
            let leaf = match (when, &c.operand) {
                (CaseWhen::Condition(condition), _) => self.condition(condition, c.at)?,
                (CaseWhen::Operands(operands), Some(operand)) => {
                    let truth = |b| Some(ValueExpression::Value(ValueSpecification::Literal(Literal::Boolean(Some(b)))));
                    let simple = CaseExpression::Simple {
                        operand: operand.clone(),
                        whens: vec![(operands.clone(), truth(true))],
                        otherwise: truth(false),
                    };
                    self.value(&ValueExpression::Case(Box::new(simple)), c.at)?.0
                }
                (CaseWhen::Operands(_), None) => {
                    return Err(rule("[ISO/IEC 9075-4:2006 draft §14.6] a WHEN operand has no case operand").or_at(c.at));
                }
            };
            branches.push((leaf, self.statements(list)?));
        }
        let otherwise = c.otherwise.as_ref().map(|list| self.statements(list)).transpose()?;
        Ok(Step::Case(Box::new(Branches { arms: branches, otherwise, at: c.at })))
    }

    /// [§14.10 to §14.12] The LOOP, WHILE or REPEAT statement `l`, its label
    /// in scope of its statements.
    pub(super) fn looped(&mut self, l: &ast::Iterated) -> Result<Step> {
        let kind = match &l.iteration {
            Iteration::Loop => Repeating::Always,
            Iteration::While(c) => Repeating::While(self.condition(c, l.at)?),
            Iteration::Repeat(c) => Repeating::Until(self.condition(c, l.at)?),
        };
        let depth = self.labels.len();
        let label = self.push_label(l.label.as_ref().map(|x| x.name.as_str()));
        let statements = self.statements(&l.statements);
        self.labels.truncate(depth);
        Ok(Step::Loop(Box::new(Looped { label, kind, statements: statements? })))
    }

    /// [§14.13] The FOR statement `f`, read as SR7 has it: its cursor, a
    /// variable of each column, named by the column [SR5] and qualified by
    /// its `<for loop variable name>`, and a loop of its label.
    pub(super) fn for_each(&mut self, f: &ast::ForStatement) -> Result<Step> {
        let (leaf, columns) = self.leaf(QueryExpression::clone(&f.specification.query), f.at)?;
        let saved = (Arc::clone(&self.scope), self.labels.len());
        let result = self.for_within(f, leaf, columns);
        self.scope = saved.0;
        self.labels.truncate(saved.1);
        result
    }

    /// `for_each`'s work, its cursor `leaf` of `columns`.
    fn for_within(&mut self, f: &ast::ForStatement, leaf: Leaf, columns: Vec<OutColumn>) -> Result<Step> {
        let mut scope = (*self.scope).clone();
        let label = f.variable.as_ref().map(|v| v.name.clone());
        let mut slots = Vec::new();
        for (k, c) in columns.into_iter().enumerate() {
            let Some(name) = c.name.filter(|n| !scope.variables.iter().skip(self.scope.variables.len()).any(|v| v.name == *n)) else {
                let msg = format!(
                    "[ISO/IEC 9075-4:2006 draft §14.13 SR5] column {} of the FOR statement's query has no name of its own",
                    k.saturating_add(1)
                );
                return Err(rule(msg).or_at(f.at));
            };
            let slot = self.frame.base.saturating_add(self.variables.len());
            self.variables.push(c.data_type.clone());
            scope.variables.push(ScopedVariable { label: label.clone(), name, data_type: c.data_type, slot });
            slots.push(slot);
        }
        let number = self.cursors.len();
        let name = f.cursor.as_ref().map_or_else(String::new, |c| c.name.clone());
        let key = cursor_key(&name, number);
        if f.cursor.is_some() {
            scope.cursors.push((name.clone(), key.clone()));
        }
        let (text, insensitive) = (f.text.clone(), f.insensitive);
        let nameless = name.is_empty();
        let cursor = Cursor { name, key, query: leaf, text, insensitive, scroll: false, read_only: false, scope: Arc::clone(&self.scope) };
        self.cursors.push(cursor);
        self.cursor_columns.push(Vec::new());
        self.scope = Arc::new(scope);
        let loop_label = self.push_label(f.label.as_ref().map(|l| l.name.as_str()));
        let statements = self.statements(&f.statements)?;
        // No statement can name a cursor of no `<cursor name>` [ISO/IEC
        // 9075-4:2006 draft §14.13 SR4]: one whose body writes nothing is
        // read only, as no change made while it is open can be seen through
        // it (RULES.md D613).
        if let Some(c) = self.cursors.get_mut(number) {
            c.read_only = nameless && !statements.iter().any(Step::writes);
        }
        Ok(Step::For(Box::new(ForEach { label: loop_label, cursor: number, slots, statements })))
    }

    /// [§17.2, §17.3] SIGNAL or RESIGNAL `s`: its condition's SQLSTATE
    /// value, if it has one, and the value of each item, a character
    /// string [§17.2 SR3].
    pub(super) fn signal(&self, s: &SignalStatement) -> Result<Step> {
        let (state, name) = match &s.value {
            None => (None, None),
            Some(SignalValue::Sqlstate(text, at)) => {
                let state = SqlState::parse(text)
                    .ok_or_else(|| rule(format!("[ISO/IEC 9075-4:2006 draft §8.2 SR4] '{text}' is no SQLSTATE value")).or_at(*at))?;
                (Some(state), None)
            }
            Some(SignalValue::Name(label)) => {
                let found = self.conditions.iter().rev().find(|(n, _)| *n == label.name);
                let Some((_, state)) = found else {
                    return Err(rule(format!("[ISO/IEC 9075-4:2006 draft §17.2 SR1a ii] no condition {} is declared here", label.name))
                        .or_at(label.at));
                };
                (*state, Some(label.name.clone()))
            }
        };
        let mut items = Vec::new();
        for (item, value) in &s.items {
            let (leaf, t) = self.value(value, s.at)?;
            if !t.represented().is_character() {
                let msg =
                    format!("[ISO/IEC 9075-4:2006 draft §17.2 SR3] {} is set to a value of {t}: it takes a character string", item.name());
                return Err(rule(msg).or_at(s.at));
            }
            items.push((*item, leaf));
        }
        Ok(Step::Signal(Box::new(Signal { resignal: s.resignal, state, name, items, at: s.at })))
    }

    /// [ISO/IEC 9075-2:2023 §14.7] SELECT INTO: its query a leaf of its
    /// rows, as many columns as targets, each assignable to its target.
    pub(super) fn select_into(&self, query: &QuerySpecification, targets: &[ast::ParameterSpecification]) -> Result<Step> {
        let at = targets.first().map_or(0, |t| t.at);
        let (leaf, mut columns) = self.leaf(QueryExpression::Spec(Box::new(query.clone())), at)?;
        // [ISO/IEC 9075-2:2023 §9.2 GR2] to an SQL variable or parameter by
        // an appropriate user-defined cast, where its type is not assignable.
        let params = self.params();
        let sites = targets.iter().map(|t| assignment_type(&params, t)).collect::<Result<Vec<DataType>>>();
        let leaf = match sites {
            Ok(sites) if sites.len() == columns.len() => {
                let types: Vec<DataType> = columns.iter().map(|c| c.data_type.clone()).collect();
                match self.assigned_leaf(leaf.clone(), &types, &sites.iter().collect::<Vec<_>>())? {
                    Some(cast) => {
                        columns.iter_mut().zip(sites).for_each(|(c, s)| c.data_type = s);
                        cast
                    }
                    None => leaf,
                }
            }
            Ok(_) | Err(_) => leaf,
        };
        check_targets(&columns, targets, &params, "§8.10 SR3", "§8.10 SR4")?;
        Ok(Step::Into(Box::new(Into { query: leaf, targets: targets.to_vec(), scope: Arc::clone(&self.scope) })))
    }

    /// [ISO/IEC 9075-2:2023 §16.2] RETURN `r`, of a function [SR1]: its
    /// value assignable to the result type [SR3].
    pub(super) fn returned(&self, r: &ReturnStatement) -> Result<Step> {
        let Some(result) = &self.frame.returns else {
            return Err(
                rule("[ISO/IEC 9075-2:2023 §16.2 SR1] RETURN is a statement of the body of an SQL-invoked function alone").or_at(r.at)
            );
        };
        let Some(v) = &r.value else { return Ok(Step::Return(None)) };
        let (leaf, t) = self.value(v, r.at)?;
        let Some(leaf) = self.assigned_leaf(leaf, std::slice::from_ref(&t), &[result])? else {
            return Err(rule(format!("[ISO/IEC 9075-2:2023 §16.2 SR3] a value of {t} is not returned as {result}")).or_at(r.at));
        };
        Ok(Step::Return(Some(leaf)))
    }

    /// [§12.2 to §12.4] OPEN, FETCH or CLOSE of a local cursor, if `s` is
    /// one: FETCH's targets as many as the cursor's columns, each
    /// assignable [ISO/IEC 9075-2:2023 §15.3 SR5], and of a cursor not
    /// SCROLL, NEXT alone.
    pub(super) fn cursor_step(&self, s: &SqlStatement) -> Result<Option<Step>> {
        let (name, action) = if let SqlStatement::Open(name) = s {
            (name, CursorAction::Open)
        } else if let SqlStatement::Close(name) = s {
            (name, CursorAction::Close)
        } else if let SqlStatement::Fetch { cursor, orientation, targets } = s {
            (cursor, CursorAction::Fetch(orientation.clone(), targets.clone(), Arc::clone(&self.scope), None))
        } else {
            return Ok(None);
        };
        let Some(cursor) = self.local_cursor(name) else { return Ok(None) };
        let mut action = action;
        if let (CursorAction::Fetch(orientation, targets, _, cast), Some(declared)) = (&mut action, self.cursors.get(cursor)) {
            if !declared.scroll && !matches!(orientation, FetchOrientation::Next) {
                return Err(rule("[ISO/IEC 9075:1992 §13.3 SR3] the cursor is not SCROLL: it fetches NEXT alone"));
            }
            let mut columns = self.cursor_columns.get(cursor).cloned().unwrap_or_default();
            *cast = self.fetch_cast(&mut columns, targets)?;
            check_targets(&columns, targets, &self.params(), "§8.6 SR2", "§8.6 SR3")?;
        }
        Ok(Some(Step::Cursor(Box::new(CursorStep { cursor, action }))))
    }

    /// [ISO/IEC 9075-2:2023 §9.2 GR2] Of a FETCH of a row of `columns`
    /// into SQL variables or parameters `targets`, a VALUES of each value
    /// assigned, by an appropriate user-defined cast where its type is not
    /// assignable, over the row's values in its outer reference slots; and
    /// the columns of the types they take. None if every one is assignable,
    /// or one is not assignable even so, as `check_targets` then says.
    fn fetch_cast(&self, columns: &mut [OutColumn], targets: &[ast::ParameterSpecification]) -> Result<Option<Leaf>> {
        let params = self.params();
        let Ok(sites) = targets.iter().map(|t| assignment_type(&params, t)).collect::<Result<Vec<DataType>>>() else { return Ok(None) };
        if sites.len() != columns.len() || columns.iter().zip(&sites).all(|(c, s)| assignable(s, &c.data_type)) {
            return Ok(None);
        }
        let outer = columns.iter().enumerate().map(|(i, c)| Expr::Outer(i, c.name.clone().unwrap_or_default()));
        let types: Vec<DataType> = columns.iter().map(|c| c.data_type.clone()).collect();
        let Some(row) = self.cast_row(outer.collect(), &types, &sites)? else { return Ok(None) };
        columns.iter_mut().zip(&sites).for_each(|(c, s)| c.data_type = s.clone());
        let plan = LogicalPlan::Values { rows: vec![row], types: sites };
        Ok(Some(Leaf { query: Arc::new(Subquery::new(plan, Vec::new())), at: targets.first().map_or(0, |t| t.at) }))
    }
}
