//! [ISO/IEC 9075-2:2023 §20.4, §20.5, §20.6, §20.13, §20.28] The SQL
//! dynamic statements a PTF component procedure runs on its PTF data area
//! (IA078): GET and SET DESCRIPTOR of a PTF descriptor area, COPY
//! DESCRIPTOR to one, FETCH from a PTF cursor, and PIPE ROW.
//!
//! \[DESIGN\] A component procedure runs where a query evaluates it, where
//! no SQL-session's own SQL descriptor areas are: a `<conventional
//! descriptor name>` there identifies none (33000).

use super::super::Completion;
use super::super::assignment::{assign_targets, target_type};
use super::super::dynamic::ptf::{columns, copy_item, copy_whole, data, put_row};
use super::super::dynamic::{Descriptor, Operands, get_from, set_into};
use super::area::{Component, DataArea, intermediate_result};
use crate::ast::{DynamicName, DynamicStatement, ExtendedName, FetchOrientation, Scope, UsingClause, ValueSpecification};
use crate::binder::value_specification;
use crate::catalog::Principal;
use crate::error::{At, Code, Result, SqlError, SqlState};
use crate::params::Params;
use crate::value::{DataType, Value};

/// A dynamic SQL error (SQLCODE -921) of SQLSTATE `state`.
fn dynamic(state: SqlState, msg: String) -> SqlError {
    SqlError::new(Code::Dynamic, msg).with_state(state)
}

/// [ISO/IEC 9075-2:2023 §5.4 SR11, §20.4 GR1, §20.5 GR1, §20.6 GR2, GR4,
/// §20.28 GR6] The name `n` identifies no PTF descriptor area of the area
/// (33000), or of a cursor's no PTF cursor (34000).
fn unidentified(n: &str, cursor: bool) -> SqlError {
    let (state, what) =
        if cursor { (SqlState::INVALID_CURSOR_NAME, "PTF cursor") } else { (SqlState::INVALID_DESCRIPTOR_NAME, "PTF descriptor area") };
    dynamic(state, format!("[ISO/IEC 9075-2:2023 §5.4] '{n}' identifies no {what} of the PTF data area"))
}

/// What a statement of a component procedure runs with: its PTF data area,
/// and whose the statement is, which its `<simple value specification>`s
/// are read as.
struct On<'a, 'u> {
    area: &'a mut DataArea,
    user: Principal<'u>,
}

/// [ISO/IEC 9075-2:2023 §20.4 to §20.6, §20.13, §20.28] The statement `d`
/// of a component procedure run on `area`, of the frame `frame`, as
/// `user`.
///
/// # Errors
///
/// SQLSTATE 33000 or 34000 if a name identifies nothing of the area;
/// 0700H if PIPE ROW is not run by a start, fulfill or finish component
/// procedure; those of each statement.
pub(crate) fn run_statement(area: &mut DataArea, d: &DynamicStatement, frame: &mut Params, user: Principal<'_>) -> Result<Completion> {
    let mut on = On { area, user };
    match d {
        DynamicStatement::GetDescriptor { name, info } => {
            let key = on.name(name, frame, false)?;
            let catalog = on.area.catalog.clone();
            let read = on.area.descriptors.get(&key).ok_or_else(|| unidentified(&key, false))?.clone();
            get_from(&read, info, frame, &on.operands(&catalog)).or_at(name.at)
        }
        DynamicStatement::SetDescriptor { name, info } => {
            let key = on.name(name, frame, false)?;
            let catalog = on.area.catalog.clone();
            let mut set = on.area.descriptors.get(&key).ok_or_else(|| unidentified(&key, false))?.clone();
            set_into(&mut set, info, frame, &on.operands(&catalog)).or_at(name.at)?;
            on.area.descriptors.insert(key, set);
            Ok(Completion::Success)
        }
        DynamicStatement::CopyDescriptor { source, target, item } => on.copy(frame, (source, target), item.as_deref()),
        DynamicStatement::PipeRow(name) => on.pipe(name, frame),
        DynamicStatement::Fetch { cursor, orientation, using } => on.fetch(cursor, (orientation, using), frame),
        DynamicStatement::AllocateDescriptor { .. }
        | DynamicStatement::DeallocateDescriptor(_)
        | DynamicStatement::Prepare { .. }
        | DynamicStatement::DeallocatePrepare(_)
        | DynamicStatement::Describe { .. }
        | DynamicStatement::AllocateCursor { .. }
        | DynamicStatement::Open { .. }
        | DynamicStatement::Close(_)
        | DynamicStatement::DeletePositioned { .. }
        | DynamicStatement::UpdatePositioned { .. } => {
            Err(SqlError::new(Code::Api, "[DESIGN] an SQL routine body holds no such SQL dynamic statement (IA078)"))
        }
    }
}

