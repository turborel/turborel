//! [ISO/IEC 9075-2:2023 §7.15 SR13, SR14, GR6, §9.23 SR9] A window's row
//! pattern recognition bound: its measures and common syntax, over the
//! rows of the table expression its window functions are over; and the
//! binder both it and a `<row pattern recognition clause>` bind theirs
//! with.

use std::collections::BTreeSet;

use super::{Recognizing, Shape, common, table};
use crate::ast::{Exclusion as Excluded, FrameBound, FrameUnits, PatternCommon, RowPattern, WindowFunction};
use crate::binder::collation::Derived;
use crate::binder::dependencies::Known;
use crate::binder::from::FromItem;
use crate::binder::views::{Source, unconstrained};
use crate::binder::window::spec::{Described, Keys};
use crate::binder::{Binder, Context, Scope, rule};
use crate::catalog::{ColumnDesc, QualifiedName, TableDesc};
use crate::error::{At, Code, Result, SqlError};
use crate::logical::{Expr, Matching, PatternValue, Slotted, WindowCall, WindowKind, WindowMatching, expr_columns, map_expr_columns};
use crate::value::DataType;

/// Of a window's row pattern recognition, the column of a row of its
/// input that its classifier is, as bound, before the window's input is
/// known (`placed`). An eighth of the range of columns, it is past any a
/// row has and below the placeholders of nested window functions, at a
/// quarter, and of window functions, at a half, so none meets another.
const CLASSIFIED: usize = usize::MAX >> 3_u32;

/// A window's row pattern recognition bound, and the type of each of its
/// measures.
pub(in crate::binder) type Matched = (WindowMatching, Vec<DataType>);

/// [ISO/IEC 9075-2:2023 §7.7, §7.15] A binder of row pattern measures and
/// definitions over the row pattern input table of `columns`, of the row
/// pattern variables, the input's partitioning and dependencies `shape`
/// gives: one copy of the input's columns each, the universal row pattern
/// variable's last, unnamed and the one seen unqualified [§7.9 SR18]. It
/// sees no outer reference [§7.8 SR2, §7.9 SR13] and no row pattern input
/// name [§7.6 SR8b i 5].
pub(super) fn pattern_binder<'a>(cx: Context<'a>, columns: &[ColumnDesc], shape: Shape) -> Result<Binder<'a>> {
    let width = columns.len();
    let (mut items, mut names) = (Vec::new(), Vec::new());
    for k in 0..=shape.names.len() {
        let correlation = shape.names.get(k).cloned();
        let name = QualifiedName { auth: String::new(), ident: String::new() };
        let desc = TableDesc { columns: columns.to_vec(), ..unconstrained(&name) };
        let mut item = FromItem::alone(name, 0, (desc, Source::Table));
        item.offset = k.checked_mul(width).ok_or_else(|| SqlError::new(Code::Unsupported, "[DESIGN] too many row pattern variables"))?;
        names.extend(columns.iter().map(|c| correlation.as_ref().map_or_else(|| c.name.clone(), |v| format!("{v}.{}", c.name))));
        item.correlation = correlation;
        items.push(item);
    }
    let visible = (0..width).map(|c| (shape.names.len(), c)).collect();
    let cx = Context { uses: None, selected: None, referencing: None, draws: false, ..cx };
    let mut binder = Binder::new(cx, None, (items, names, visible), BTreeSet::new());
    binder.recognizing = Some(Box::new(Recognizing::new(shape)));
    Ok(binder)
}

/// [ISO/IEC 9075-2:2023 §7.8 SR3, §7.9 SR14] That no subquery of the
/// measures `measures` and definitions of `matching` references a row
/// pattern variable: none references the binder's row.
pub(super) fn no_variable_in_subquery(matching: &Matching, measures: &Slotted<Vec<Expr>>) -> Result<()> {
    let (exprs, conds) = matching.parts();
    let mut subqueries = Vec::new();
    measures.exprs().chain(exprs).for_each(|e| subqueries.extend(e.all_subqueries()));
    conds.for_each(|c| subqueries.extend(c.all_subqueries()));
    if subqueries.iter().any(|s| !s.outer.is_empty()) {
        return Err(rule(
            "[ISO/IEC 9075-2:2023 §7.8 SR3, §7.9 SR14] a subquery of a <row pattern measures> or <row pattern definition search condition> references no row pattern variable",
        ));
    }
    Ok(())
}

