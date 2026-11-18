//! [ISO/IEC 9075-2:2023 §7.11] A `<JSON table>` bound: transformed by its
//! Syntax Rules into a `json::JsonTable`, its `<JSON table primitive>`s,
//! one of each `<JSON table columns clause>`, and its plan tree
//! (`table/plan.rs`), and planned as a collection derived table is
//! (`binder/unnest.rs`), laterally over the table references before it
//! [§7.6 SR5]: its operands, the context item, the PASSING arguments and
//! each DEFAULT, may reference them, and a `?` among them is typed as
//! JSON_VALUE's is.
//!
//! Every clause a column does not write is made explicit as the Syntax
//! Rules say [SR1e, SR1f]. A column's path is parsed once, here, and it
//! names no variable, as its rules apply §9.46 with no PASSING clause
//! [SR1e vi, SR1f xiii]; \[DESIGN\] a nested path may name the PASSING
//! arguments, since its primitive is given them [SR1k iii 6 D], though
//! SR1g v names the column path's rules for it. A JSON table is a
//! potential source of non-determinism [§9.16 SR1u], so no constraint or
//! generation expression holds one.

mod plan;

use super::super::composite::integer;
use super::super::expression::nondeterministic_source;
use super::super::from::{Bounds, From, FromItem, renamed};
use super::super::unnest::{Unnested, lateral_binder, lateral_item};
use super::super::{Binder, Scope, rule};
use super::{CONTEXT_RULE, PASSED, expr_initials, returnable};
use crate::ast::{
    JsonTable, JsonTableColumn, JsonTableColumnKind, JsonTableFormatted, JsonTableNamed, JsonTablePath, JsonValueBehavior, TypeName,
};
use crate::catalog::ColumnDesc;
use crate::error::{At, Code, Result, SqlError};
use crate::functions::{input_type, json_text};
use crate::json::{
    Formatted, Item, JsonTable as Table, Path, Primitive, QueryBehavior, Quotes, Scalar, Slot, TableColumn, TableErrorBehavior, Wrapper,
    parse_path, serialize,
};
use crate::logical::Expr;
use crate::params::ParameterType;
use crate::value::DataType;

/// The rule a set function among a JSON table's operands breaks.
const SET_FUNCTION_RULE: &str = "[ISO/IEC 9075-2:2023 §6.9 SR7] a set function in a <JSON table> is over an outer reference";

/// A `<JSON table columns clause>`, CNODE of SR1i iv, as it is compiled.
struct Clause<'t> {
    /// Its `<JSON table path name>` and where it is written, if one is.
    name: Option<(&'t str, usize)>,
    /// Its `<JSON path specification>`, and where it is written.
    path: (&'t str, usize),
    /// The clause it is nested in.
    parent: Option<usize>,
    /// The clauses nested in it, in order.
    children: Vec<usize>,
    /// Its columns, each with its place among the JSON table's.
    columns: Vec<(usize, TableColumn)>,
}

/// A JSON table's clauses and columns, as they are compiled.
struct Shape<'t> {
    /// The clauses, in the order their COLUMNS are written [SR1i iv].
    clauses: Vec<Clause<'t>>,
    /// The columns, in the order they are written [SR1l].
    columns: Vec<ColumnDesc>,
    /// The type each DEFAULT is cast to, in the order of `operands`.
    defaults: Vec<DataType>,
    /// The `<JSON table error behavior>`, explicit or implicit.
    on_error: TableErrorBehavior,
}

