//! \[DESIGN\] A procedure's program compiled (RULES.md D841): a node of
//! code for each step of the bound program, with what the step needs as
//! it runs found once, as it is compiled.

use std::sync::Arc;

use super::super::super::super::area::commands::Command;
use super::super::{diagnostics, step_command};
use super::change::Change;
use super::valued::{Stores, Valued};
use crate::ast::FetchOrientation;
use crate::logical::{self, Contained, Handler, Initial, Program, Repeating, Step};
use crate::params::RoutineScope;
use crate::value::DataType;

/// A statement of the program compiled, and how the diagnostics area
/// names it.
#[derive(Debug)]
pub(super) struct Node {
    pub(super) code: Code,
    /// The statement's command, as the area names it as it begins.
    pub(super) command: Command,
    /// Whether it is GET DIAGNOSTICS, which leaves the area as it was.
    pub(super) reads_area: bool,
}

/// What a compiled statement does.
#[derive(Debug)]
pub(super) enum Code {
    Block(Box<Block>),
    Assign(Box<Assign>),
    If(Box<Branches>),
    Case(Box<Branches>),
    Loop(Box<Looped>),
    For(Box<ForEach>),
    Leave(usize),
    Iterate(usize),
    Return(Option<Valued>),
    Signal(Box<Signal>),
    Into(Box<Into>),
    Cursor(Box<logical::CursorStep>),
    Statement(Box<Statement>),
}

/// [ISO/IEC 9075-4:2006 draft §14.1] A compound statement compiled.
#[derive(Debug)]
pub(super) struct Block {
    pub(super) label: usize,
    pub(super) atomic: bool,
    pub(super) variables: Vec<Variable>,
    pub(super) cursors: Vec<usize>,
    /// Its handlers, as bound, and the action of each compiled.
    pub(super) handlers: Vec<Handler>,
    pub(super) actions: Vec<Node>,
    pub(super) statements: Vec<Node>,
}

/// [§14.4] A variable: its slot, declared type and initial value.
#[derive(Debug)]
pub(super) struct Variable {
    pub(super) slot: usize,
    pub(super) data_type: DataType,
    pub(super) initial: Begun,
}

/// [§14.4 GR1] The value a variable is given as it is created.
#[derive(Debug)]
pub(super) enum Begun {
    Null,
    /// The empty collection: a multiset if true, else an array.
    Empty(bool),
    Value(Valued),
}

/// [§14.5] An assignment statement compiled.
#[derive(Debug)]
pub(super) struct Assign {
    /// Its targets, each a slot found once if it is a variable or
    /// parameter as a whole.
    pub(super) targets: Vec<logical::Target>,
    pub(super) stores: Stores,
    pub(super) source: Option<Valued>,
    pub(super) scope: Arc<RoutineScope>,
    pub(super) at: usize,
}

/// [§14.6, §14.7] IF or CASE compiled.
#[derive(Debug)]
pub(super) struct Branches {
    pub(super) arms: Vec<(Valued, Vec<Node>)>,
    pub(super) otherwise: Option<Vec<Node>>,
    pub(super) at: usize,
}

/// How an iterated statement repeats, its condition compiled.
#[derive(Debug)]
pub(super) enum Repeat {
    Always,
    While(Valued),
    Until(Valued),
}

/// [§14.10 to §14.12] LOOP, WHILE or REPEAT compiled.
#[derive(Debug)]
pub(super) struct Looped {
    pub(super) label: usize,
    pub(super) kind: Repeat,
    pub(super) statements: Vec<Node>,
}

/// [§14.13] FOR compiled: its cursor is the session's, as the
/// interpreter opens it.
#[derive(Debug)]
pub(super) struct ForEach {
    pub(super) label: usize,
    pub(super) cursor: usize,
    pub(super) slots: Vec<usize>,
    pub(super) statements: Vec<Node>,
    pub(super) next: FetchOrientation,
}

/// [§17.2, §17.3] SIGNAL or RESIGNAL, its items' values compiled.
#[derive(Debug)]
pub(super) struct Signal {
    pub(super) signal: logical::Signal,
    pub(super) items: Vec<Valued>,
}

/// [ISO/IEC 9075-2:2023 §14.7] SELECT INTO compiled: its query, read as a
/// point read once it is one, and its targets.
#[derive(Debug)]
pub(super) struct Into {
    pub(super) query: Valued,
    pub(super) targets: Vec<crate::ast::ParameterSpecification>,
    pub(super) stores: Stores,
    pub(super) scope: Arc<RoutineScope>,
}