impl Binder<'_> {
    /// [ISO/IEC 9075-2:2023 §7.15 SR13, SR14] The row pattern recognition
    /// of the window `described`, of `keys`, its window functions bound in
    /// `scope`, if it has one: of a frame of ROWS BETWEEN CURRENT ROW AND
    /// a bound, EXCLUDE NO OTHERS, of no `^` or `$`, INITIAL implicit
    /// [§7.9 SR3b ii].
    pub(in crate::binder) fn window_matching(&self, described: &Described, keys: &Keys, scope: Scope<'_>) -> Result<Option<Matched>> {
        let Some(recognition) = &described.recognition else { return Ok(None) };
        let common = &recognition.common;
        window_rules(described, common, scope).or_at(common.at)?;
        let (columns, positions) = self.window_input();
        let variables = common::variables(common)?;
        let partition = keys
            .partition
            .iter()
            .filter_map(|e| if let Expr::Column(at) = e { positions.iter().position(|q| q == at) } else { None })
            .collect();
        let shape = Shape {
            names: common::names(&variables),
            width: columns.len(),
            classifier: CLASSIFIED,
            table: false,
            partition,
            known: Known::default(),
        };
        let binder = pattern_binder(self.context(), &columns, shape)?;
        let Some(r) = binder.recognizing.as_deref() else {
            return Err(SqlError::new(Code::Api, "[DESIGN] a row pattern binder recognizes"));
        };
        let defines = common::defines(&binder, r, common, &variables)?;
        let (expr, columns_of) = table::measures(&binder, r, &recognition.measures)?;
        let mut measures = Slotted { expr, values: r.take_slots() };
        let seek = common.seek == Some(true);
        let mut matching = common::matching(common, variables, defines, seek)?;
        no_variable_in_subquery(&matching, &measures)?;
        self.note_read(&measures, &matching)?;
        positioned(&mut measures, &mut matching, &positions);
        let names = recognition.measures.iter().map(|m| m.name.clone()).collect();
        let types = columns_of.into_iter().map(|c| c.data_type).collect();
        Ok(Some((WindowMatching { matching, measures, names }, types)))
    }

    /// Each column of the table expression the values of `measures` and of
    /// `matching`'s definitions read, noted as used, so that it is read
    /// and its privileges checked as a column a select list names is.
    fn note_read(&self, measures: &Slotted<Vec<Expr>>, matching: &Matching) -> Result<()> {
        let mut read = Vec::new();
        let defined = matching.defines.iter().flatten().flat_map(|d| d.values.iter());
        measures.values.iter().chain(defined).flat_map(PatternValue::exprs).for_each(|e| expr_columns(e, &mut read));
        read.sort_unstable();
        read.dedup();
        for c in read {
            let Some(&(i, k)) = self.visible.get(c) else { continue };
            if let Some(item) = self.items.get(i) {
                self.note(item, k)?;
            }
        }
        Ok(())
    }

    /// The columns of the table expression its window functions are over,
    /// those it sees unqualified, and the position of each in its row.
    fn window_input(&self) -> (Vec<ColumnDesc>, Vec<usize>) {
        let (mut columns, mut positions) = (Vec::new(), Vec::new());
        for &(i, c) in &self.visible {
            let Some(item) = self.items.get(i) else { continue };
            let Some(column) = item.desc.columns.get(c) else { continue };
            // Nothing a window's row pattern binds reads whether a column
            // is known not null.
            columns.push(ColumnDesc { not_null: false, default: None, generated: None, ..column.clone() });
            positions.push(item.offset.saturating_add(item.position(c)));
        }
        (columns, positions)
    }
}

impl Binder<'_> {
    /// How the `<window row pattern measure>` `name` of `w` came by its
    /// collation: as its `<row pattern measure expression>` did.
    pub(in crate::binder) fn measure_derived(&self, w: &WindowFunction, name: &str) -> Result<Derived> {
        let described = self.described_window(&w.over)?;
        let Some(recognition) = &described.recognition else { return Ok(Derived::Yielding) };
        let Some(m) = recognition.measures.iter().find(|m| m.name == name) else { return Ok(Derived::Yielding) };
        let variables = common::variables(&recognition.common)?;
        let (columns, _) = self.window_input();
        let shape = Shape {
            names: common::names(&variables),
            width: columns.len(),
            classifier: CLASSIFIED,
            table: false,
            partition: Vec::new(),
            known: Known::default(),
        };
        pattern_binder(self.context(), &columns, shape)?.derived(&m.expr)
    }
}