/// [ISO/IEC 9075-2:2023 §7.11] The JSON table `table`, its first column at
/// `offset` of the FROM row: its operands bound over every item of `from`
/// before it [§7.6 SR5], its columns named as they are written, else by
/// its derived column list.
///
/// # Errors
///
/// SQLCODE -103 if it breaks a Syntax Rule of §7.11, §10.14 or §9.46, or
/// is in a constraint [§9.16 SR1u]; -900 of a binary string's JSON text.
pub(in crate::binder) fn json_table_item(bounds: Bounds<'_>, table: &JsonTable, offset: usize, from: &mut From) -> Result<FromItem> {
    let binder = lateral_binder(bounds, from);
    if binder.purpose.constrains() {
        return Err(nondeterministic_source("u", "JSON_TABLE"));
    }
    // [SR1b] EMPTY ON ERROR if none is written.
    let on_error = table.on_error.unwrap_or(TableErrorBehavior::Empty);
    let mut shape = Shape { clauses: Vec::new(), columns: Vec::new(), defaults: Vec::new(), on_error };
    let root = (table.name.as_ref().map(|(n, at)| (n.as_str(), *at)), (table.common.path.as_str(), table.common.path_at));
    shape.clause(&binder, root, None, &table.columns)?;
    shape.distinct()?;
    let (operands, types) = operands(&binder, table, &shape.defaults)?;
    let primitives = shape.primitives(&binder, table, &types)?;
    let (nodes, top) = plan::plan(&shape.clauses, table.plan.as_ref())?;
    let passing = table.common.passing.iter().map(|argument| (argument.name.clone(), argument.input.format.is_some())).collect();
    let json = Table { primitives, nodes, top, passing, on_error, width: shape.columns.len() };
    let mut columns = shape.columns;
    if let Some(list) = &table.derived {
        renamed(&mut columns, list)?;
    }
    let reads = binder.used.into_inner();
    from.used.extend(reads.iter().copied());
    Ok(lateral_item(&table.correlation, table.at, columns, offset, Unnested::json(json, operands, reads)))
}

/// [§7.11 SR1a, SR1e, SR1f] The operands of `table` bound, and their
/// types: the context item, a character string, of which a `?` is
/// CHARACTER VARYING; each PASSING argument, FORMAT JSON a character
/// string, else of a type a `<JSON argument>` has [§10.12]; each DEFAULT,
/// which is cast to its column's type, of `defaults`, and a `?` is of.
fn operands(binder: &Binder<'_>, table: &JsonTable, defaults: &[DataType]) -> Result<(Vec<Expr>, Vec<DataType>)> {
    let scope = Scope::Rows(SET_FUNCTION_RULE);
    let passing = &table.common.passing;
    let mut taken = vec![Some(ParameterType::Varying)];
    taken.extend(passing.iter().map(|argument| argument.input.format.map(|_| ParameterType::Varying)));
    taken.extend(defaults.iter().map(|t| Some(ParameterType::Of(t.clone()))));
    let (mut exprs, mut types) = (Vec::new(), Vec::new());
    for (operand, typing) in table.operands().into_iter().zip(taken) {
        let (e, t) = match typing {
            Some(given) => binder.typed(operand, given, scope)?,
            None => binder.value_expression(operand, scope)?,
        };
        exprs.push(e);
        types.push(t);
    }
    // [§10.14 SR1] the context item, JSON text.
    types.first().map_or(Ok(()), |context| json_text(context, table.common.context.format, CONTEXT_RULE))?;
    for (argument, t) in passing.iter().zip(types.iter().skip(1)) {
        input_type(t, argument.input.format, PASSED).or_at(argument.at)?;
    }
    let first_default = passing.len().saturating_add(1);
    for ((e, t), to) in exprs.iter().zip(&types).skip(first_default).zip(defaults) {
        binder.cast_bound((e.clone(), t.clone()), to.clone())?;
    }
    Ok((exprs, types))
}

impl<'t> Shape<'t> {
    /// Compiles the clause of `columns`, named `name` and of the path
    /// `path`, nested in `parent`, and the clauses nested in it. Its
    /// recursion is bounded as the statement's nesting is, by
    /// `parser::MAX_NESTING_DEPTH` (RULES.md R-CF-6).
    fn clause(
        &mut self,
        binder: &Binder<'_>,
        (name, path): (Option<(&'t str, usize)>, (&'t str, usize)),
        parent: Option<usize>,
        columns: &'t [JsonTableColumn],
    ) -> Result<()> {
        let me = self.clauses.len();
        self.clauses.push(Clause { name, path, parent, children: Vec::new(), columns: Vec::new() });
        if let Some(up) = parent.and_then(|p| self.clauses.get_mut(p)) {
            up.children.push(me);
        }
        for column in columns {
            match column {
                JsonTableColumn::Nested(nested) => {
                    let nested_name = nested.name.as_ref().map(|(n, at)| (n.as_str(), *at));
                    self.clause(binder, (nested_name, (nested.path.text.as_str(), nested.path.at)), Some(me), &nested.columns)?;
                }
                JsonTableColumn::Column(named) => {
                    let made = self.column(binder, named).or_at(named.at)?;
                    if let Some(clause) = self.clauses.get_mut(me) {
                        clause.columns.push(made);
                    }
                }
            }
        }
        Ok(())
    }