/// A statement the session runs, and its change run straight on its
/// table once its kept plans show it is one of the shapes that may be.
#[derive(Debug)]
pub(super) struct Statement {
    pub(super) contained: Contained,
    pub(super) change: Option<Change>,
}

/// The program `program`'s body compiled.
pub(super) fn compiled(program: &Program) -> Node {
    node(&program.body)
}

/// The statements `steps` compiled.
fn nodes(steps: &[Step]) -> Vec<Node> {
    steps.iter().map(node).collect()
}

/// The step `step` compiled.
fn node(step: &Step) -> Node {
    Node { code: code(step), command: step_command(step), reads_area: diagnostics(step) }
}

/// What `step` does, compiled.
fn code(step: &Step) -> Code {
    match step {
        Step::Block(b) => Code::Block(Box::new(block(b))),
        Step::Assign(a) => Code::Assign(Box::new(Assign {
            stores: Stores::of_targets(&a.targets, &a.scope),
            targets: a.targets.clone(),
            source: a.source.as_ref().map(Valued::of),
            scope: Arc::clone(&a.scope),
            at: a.at,
        })),
        Step::If(b) => Code::If(Box::new(branches(b))),
        Step::Case(b) => Code::Case(Box::new(branches(b))),
        Step::Loop(l) => Code::Loop(Box::new(Looped { label: l.label, kind: repeat(&l.kind), statements: nodes(&l.statements) })),
        Step::For(f) => Code::For(Box::new(ForEach {
            label: f.label,
            cursor: f.cursor,
            slots: f.slots.clone(),
            statements: nodes(&f.statements),
            next: FetchOrientation::Next,
        })),
        Step::Leave(label) => Code::Leave(*label),
        Step::Iterate(label) => Code::Iterate(*label),
        Step::Return(leaf) => Code::Return(leaf.as_ref().map(Valued::of)),
        Step::Signal(s) => {
            Code::Signal(Box::new(Signal { items: s.items.iter().map(|(_, l)| Valued::of(l)).collect(), signal: (**s).clone() }))
        }
        Step::Into(i) => Code::Into(Box::new(Into {
            query: Valued::of_query(&i.query),
            stores: Stores::of_specs(&i.targets, &i.scope),
            targets: i.targets.clone(),
            scope: Arc::clone(&i.scope),
        })),
        Step::Cursor(c) => Code::Cursor(c.clone()),
        Step::Statement(c) => Code::Statement(Box::new(Statement { change: Change::of(c.statement()), contained: (**c).clone() })),
        // A body holding one is not compiled (`Compiled::of`).
        Step::Call(_) => Code::Leave(usize::MAX),
    }
}

/// [§14.1] The compound statement `b` compiled.
fn block(b: &logical::Block) -> Block {
    let variables = b
        .variables
        .iter()
        .map(|v| Variable { slot: v.slot, data_type: v.data_type.clone(), initial: begun(&v.initial, &v.data_type) })
        .collect();
    Block {
        label: b.label,
        atomic: b.atomic,
        variables,
        cursors: b.cursors.clone(),
        handlers: b.handlers.clone(),
        actions: b.handlers.iter().map(|h| node(&h.action)).collect(),
        statements: nodes(&b.statements),
    }
}

/// [§14.4 GR1] The initial value `initial` of a variable of `data_type`.
fn begun(initial: &Initial, data_type: &DataType) -> Begun {
    match initial {
        Initial::Null => Begun::Null,
        Initial::Empty => Begun::Empty(matches!(data_type.represented(), DataType::Multiset(_))),
        Initial::Value(leaf) => Begun::Value(Valued::of(leaf)),
    }
}

/// IF's or CASE's branches compiled.
fn branches(b: &logical::Branches) -> Branches {
    Branches {
        arms: b.arms.iter().map(|(c, steps)| (Valued::of(c), nodes(steps))).collect(),
        otherwise: b.otherwise.as_deref().map(nodes),
        at: b.at,
    }
}

/// How an iterated statement repeats, compiled.
fn repeat(kind: &Repeating) -> Repeat {
    match kind {
        Repeating::Always => Repeat::Always,
        Repeating::While(c) => Repeat::While(Valued::of(c)),
        Repeating::Until(c) => Repeat::Until(Valued::of(c)),
    }
}