impl On<'_, '_> {
    /// The value of `v` in `frame`, and its type.
    fn value(&self, v: &ValueSpecification, frame: &Params) -> Result<(Value, DataType)> {
        value_specification(v, self.user, frame)
    }

    /// What GET and SET DESCRIPTOR read their operands with.
    fn operands<'c>(&'c self, catalog: &'c str) -> Operands<'c, impl Fn(&ValueSpecification, &Params) -> Result<(Value, DataType)> + 'c> {
        Operands { value: move |v: &ValueSpecification, p: &Params| self.value(v, p), catalog }
    }

    /// [ISO/IEC 9075-2:2023 §5.4 SR11] The PTF extended name `n` names, of
    /// a cursor if `cursor`: its value without leading or trailing spaces.
    /// A `<conventional descriptor name>` names none here.
    fn name(&self, n: &ExtendedName, frame: &Params, cursor: bool) -> Result<String> {
        if n.scope != Scope::Ptf {
            return Err(unidentified("(a conventional name)", cursor).or_at(n.at));
        }
        match self.value(&n.value, frame).or_at(n.at)? {
            (Value::Char(s), _) => Ok(s.trim_matches(' ').to_owned()),
            (Value::Null, _) => Err(unidentified("NULL", cursor).or_at(n.at)),
            (_, _) => {
                Err(SqlError::at(Code::SyntaxRule, n.at, "[ISO/IEC 9075-2:2023 §5.4 SR11] the PTF extended name is a character string"))
            }
        }
    }

    /// The item number `v`, as `rule` reads it.
    fn number(&self, v: &ValueSpecification, frame: &Params, rule: &str) -> Result<i64> {
        let catalog = String::new();
        self.operands(&catalog).whole(v, frame, rule)
    }

    /// [ISO/IEC 9075-2:2023 §20.6] COPY DESCRIPTOR from `source` to
    /// `target`, the whole of it, or of `item` its item.
    fn copy(
        &mut self,
        frame: &Params,
        (source, target): (&ExtendedName, &ExtendedName),
        item: Option<&crate::ast::CopiedItem>,
    ) -> Result<Completion> {
        let from_key = self.name(source, frame, false)?;
        let from = self.area.descriptors.get(&from_key).ok_or_else(|| unidentified(&from_key, false).or_at(source.at))?.clone();
        let to_key = self.name(target, frame, false)?;
        let mut to = self.area.descriptors.get(&to_key).ok_or_else(|| unidentified(&to_key, false).or_at(target.at))?.clone();
        match item {
            None => copy_whole(&from, &mut to)?,
            Some(i) => {
                let n1 = self.number(&i.from, frame, "§20.6 SR1")?;
                let n2 = self.number(&i.to, frame, "§20.6 SR1")?;
                copy_item(&from, n1, (&mut to, n2), i.options)?;
            }
        }
        self.area.descriptors.insert(to_key, to);
        Ok(Completion::Success)
    }

    /// [ISO/IEC 9075-2:2023 §20.28] PIPE ROW of the PTF descriptor area
    /// `name` names: the values of the columns it describes, kept, if it is
    /// the intermediate result row type descriptor area and a start,
    /// fulfill or finish component procedure runs.
    fn pipe(&mut self, name: &ExtendedName, frame: &Params) -> Result<Completion> {
        if !matches!(self.area.running, Some(Component::Start | Component::Fulfill | Component::Finish)) {
            let msg = "[ISO/IEC 9075-2:2023 §20.28 GR1] PIPE ROW is executed by a PTF start, fulfill or finish component procedure alone";
            return Err(dynamic(SqlState::PIPE_ROW_OUTSIDE, msg.to_owned()).or_at(name.at));
        }
        let key = self.name(name, frame, false)?;
        if key != intermediate_result() {
            let msg =
                format!("[ISO/IEC 9075-2:2023 §20.28 GR6] '{key}' does not identify the intermediate result row type descriptor area");
            return Err(dynamic(SqlState::INVALID_DESCRIPTOR_NAME, msg).or_at(name.at));
        }
        let area = self.area.descriptors.get(&key).ok_or_else(|| unidentified(&key, false))?;
        let catalog = &self.area.catalog;
        let types: Vec<DataType> = columns(area, catalog).unwrap_or_default().into_iter().map(|(_, t)| t).collect();
        let row = data(area, &types, catalog);
        self.area.piped.push(row);
        Ok(Completion::Success)
    }

    /// [ISO/IEC 9075-2:2023 §20.13] FETCH from the PTF cursor `cursor` of
    /// `orientation`, NEXT alone, as a PTF cursor is NO SCROLL [§9.25
    /// GR20c i 1 E], its row put as `using` says: assigned to its targets,
    /// or put in the PTF descriptor area it names.
    fn fetch(
        &mut self,
        cursor: &DynamicName,
        (orientation, using): (&FetchOrientation, &UsingClause),
        frame: &mut Params,
    ) -> Result<Completion> {
        let DynamicName::Extended(name) = cursor else {
            return Err(SqlError::new(Code::Api, "[DESIGN] a PTF component procedure fetches from a PTF cursor alone"));
        };
        let key = self.name(name, frame, true)?;
        if !matches!(orientation, FetchOrientation::Next) {
            let msg = "[ISO/IEC 9075-2:2023 §20.13 SR2] a PTF cursor is NO SCROLL, so FETCH NEXT alone";
            return Err(SqlError::at(Code::SyntaxRule, name.at, msg));
        }
        let c = self.area.cursors.get_mut(&key).ok_or_else(|| unidentified(&key, true).or_at(name.at))?;
        let Some(n) = c.next else {
            let msg = format!("[ISO/IEC 9075-2:2023 §15.3 GR1] PTF cursor '{key}' is not open");
            return Err(SqlError::at(Code::Cursor, name.at, msg));
        };
        let Some(row) = c.rows.get(n).cloned() else {
            c.next = Some(c.rows.len().saturating_add(1));
            return Ok(Completion::NoData);
        };
        c.next = Some(n.saturating_add(1));
        let types = c.types.clone();
        match using {
            UsingClause::Arguments(targets) => {
                if targets.len() != row.len() {
                    let msg =
                        format!("[ISO/IEC 9075-2:2023 §20.13 GR4] {} targets for the {} columns of the cursor", targets.len(), row.len());
                    return Err(dynamic(SqlState::USING_CLAUSE_TARGETS_MISMATCH, msg).or_at(name.at));
                }
                for t in targets {
                    target_type(frame, t).or_at(t.at)?;
                }
                assign_targets(&row, targets, frame, "ISO/IEC 9075-2:2023 §20.13 GR4", "ISO/IEC 9075-2:2023 §20.13 GR4")
            }
            UsingClause::Descriptor(d) => {
                let into = self.name(d, frame, false)?;
                let catalog = self.area.catalog.clone();
                let area: &mut Descriptor = self.area.descriptors.get_mut(&into).ok_or_else(|| unidentified(&into, false).or_at(d.at))?;
                if columns(area, &catalog).is_none_or(|described| described.len() != row.len()) {
                    let msg = format!(
                        "[ISO/IEC 9075-2:2023 §20.13 GR3] PTF descriptor area '{into}' does not describe the cursor's {} columns",
                        row.len()
                    );
                    return Err(dynamic(SqlState::INVALID_DESCRIPTOR_COUNT, msg).or_at(d.at));
                }
                put_row(area, &row, &types, &catalog);
                Ok(Completion::Success)
            }
        }
    }
}
