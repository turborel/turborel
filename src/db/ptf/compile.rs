//! [ISO/IEC 9075-2:2023 §9.24; §9.18 SR12i to SR12n] The compilation of
//! an invocation of a polymorphic table function, as it is bound: its PTF
//! data area made, its describe component procedure run on it, or its
//! requested row types its tables' full ones, and what that gives checked:
//! the columns each table argument's cursor reads, and the proper result
//! columns.

use std::sync::Arc;

use super::super::dynamic::ptf::{
    MAX_RESULT_ITEMS, columns, copy_whole, empty, of_descriptor_columns, of_names, of_ordering, of_row, top_names,
};
use super::super::routines::run_component;
use super::area::{
    Component, DataArea, Layout, cursor_row, descriptor, full, initial_result, intermediate_result, ordering, partitioning, requested,
};
use crate::catalog::{GenericParameter, SqlRoutineDesc, TableSemantics};
use crate::error::{Code, Result, SqlError, SqlState};
use crate::exec::Subqueries;
use crate::logical::{Program, SortKey};
use crate::value::{BIGINT_PRECISION, DataType, Spelled, Value, assign};

/// A PTF component procedure, bound: its body's program and its
/// parameters' declared types, which its arguments are assigned to.
#[derive(Debug, Clone)]
pub(crate) struct Procedure {
    /// Its body, bound as a program.
    pub(crate) program: Arc<Program>,
    /// Its parameters' declared types.
    pub(crate) types: Vec<DataType>,
}

/// [ISO/IEC 9075-2:2023 §9.18 SR12b] A `<table argument>`, bound: its row
/// type, its partitioning and ordering columns, and its rows' extra
/// columns, after its own, that partition and order them.
#[derive(Debug, Clone)]
pub(crate) struct TableInput {
    /// The columns of its row type, each its name and type.
    pub(crate) columns: Vec<(String, DataType)>,
    /// Its partitioning columns, each by its place among `columns`, in
    /// the order written.
    pub(crate) partition: Vec<usize>,
    /// What each partitioning column is compared by, a column of its rows:
    /// its own, or the sort key of its collation.
    pub(crate) keys: Vec<usize>,
    /// Its ordering columns, each by its place among `columns`, whether
    /// DESC and whether NULLS FIRST.
    pub(crate) order: Vec<(usize, bool, bool)>,
    /// The sort keys its rows are ordered by, over its rows' columns.
    pub(crate) sort: Vec<SortKey>,
    /// Whether its parameter is PASS THROUGH.
    pub(crate) pass_through: bool,
    /// Its parameter's semantics.
    pub(crate) semantics: TableSemantics,
    /// Whether it is pruned [SR12b v].
    pub(crate) pruned: bool,
    /// The `<co-partition specification>` it is referenced in, if one is,
    /// by its place in the `<co-partition clause>`.
    pub(crate) copartitioned: Option<usize>,
}

/// [ISO/IEC 9075-2:2023 §9.24] An invocation, compiled.
#[derive(Debug, Clone)]
pub(crate) struct Compiled {
    /// Its PTF data area, as compilation leaves it.
    pub(crate) area: DataArea,
    /// What its component procedures are passed.
    pub(crate) layout: Layout,
    /// Of each table argument, the columns its cursor reads, each by its
    /// place among the argument's columns [§9.25 GR11a].
    pub(crate) requested: Vec<Vec<usize>>,
    /// Its proper result columns [§9.18 SR12n].
    pub(crate) proper: Vec<(String, DataType)>,
    /// Of each table argument, the name of its pass-through input
    /// surrogate column, of one of PASS THROUGH [§9.25 GR10] (UV133).
    pub(crate) surrogates: Vec<Option<String>>,
}

/// [§9.25 GR11c i] (UV133) The declared type of a pass-through input
/// surrogate column: BIGINT, the ordinal of its row among the table's.
pub(crate) fn surrogate_type() -> DataType {
    DataType::Exact { precision: BIGINT_PRECISION, scale: 0, spelled: Spelled::Bigint }
}