    /// [§7.11 SR1e, SR1f, SR2] The column `named`, every clause explicit,
    /// and its place; its descriptor added.
    fn column(&mut self, binder: &Binder<'_>, named: &JsonTableNamed) -> Result<(usize, TableColumn)> {
        let place = self.columns.len();
        let (column, data_type) = match &named.kind {
            // [SR2b i] IV158: INTEGER.
            JsonTableColumnKind::Ordinality => (TableColumn::Ordinality, integer()),
            JsonTableColumnKind::Regular { data_type: data_type @ TypeName::Data(DataType::Json), path, on_empty, on_error } => {
                let f = json_typed(data_type, path.as_ref(), [on_empty.as_ref(), on_error.as_ref()])?;
                let formatted = self.formatted(binder, &f, named)?;
                (TableColumn::Formatted(Box::new(formatted)), DataType::Json)
            }
            JsonTableColumnKind::Regular { data_type, path, on_empty, on_error } => {
                let to = regular_type(data_type)?;
                let path = column_path(binder, path.as_ref(), &named.name, named.at)?;
                // [SR1e iii] NULL ON EMPTY; [SR1e iv] ERROR ON ERROR of
                // ERROR ON ERROR, else NULL ON ERROR.
                let on_empty = self.slot(on_empty.as_ref(), Slot::Null, &to);
                let implicit = if self.on_error == TableErrorBehavior::Error { Slot::Error } else { Slot::Null };
                let on_error = self.slot(on_error.as_ref(), implicit, &to);
                (TableColumn::Regular { path, to: to.clone(), on_empty, on_error }, to)
            }
            JsonTableColumnKind::Formatted(f) => {
                let formatted = self.formatted(binder, f, named)?;
                let to = formatted.returning.clone();
                (TableColumn::Formatted(Box::new(formatted)), to)
            }
        };
        self.columns.push(ColumnDesc {
            name: named.name.clone(),
            data_type,
            not_null: false,
            default: None,
            domain: None,
            collation: None,
            generated: None,
        });
        Ok((place, column))
    }

    /// The slot of the behavior `written`, `implicit` if none is; a
    /// DEFAULT's cast to `to`, its operand the next.
    fn slot(&mut self, written: Option<&JsonValueBehavior>, implicit: Slot, to: &DataType) -> Slot {
        match written {
            None => implicit,
            Some(JsonValueBehavior::Error) => Slot::Error,
            Some(JsonValueBehavior::Null) => Slot::Null,
            Some(JsonValueBehavior::Default(_)) => {
                self.defaults.push(to.clone());
                Slot::Default(self.defaults.len().saturating_sub(1))
            }
        }
    }

    /// [§7.11 SR1f] The formatted column `f` of `named`, every clause
    /// explicit.
    ///
    /// # Errors
    ///
    /// SQLCODE -103 if it has no FORMAT JSON, its type is no character
    /// string's, it has ENCODING, or a quotes behavior WITH a wrapper; -900
    /// of a binary string type.
    fn formatted(&self, binder: &Binder<'_>, f: &JsonTableFormatted, named: &JsonTableNamed) -> Result<Formatted> {
        let t = formatted_type(f)?;
        if let (Some(Wrapper::Conditional | Wrapper::Unconditional), Some((_, at))) = (f.wrapper, f.quotes) {
            return Err(rule("[ISO/IEC 9075-2:2023 §7.11 SR1f vii] a formatted column has no quotes behavior WITH a wrapper")).or_at(at);
        }
        let implicit = if self.on_error == TableErrorBehavior::Error { QueryBehavior::Error } else { QueryBehavior::Null };
        Ok(Formatted {
            path: column_path(binder, f.path.as_ref(), &named.name, named.at)?,
            returning: t,
            // [SR1f v] WITHOUT ARRAY; [SR1f viii] KEEP; [SR1f x] NULL ON
            // EMPTY; [SR1f xi] as the error behavior of the table.
            wrapper: f.wrapper.unwrap_or(Wrapper::Without),
            quotes: f.quotes.map_or(Quotes::Keep, |(q, _)| q),
            on_empty: f.on_empty.unwrap_or(QueryBehavior::Null),
            on_error: f.on_error.unwrap_or(implicit),
        })
    }