/// [ISO/IEC 9075-2:2023 §7.15 SR14] That the window `described`, its
/// window functions bound in `scope`, may hold the row pattern recognition
/// of `common`. Its frame is one `frame_rules` takes and its pattern has
/// no `^` or `$`. A grouped query's is not supported.
fn window_rules(described: &Described, common: &PatternCommon, scope: Scope<'_>) -> Result<()> {
    frame_rules(described)?;
    if anchored(&common.pattern) {
        return Err(rule("[ISO/IEC 9075-2:2023 §7.15 SR14e] the pattern of a window has no ^ and no $"));
    }
    if let Scope::Groups(..) = scope {
        return Err(SqlError::new(Code::Unsupported, "[DESIGN] row pattern recognition in a window of a grouped query is not supported"));
    }
    Ok(())
}

/// The values of `measures` and of `matching`'s definitions, each column
/// of the table expression they read made the column of a row of the
/// window's input it is of `positions`.
fn positioned(measures: &mut Slotted<Vec<Expr>>, matching: &mut Matching, positions: &[usize]) {
    let input = |c: usize| positions.get(c).copied().unwrap_or(c);
    let defined = matching.defines.iter_mut().flatten().flat_map(|d| d.values.iter_mut());
    measures.values.iter_mut().chain(defined).for_each(|v| v.map_exprs(&mut |e| map_expr_columns(e, &input)));
}

/// [ISO/IEC 9075-2:2023 §7.15 SR14b to SR14d] That the frame of the window
/// `described` is ROWS BETWEEN CURRENT ROW AND a bound, EXCLUDE NO OTHERS.
fn frame_rules(described: &Described) -> Result<()> {
    let Some(f) = &described.frame else {
        return Err(rule("[ISO/IEC 9075-2:2023 §7.15 SR14a] a window of row pattern recognition has a <window frame clause>"));
    };
    if f.units != FrameUnits::Rows {
        return Err(rule("[ISO/IEC 9075-2:2023 §7.15 SR14b] the frame of a window of row pattern recognition is of ROWS"));
    }
    if !matches!(f.start, FrameBound::CurrentRow) || f.end.is_none() {
        return Err(rule(
            "[ISO/IEC 9075-2:2023 §7.15 SR14c] the frame of a window of row pattern recognition is BETWEEN CURRENT ROW AND a bound",
        ));
    }
    if f.exclusion != Excluded::NoOthers {
        return Err(rule("[ISO/IEC 9075-2:2023 §7.15 SR14d] the frame of a window of row pattern recognition is EXCLUDE NO OTHERS"));
    }
    Ok(())
}

/// Whether `pattern` has `^` or `$`.
fn anchored(pattern: &RowPattern) -> bool {
    let mut stack = vec![pattern];
    while let Some(p) = stack.pop() {
        match p {
            RowPattern::Start | RowPattern::End => return true,
            RowPattern::Variable(..) | RowPattern::Empty => {}
            RowPattern::Concat(list) | RowPattern::Alternation(list) | RowPattern::Permute(list) => stack.extend(list),
            RowPattern::Quantified(q) => stack.push(&q.pattern),
            RowPattern::Excluded(inner) => stack.push(inner),
        }
    }
    false
}

/// `call`'s frame's row pattern recognition, if it has one, its values'
/// classifier the column `at` after its window's input's.
pub(in crate::binder) fn placed(call: &mut WindowCall, at: usize) {
    let Some(m) = &mut call.frame.matching else { return };
    let place = |c: usize| c.checked_sub(CLASSIFIED).map_or(c, |k| at.saturating_add(k));
    let m = std::sync::Arc::make_mut(m);
    let mut map = |v: &mut PatternValue| v.map_exprs(&mut |e| map_expr_columns(e, &place));
    m.measures.values.iter_mut().for_each(&mut map);
    m.matching.defines.iter_mut().flatten().for_each(|d| d.values.iter_mut().for_each(&mut map));
}

/// [ISO/IEC 9075-2:2023 §9.23 SR9, §6.10 GR1e] The `<window row pattern
/// measure>` `name` of a window of the row pattern recognition `matched`:
/// a measure of it, of its type.
pub(in crate::binder) fn window_measure(name: &str, matched: Option<&Matched>) -> Result<(WindowKind, DataType)> {
    let found = matched.and_then(|(m, types)| m.names.iter().position(|n| n == name).and_then(|k| Some((k, types.get(k)?.clone()))));
    let Some((k, t)) = found else {
        return Err(rule(format!(
            "[ISO/IEC 9075-2:2023 §9.23 SR9] {name} OVER a window names a <row pattern measure definition> of its <row pattern measures>"
        )));
    };
    Ok((WindowKind::Measure(k), t))
}