/// An exception of a Syntax Rule of [ISO/IEC 9075-2:2023 §9.18] `rule`.
fn broken(rule: &str, msg: &str) -> SqlError {
    SqlError::new(Code::SyntaxRule, format!("[ISO/IEC 9075-2:2023 §9.18 {rule}] {msg}"))
}

/// [ISO/IEC 9075-2:2023 §9.27] `component`, the procedure `procedure`, run
/// on `area`, passed what `layout` says of the function's arguments
/// `arguments`, where `subqueries` runs a query's subqueries: its INOUT
/// parameters' values after made the area's private variables' and status
/// variable's.
///
/// # Errors
///
/// An argument's store assignment's; an exception of the procedure no
/// handler of it handles.
pub(crate) fn invoke(
    (procedure, component): (&Procedure, Component),
    layout: &Layout,
    arguments: &[Value],
    (area, subqueries): (&mut DataArea, &dyn Subqueries),
) -> Result<()> {
    let passed = layout.arguments(component, area, arguments);
    let status = passed.len().saturating_sub(1);
    let mut values = Vec::with_capacity(passed.len());
    for (v, t) in passed.into_iter().zip(&procedure.types) {
        values.push(assign(v, t, "ISO/IEC 9075-2:2023 §9.18 GR4c i")?);
    }
    area.running = Some(component);
    let ran = run_component(&procedure.program, values, subqueries, area);
    area.running = None;
    let out = ran?;
    let private = area.private.len();
    area.private = out.get(..private).map(<[Value]>::to_vec).unwrap_or_default();
    area.status = out.get(status).cloned().unwrap_or(Value::Null);
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §9.25 GR20d iii, GR20i, GR20j iii] The exception
/// the status variable of `area` raises, if it is other than successful
/// completion: of its value, a valid SQLSTATE, else 45000.
pub(crate) fn raised(area: &DataArea) -> Option<SqlError> {
    let status = area.failed()?;
    let state = SqlState::parse(&status).unwrap_or(SqlState::UNHANDLED_USER_EXCEPTION);
    let msg =
        format!("[ISO/IEC 9075-2:2023 §9.25 GR20] the status variable of the polymorphic table function's PTF data area is '{status}'");
    Some(SqlError::new(Code::Signalled, msg).with_state(state))
}

/// [ISO/IEC 9075-2:2023 §9.18 SR12b ii] The columns a `<descriptor
/// argument>` names: each its name, and its data type if it is written
/// with one.
pub(crate) type DescriptorColumns = Vec<(String, Option<DataType>)>;

/// [§9.24 GR4a] The describe component procedure `procedure`, run where
/// `subqueries` runs a query's subqueries, of `layout`'s compilation
/// arguments `arguments`, on `area`.
///
/// # Errors
///
/// SQLCODE -103 if it raises an exception [SR12i] or leaves the status
/// variable other than successful completion [SR12j]; what it runs that
/// turborel does not implement, SQLCODE -900.
fn described(
    (procedure, subqueries): (&Procedure, &dyn Subqueries),
    layout: &Layout,
    arguments: &[Value],
    area: &mut DataArea,
) -> Result<()> {
    invoke((procedure, Component::Describe), layout, arguments, (area, subqueries)).map_err(|e| {
        // What turborel does not implement is refused as such,
        // SQLCODE -900, not as a rule the invocation breaks.
        if e.code == Code::Unsupported {
            return e;
        }
        broken("SR12i", &format!("the PTF describe component procedure raised an exception, SQLSTATE {}: {}", e.sqlstate().as_str(), e.msg))
    })?;
    if let Some(status) = area.failed() {
        return Err(broken("SR12j", &format!("the PTF describe component procedure left the status variable '{status}'")));
    }
    Ok(())
}

