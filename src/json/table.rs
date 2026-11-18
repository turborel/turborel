//! JSON_TABLE [ISO/IEC 9075-2:2023 §7.11], `<JSON table>`: the rows a
//! JSON text gives as a table, of its operands already evaluated.
//!
//! The Syntax Rules define a `<JSON table>` by a transformation into
//! `<JSON table primitive>`s, one for each `<JSON table columns clause>`,
//! joined laterally as its plan says [SR1k]. A `JsonTable` is that
//! transformation done, by the binder: each primitive a `Primitive`, its
//! row pattern parsed and its columns each with every clause explicit
//! [SR1k iii 6 C], and the plan a tree of `PlanNode`s. Its rows are made
//! by a `Cursor` (`table/rows.rs`), one at a time, as they are read, so no
//! table but the SQL/JSON sequence of a row pattern is held whole.
//!
//! A primitive's context item is the JSON table's for the outermost, else
//! the item its parent's row was made of, its chaining column [SR1k iii 6
//! B II, GR1e ii 4]. \[DESIGN\] That item is passed on as it is, not as
//! the JSON text it serializes to (UV085) and is parsed from again, since
//! the text parses to an equivalent item [§9.43 GR2a ii]; it is first
//! checked to have a JSON text, so an item that has none, a datetime,
//! raises its exception condition as CHAINST does \[GR1e ii 4 A II\]. The
//! values of a column are those of §9.48 and §9.49, the rules of
//! JSON_VALUE and JSON_QUERY (`json/query.rs`), in `table/column.rs`.

mod column;
mod rows;

pub use rows::{Cursor, TableRow};

use super::{Item, Path, QueryBehavior, Quotes, Wrapper};
use crate::value::{DataType, Value};

/// [§7.11] A `<JSON table error behavior>`, what a JSON table gives in
/// place of an exception condition of a row pattern. \[SR1b\] Without
/// one, EMPTY ON ERROR is implicit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableErrorBehavior {
    /// `ERROR`: the exception condition, raised.
    Error,
    /// `EMPTY`: the primitive whose row pattern raised it is an empty
    /// table \[GR1e i 2\].
    Empty,
}

/// [§7.11] A behavior of a regular column ON EMPTY or ON ERROR, every one
/// explicit [SR1e iii, SR1e iv].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// ERROR.
    Error,
    /// NULL.
    Null,
    /// DEFAULT `<value expression>`: the operand of that place among the
    /// JSON table's DEFAULTs, evaluated only when it is the column's value.
    Default(usize),
}

/// [§7.11 SR1k iii 6 C] A `<JSON table primitive column definition>`
/// other than a chaining column, every clause explicit.
#[derive(Debug, Clone)]
pub enum TableColumn {
    /// `<JSON table ordinality column definition>`: the row's ordinal.
    Ordinality,
    /// `<JSON table regular column definition>`: its path's sequence cast
    /// to its type [GR1e ii 2].
    Regular {
        /// The column path, explicit or implicit [SR1e v].
        path: Path,
        /// The `<data type>`.
        to: DataType,
        /// The behavior ON EMPTY.
        on_empty: Slot,
        /// The behavior ON ERROR.
        on_error: Slot,
    },
    /// `<JSON table formatted column definition>`: its path's sequence
    /// serialized to its type [GR1e ii 3].
    Formatted(Box<Formatted>),
}

/// [§7.11 SR1f] A `<JSON table formatted column definition>`'s clauses,
/// every one explicit, of a character string type, FORMAT JSON.
#[derive(Debug, Clone)]
pub struct Formatted {
    /// The column path, explicit or implicit [SR1f xii].
    pub path: Path,
    /// The `<data type>`.
    pub returning: DataType,
    /// The wrapper behavior [SR1f v, vi].
    pub wrapper: Wrapper,
    /// The quotes behavior [SR1f viii].
    pub quotes: Quotes,
    /// The behavior ON EMPTY [SR1f x].
    pub on_empty: QueryBehavior,
    /// The behavior ON ERROR [SR1f xi].
    pub on_error: QueryBehavior,
}

/// [§7.11 SR1k iii 6] A `<JSON table primitive>`: the row pattern of one
/// `<JSON table columns clause>` and its columns.
#[derive(Debug, Clone)]
pub struct Primitive {
    /// Its `<JSON table path name>`, if written, for EXPLAIN.
    pub name: Option<String>,
    /// The text of its `<JSON path specification>`, for EXPLAIN.
    pub text: String,
    /// Its row pattern, parsed.
    pub path: Path,
    /// Its columns, each with its place among the JSON table's.
    pub columns: Vec<(usize, TableColumn)>,
}