    /// [§7.11 SR1h] That the path names written and the column names are
    /// distinct from one another.
    ///
    /// # Errors
    ///
    /// SQLCODE -103, at the second, if two are not.
    fn distinct(&self) -> Result<()> {
        let paths = self.clauses.iter().filter_map(|c| c.name);
        let mut seen: Vec<&str> = Vec::new();
        for (name, at) in paths {
            if seen.contains(&name) {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.11 SR1h] the path name {name} is written twice"))).or_at(at);
            }
            seen.push(name);
        }
        for column in &self.columns {
            if seen.contains(&column.name.as_str()) {
                return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.11 SR1h] the name {} is of two columns or paths", column.name)));
            }
            seen.push(&column.name);
        }
        Ok(())
    }

    /// [§7.11 SR1k iii 6, §10.14 SR5] The primitives of the clauses: each
    /// row pattern parsed, its named variables the PASSING arguments of
    /// `table`, of `types` after the context item's [§9.46 SR4, SR8].
    fn primitives(&mut self, binder: &Binder<'_>, table: &JsonTable, types: &[DataType]) -> Result<Vec<Primitive>> {
        let names: Vec<&str> = table.common.passing.iter().map(|argument| argument.name.as_str()).collect();
        let mut out = Vec::with_capacity(self.clauses.len());
        for clause in &mut self.clauses {
            let (text, at) = clause.path;
            let path = parse_path(text, binder.now).or_at(at)?;
            path.check_variables(&names).or_at(at)?;
            starts_with(&path, table, types).or_at(at)?;
            let columns = std::mem::take(&mut clause.columns);
            out.push(Primitive { name: clause.name.map(|(n, _)| n.to_owned()), text: text.to_owned(), path, columns });
        }
        Ok(out)
    }
}

/// [§7.11 SR1f ii to iv, ix] The type of the formatted column `f`: the
/// JSON type, FORMAT JSON implicit, and KEEP QUOTES; else a character
/// string type, FORMAT JSON written.
///
/// # Errors
///
/// SQLCODE -103 if it is of neither, has no FORMAT JSON, has ENCODING, or
/// is of the JSON type and OMIT QUOTES; -900 of a binary string type.
fn formatted_type(f: &JsonTableFormatted) -> Result<DataType> {
    let TypeName::Data(t) = &f.data_type else {
        return Err(rule("[ISO/IEC 9075-2:2023 §7.11 SR1f iv] a formatted column FORMAT JSON is of a string type or JSON"));
    };
    if f.format.is_some_and(|format| format.encoding.is_some()) {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §10.13 SR2a] a formatted column of {t}, of no binary string type, has no ENCODING"
        )));
    }
    if let DataType::Json = t {
        if let Some((Quotes::Omit, at)) = f.quotes {
            return Err(rule("[ISO/IEC 9075-2:2023 §7.11 SR1f ix] a formatted column of the JSON type KEEPs QUOTES")).or_at(at);
        }
        return Ok(DataType::Json);
    }
    if f.format.is_none() {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.11 SR1f ii 1] a formatted column of {t}, no JSON type, is FORMAT JSON")));
    }
    if t.is_binary() {
        return Err(SqlError::new(
            Code::Unsupported,
            format!(
                "[DESIGN] a formatted column of {t}, a binary string type, FORMAT JSON ENCODING, is valid SQL but is not implemented (RULES.md D18)"
            ),
        ));
    }
    if !t.is_character() {
        return Err(rule(format!("[ISO/IEC 9075-2:2023 §7.11 SR1f iv] a formatted column FORMAT JSON is of a string type, not {t}")));
    }
    Ok(t.clone())
}