/// [ISO/IEC 9075-2:2023 §9.24, §9.18 SR12i to SR12n] The invocation of
/// `desc` compiled: of the table arguments `tables`, the descriptor
/// arguments `descriptors` (by parameter, none of a null one), and the
/// compilation argument list `arguments`; its private variables' initial
/// values `private`; its describe component procedure run where
/// `subqueries` runs a query's subqueries, if it has one; of the catalog
/// `catalog`.
///
/// # Errors
///
/// SQLCODE -103 if the describe component procedure raises an exception
/// [SR12i] or leaves the status variable other than successful completion
/// [SR12j], or the descriptor areas it leaves break SR12k or SR12l.
pub(crate) fn compile(
    desc: &SqlRoutineDesc,
    (tables, descriptors, arguments): (&[TableInput], &[Option<DescriptorColumns>], &[Value]),
    (private, catalog): (Vec<Value>, String),
    describe: Option<(&Procedure, &dyn Subqueries)>,
) -> Result<Compiled> {
    let layout = Layout::of(desc);
    let mut area = DataArea::new(private, catalog);
    let column_list = desc.ptf.as_ref().and_then(|p| p.columns.clone());
    for ((t, _), input) in layout.tables().zip(tables) {
        made(&mut area, t, input);
    }
    for (p, generic) in layout.parameters.iter().enumerate() {
        if let (Some(GenericParameter::Descriptor), Some(Some(columns))) = (generic, descriptors.get(p)) {
            let d = of_descriptor_columns(columns, &area.catalog);
            area.descriptors.insert(descriptor(p), d);
        }
    }
    let irr = match &column_list {
        Some(list) => of_row(list, 0, &area.catalog),
        None => empty(usize::try_from(MAX_RESULT_ITEMS).unwrap_or(usize::MAX)),
    };
    area.descriptors.insert(initial_result().to_owned(), irr);
    area.descriptors.insert(intermediate_result().to_owned(), empty(usize::try_from(MAX_RESULT_ITEMS).unwrap_or(usize::MAX)));
    match describe {
        Some(run) => described(run, &layout, arguments, &mut area)?,
        // [§9.24 GR4b] each requested row type the full row type.
        None => {
            for (t, _) in layout.tables() {
                let whole = area.descriptors.get(&full(t)).cloned();
                if let (Some(whole), Some(asked)) = (whole, area.descriptors.get_mut(&requested(t))) {
                    copy_whole(&whole, asked)?;
                }
            }
        }
    }
    let requested = requested_columns(&area, &layout, tables)?;
    let proper = proper_columns(&area, tables.iter().any(|t| t.pass_through))?;
    let surrogates: Vec<Option<String>> =
        tables.iter().zip(1..).map(|(input, t)| input.pass_through.then(|| surrogate_name(t, &input.columns))).collect();
    // [§9.25 GR12] The intermediate result row type: the initial one's
    // columns, then a pass-through output surrogate column of each table of
    // PASS THROUGH.
    let mut row = proper.clone();
    row.extend(surrogates.iter().flatten().map(|n| (n.clone(), surrogate_type())));
    let mrr = of_row(&row, 0, &area.catalog);
    area.descriptors.insert(intermediate_result().to_owned(), mrr);
    Ok(Compiled { area, layout, requested, proper, surrogates })
}

/// [ISO/IEC 9075-2:2023 §9.24 GR2b] The PTF descriptor areas of generic
/// table `t`, of the table argument `input`, made in `area`: its full row
/// type's, of a table of set semantics its partitioning's and ordering's,
/// and its requested and cursor row types', empty.
fn made(area: &mut DataArea, t: usize, input: &TableInput) {
    let width = input.columns.len();
    area.descriptors.insert(full(t), of_row(&input.columns, 0, &area.catalog));
    if let TableSemantics::Set { .. } = input.semantics {
        let names: Vec<String> = input.partition.iter().filter_map(|&c| input.columns.get(c)).map(|(n, _)| n.clone()).collect();
        area.descriptors.insert(partitioning(t), of_names(&names));
        let order: Vec<(String, bool, bool)> =
            input.order.iter().filter_map(|&(c, d, n)| input.columns.get(c).map(|(name, _)| (name.clone(), d, n))).collect();
        area.descriptors.insert(ordering(t), of_ordering(&order));
    }
    area.descriptors.insert(requested(t), empty(width));
    area.descriptors.insert(cursor_row(t), empty(width.saturating_add(usize::from(input.pass_through))));
}