/// [§7.11 SR1k] A node of the plan tree, its operands by their places in
/// `JsonTable::nodes`.
#[derive(Debug, Clone)]
pub enum PlanNode {
    /// A `<JSON table path name>`: the rows of the primitive of that
    /// place in `JsonTable::primitives`.
    Primitive(usize),
    /// `<JSON table plan outer>` or `<JSON table plan inner>`: each row of
    /// the primitive joined laterally to the rows of `child` of its item,
    /// kept null extended if it has none and the join is OUTER [SR1k iii
    /// 2, 3].
    Parent {
        /// The primitive of the first operand.
        primitive: usize,
        /// INNER, else OUTER.
        inner: bool,
        /// The node of the second operand.
        child: usize,
    },
    /// `<JSON table plan union>` or `<JSON table plan cross>`: the rows of
    /// each operand in turn, null extended, or their product [SR1k iii 4,
    /// 5].
    Siblings {
        /// CROSS, else UNION.
        cross: bool,
        /// The operands' nodes, two or more, in order.
        operands: Vec<usize>,
    },
}

/// [§7.11] A `<JSON table>`, transformed by its Syntax Rules.
#[derive(Debug, Clone)]
pub struct JsonTable {
    /// The primitives, the outermost first, then in the order their
    /// COLUMNS are written [SR1i iv].
    pub primitives: Vec<Primitive>,
    /// The nodes of the plan tree.
    pub nodes: Vec<PlanNode>,
    /// The node that is the whole plan.
    pub top: usize,
    /// The PASSING arguments' identifiers and whether each is FORMAT JSON.
    pub passing: Vec<(String, bool)>,
    /// The `<JSON table error behavior>`, explicit or implicit.
    pub on_error: TableErrorBehavior,
    /// How many columns it has: its ordinality, regular and formatted
    /// columns at every depth \[SR1l\].
    pub width: usize,
}

impl JsonTable {
    /// What EXPLAIN shows of it: its outermost row pattern and its plan,
    /// each primitive by its path name, else by its row pattern.
    pub fn render(&self) -> String {
        let text = self.primitives.first().map_or("", |p| p.text.as_str());
        format!("JSON_TABLE '{}' of {} columns PLAN ({})", text.replace('\'', "''"), self.width, self.render_node(self.top))
    }

    /// The plan of `node`, as written.
    fn render_node(&self, node: usize) -> String {
        match self.nodes.get(node) {
            None => String::new(),
            Some(PlanNode::Primitive(p)) => self.render_primitive(*p),
            Some(PlanNode::Parent { primitive, inner, child }) => {
                format!("{} {} ({})", self.render_primitive(*primitive), if *inner { "INNER" } else { "OUTER" }, self.render_node(*child))
            }
            Some(PlanNode::Siblings { cross, operands }) => {
                let shown: Vec<String> = operands.iter().map(|&o| format!("({})", self.render_node(o))).collect();
                shown.join(if *cross { " CROSS " } else { " UNION " })
            }
        }
    }

    /// The primitive `p` by its path name, else by its row pattern.
    fn render_primitive(&self, p: usize) -> String {
        match self.primitives.get(p) {
            Some(Primitive { name: Some(name), .. }) => name.clone(),
            Some(Primitive { name: None, text, .. }) => format!("'{}'", text.replace('\'', "''")),
            None => String::new(),
        }
    }
}

/// [§9.43 GR2a ii] Whether `item` has a JSON text: it neither is nor
/// holds a datetime or an approximate number that is not finite. Items
/// nest at most `MAX_NESTING` deep, which bounds the walk (RULES.md
/// R-CF-6).
fn has_text(item: &Item) -> bool {
    match item {
        Item::Null => true,
        Item::Scalar(scalar) => match scalar.value() {
            Value::Datetime(_) => false,
            Value::Approx(f) => f.is_finite(),
            Value::Null
            | Value::Char(_)
            | Value::Exact(_)
            | Value::DecFloat(_)
            | Value::Binary(_)
            | Value::Lob(_)
            | Value::Bool(_)
            | Value::Interval(_)
            | Value::Array(_)
            | Value::Multiset(_)
            | Value::Row(_)
            | Value::Structured(_)
            | Value::Reference(_)
            | Value::Json(_)
            | Value::Xml(_)
            | Value::Extension(_) => true,
        },
        Item::Array(elements) => elements.iter().all(has_text),
        Item::Object(members) => members.iter().all(|m| has_text(&m.value)),
    }
}

#[cfg(test)]
mod tests;