/// [§7.11] A column of the JSON type that reads as a regular column, of
/// no formatted column's clause, which is formatted: a regular column is
/// of no JSON type [SR1e ii].
///
/// # Errors
///
/// SQLCODE -103 of a DEFAULT behavior, which a formatted column has none
/// of.
fn json_typed(
    data_type: &TypeName,
    path: Option<&JsonTablePath>,
    behaviors: [Option<&JsonValueBehavior>; 2],
) -> Result<JsonTableFormatted> {
    let [on_empty, on_error] = behaviors.map(|b| match b {
        None => Ok(None),
        Some(JsonValueBehavior::Error) => Ok(Some(QueryBehavior::Error)),
        Some(JsonValueBehavior::Null) => Ok(Some(QueryBehavior::Null)),
        Some(JsonValueBehavior::Default(_)) => {
            Err(rule("[ISO/IEC 9075-2:2023 §7.11 SR1e ii] a column of DEFAULT ON EMPTY or ON ERROR is regular, of no JSON type"))
        }
    });
    Ok(JsonTableFormatted {
        data_type: data_type.clone(),
        format: None,
        path: path.cloned(),
        wrapper: None,
        quotes: None,
        on_empty: on_empty?,
        on_error: on_error?,
    })
}

/// [§7.11 SR1e ii] The type of a regular column, `written`: a predefined
/// character string, numeric, Boolean or datetime type.
fn regular_type(written: &TypeName) -> Result<DataType> {
    const CITED: &str =
        "[ISO/IEC 9075-2:2023 §7.11 SR1e ii] a regular column of a JSON table is of a character string, numeric, Boolean or datetime type";
    match written {
        TypeName::Data(t) => {
            if returnable(t) {
                Ok(t.clone())
            } else {
                Err(rule(format!("{CITED}, not {t}")))
            }
        }
        TypeName::Domain(_) | TypeName::Reference(..) | TypeName::Collection(..) | TypeName::Row(_) | TypeName::Extension { .. } => {
            Err(rule(CITED))
        }
    }
}

/// The `<JSON path mode>` of a column's implicit path.
const IMPLICIT_MODE: &str = "lax";

/// [§7.11 SR1e v, vi, SR1f xii, xiii] The path of a column named `name`,
/// written at `at`: `written`, else `$.` and the JSON string of its name;
/// parsed, and naming no variable. \[DESIGN\] SR1e v 2 and SR1f xii 2
/// write no `<JSON path mode>`, which §9.46 asks of every path; turborel
/// reads the implicit path in lax mode, `lax $."NAME"`.
fn column_path(binder: &Binder<'_>, written: Option<&JsonTablePath>, name: &str, at: usize) -> Result<Path> {
    let (text, at) = match written {
        Some(p) => (p.text.clone(), p.at),
        None => (format!("{IMPLICIT_MODE} $.{}", serialize(&Item::Scalar(Scalar::from(name.to_owned())))?), at),
    };
    let path = parse_path(&text, binder.now).or_at(at)?;
    path.check_variables(&[]).or_at(at)?;
    Ok(path)
}

/// [§9.46 SR8] That each named variable of `path` that is a `<JSON starts
/// with initial>` is an argument of `table` that is a character string,
/// of `types` after the context item's, without FORMAT JSON.
fn starts_with(path: &Path, table: &JsonTable, types: &[DataType]) -> Result<()> {
    let mut initials = Vec::new();
    expr_initials(&path.expr, &mut initials);
    for name in initials {
        let argument = table.common.passing.iter().zip(types.iter().skip(1)).find(|(argument, _)| argument.name == name);
        let Some((argument, t)) = argument else { continue };
        if argument.input.format.is_some() || !t.is_character() {
            return Err(rule(format!(
                "[ISO/IEC 9075-2:2023 §9.46 SR8] the variable ${name} after starts with is a character string argument without FORMAT JSON"
            )))
            .or_at(argument.at);
        }
    }
    Ok(())
}