/// [ISO/IEC 9075-2:2023 §9.18 SR12k] Of each table argument of `tables`,
/// the columns its requested row type descriptor area in `area` names:
/// at least one, each of LEVEL 0, its NAME not empty, no two alike, each
/// the NAME of one item alone of LEVEL 0 of its full row type's.
fn requested_columns(area: &DataArea, layout: &Layout, tables: &[TableInput]) -> Result<Vec<Vec<usize>>> {
    let mut out = Vec::with_capacity(tables.len());
    for ((t, _), input) in layout.tables().zip(tables) {
        let names = area.descriptors.get(&requested(t)).and_then(top_names).unwrap_or_default();
        if names.is_empty() {
            return Err(broken(
                "SR12k ii",
                &format!("the requested row type of generic table {t} names no column, or an item of it is not of LEVEL 0"),
            ));
        }
        let mut columns = Vec::with_capacity(names.len());
        for (k, name) in names.iter().enumerate() {
            let alike: Vec<usize> = input.columns.iter().enumerate().filter(|(_, (n, _))| n == name).map(|(c, _)| c).collect();
            let [c] = alike.as_slice() else {
                return Err(broken(
                    "SR12k iii",
                    &format!("the requested row type of generic table {t} names '{name}', which is not one column of the table"),
                ));
            };
            if name.is_empty() || names.iter().take(k).any(|n| n == name) {
                return Err(broken(
                    "SR12k iii",
                    &format!("the requested row type of generic table {t} names '{name}' twice, or a column of no name"),
                ));
            }
            columns.push(*c);
        }
        out.push(columns);
    }
    Ok(out)
}

/// [ISO/IEC 9075-2:2023 §9.18 SR12l, SR12m] The columns the initial
/// result row type descriptor area of `area` describes: each of a name
/// not empty, no two alike, each of a declared type; at least one unless
/// a table's parameter is PASS THROUGH, `passed`. \[DESIGN\] COUNT is the
/// number of its items, those that describe a row type's fields among
/// them [§9.24 GR2d i 3].
fn proper_columns(area: &DataArea, passed: bool) -> Result<Vec<(String, DataType)>> {
    let irr = area.descriptors.get(initial_result());
    let Some(columns) = irr.and_then(|d| columns(d, &area.catalog)) else {
        return Err(broken("SR12l iii", "an item of the initial result row type descriptor area matches no declared type"));
    };
    for (k, (name, _)) in columns.iter().enumerate() {
        if name.is_empty() || columns.iter().take(k).any(|(n, _)| n == name) {
            return Err(broken("SR12l ii", &format!("the initial result row type names '{name}' twice, or a column of no name")));
        }
    }
    if columns.is_empty() && !passed {
        return Err(broken("SR12l iv", "the initial result row type has no column, and no generic table parameter is PASS THROUGH"));
    }
    Ok(columns)
}

/// [ISO/IEC 9075-2:2023 §9.25 GR10] (UV133) The name of the pass-through
/// input surrogate column of generic table `t`, of the columns `columns`:
/// `PTF_PASS_THROUGH_` and `t`, an underscore more while a column has it.
fn surrogate_name(t: usize, columns: &[(String, DataType)]) -> String {
    let mut name = format!("PTF_PASS_THROUGH_{t}");
    while columns.iter().any(|(n, _)| *n == name) {
        name.push('_');
    }
    name
}
