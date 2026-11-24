//! [ISO/IEC 9075-4:2006 draft §11.2, Clause 14] An SQL control statement,
//! bound: a program of steps over a frame of values (RULES.md D76).
//!
//! The frame is a routine's or triggered action's dynamic values: its
//! parameters, or its transition variables, then each SQL variable of its
//! compound statements, each in a slot of its own, laid out as the
//! program is bound. A value, a condition or a query of the program is a
//! leaf: bound once, as a function's `<return statement>` is (RULES.md
//! D73), a subquery whose outer references are slots of the frame, each
//! filled with the slot's value as the leaf is evaluated. A statement the
//! session runs, an INSERT, UPDATE, DELETE, MERGE, CALL, GET DIAGNOSTICS,
//! positioned change or cursor statement, keeps its syntax, and the scope
//! of the variables and cursors it sees, bound as it runs.
//!
//! In the body of a function, which runs where no SQL-session may run its
//! statements, a CALL is bound with the program, the procedure's body a
//! program of its own (`Called`).
//!
//! Labels are numbered as they are bound, so LEAVE and ITERATE name the
//! statement they leave or iterate by its number; conditions are their
//! SQLSTATE values, or the names of those declared with none [§14.3].

use std::sync::Arc;

use super::Subquery;
use crate::ast::{ConditionItem, FetchOrientation, HandlerType, ParameterSpecification, SqlStatement};
use crate::catalog::{ParameterMode, QualifiedName};
use crate::error::SqlState;
use crate::params::RoutineScope;
use crate::value::DataType;

/// \[DESIGN\] A bound SQL control statement.
#[derive(Debug, Clone)]
pub(crate) struct Program {
    /// How many slots of the frame come before the variables': the
    /// routine's parameters, or the action's transition variables.
    pub(crate) base: usize,
    /// The declared type of each variable's slot, the first after `base`.
    pub(crate) variables: Vec<DataType>,
    /// The local cursors, by the number its statements name them by.
    pub(crate) cursors: Vec<Cursor>,
    /// Its statement.
    pub(crate) body: Step,
    /// The scope of the routine or triggered action it is the body of, of
    /// no variables: what its frame's first slots are.
    pub(crate) scope: Arc<RoutineScope>,
    /// \[DESIGN\] Where each of its steps begins in the text it was read
    /// from, in the order `Step::steps` walks them from its body, the body
    /// first, which a debugger places a step by (RULES.md D1003).
    pub(crate) places: Vec<usize>,
    /// \[DESIGN\] The routine or trigger it is the body of, if it is one,
    /// as a debugger names it (RULES.md D1004).
    pub(crate) origin: Option<crate::debugger::Origin>,
}

/// A value, condition or query of a program, bound as a subquery of the
/// frame: of a value, one row of one column; of a condition, one row of
/// its truth value; of a query, its rows.
#[derive(Debug, Clone)]
pub(crate) struct Leaf {
    /// The subquery, whose outer expressions are over the frame.
    pub(crate) query: Arc<Subquery>,
    /// Where it is written.
    pub(crate) at: usize,
}

/// One step of a program: an `<SQL procedure statement>`, or a part of one.
#[derive(Debug, Clone)]
pub(crate) enum Step {
    /// [§14.1] A `<compound statement>`.
    Block(Box<Block>),
    /// [§14.5] An `<assignment statement>`.
    Assign(Box<Assign>),
    /// [§14.7] An `<if statement>`: each condition and its steps, the
    /// first true one's taken, else ELSE's.
    If(Box<Branches>),
    /// [§14.6] A `<case statement>`, each WHEN a condition: as IF, but
    /// with no ELSE and no WHEN true, SQLSTATE 20000.
    Case(Box<Branches>),
    /// [§14.10 to §14.12] A LOOP, WHILE or REPEAT.
    Loop(Box<Looped>),
    /// [§14.13] A `<for statement>`.
    For(Box<ForEach>),
    /// [§14.9] LEAVE the statement of the label numbered so.
    Leave(usize),
    /// [§14.8] ITERATE the statement of the label numbered so.
    Iterate(usize),
    /// [ISO/IEC 9075-2:2023 §16.2] A `<return statement>`, its value; none
    /// of NULL.
    Return(Option<Leaf>),
    /// [§17.2, §17.3] A `<signal statement>` or `<resignal statement>`.
    Signal(Box<Signal>),
    /// [ISO/IEC 9075-2:2023 §14.7] A `<select statement: single row>`: its
    /// query, and its targets.
    Into(Box<Into>),
    /// [§12.2 to §12.4] An OPEN, FETCH or CLOSE of a local cursor.
    Cursor(Box<CursorStep>),
    /// A statement the session runs.
    Statement(Box<Contained>),
    /// [ISO/IEC 9075-2:2023 §16.1] A CALL bound with the program.
    Call(Box<Called>),
}

/// [§14.1] A `<compound statement>`, bound.
#[derive(Debug, Clone)]
pub(crate) struct Block {
    /// The number of its label, explicit or implicit [SR2].
    pub(crate) label: usize,
    /// Whether it is ATOMIC.
    pub(crate) atomic: bool,
    /// Its variables, in the order they are declared.
    pub(crate) variables: Vec<Declared>,
    /// The numbers of the cursors it declares, closed as it ends [GR4].
    pub(crate) cursors: Vec<usize>,
    /// Its handlers.
    pub(crate) handlers: Vec<Handler>,
    /// Its statements.
    pub(crate) statements: Vec<Step>,
}

/// [§14.4] An SQL variable declared.
#[derive(Debug, Clone)]
pub(crate) struct Declared {
    /// \[DESIGN\] Its `<SQL variable name>`, which a debugger shows it by.
    pub(crate) name: String,
    /// Its slot in the frame.
    pub(crate) slot: usize,
    /// Its declared type.
    pub(crate) data_type: DataType,
    /// Its initial value, of its `<default option>` [GR1].
    pub(crate) initial: Initial,
}

/// [§14.4 GR1] The value an SQL variable is given as it is created.
#[derive(Debug, Clone)]
pub(crate) enum Initial {
    /// The null value: of no `<default clause>`, or DEFAULT NULL.
    Null,
    /// ARRAY `[` `]` or MULTISET `[` `]`, the collection of no element of
    /// the variable's type.
    Empty,
    /// A literal, an authorization identifier or a datetime value function.
    Value(Leaf),
}

/// [§14.2] A handler declared.
#[derive(Debug, Clone)]
pub(crate) struct Handler {
    /// CONTINUE, EXIT or UNDO.
    pub(crate) kind: HandlerType,
    /// The conditions it is associated with.
    pub(crate) conditions: Vec<Handled>,
    /// Its `<handler action>`.
    pub(crate) action: Step,
}

/// [§14.2 SR6 to SR12] A condition a handler is associated with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Handled {
    /// An SQLSTATE value, of an `<sqlstate value>` or of a condition
    /// declared for one [SR10, SR11].
    State(SqlState),
    /// A condition declared for no SQLSTATE value, by its name.
    Name(String),
    /// SQLEXCEPTION: every SQLSTATE value of an exception [SR6].
    Exception,
    /// SQLWARNING: every SQLSTATE value of class 01.
    Warning,
    /// NOT FOUND: every SQLSTATE value of class 02.
    NotFound,
}

impl Handled {
    /// Whether it is specific, not general [SR7].
    pub(crate) fn specific(&self) -> bool {
        matches!(self, Handled::State(_) | Handled::Name(_))
    }
}

/// [§14.5] An `<assignment statement>`, bound.
#[derive(Debug, Clone)]
pub(crate) struct Assign {
    /// Its targets, in order.
    pub(crate) targets: Vec<Target>,
    /// Its source: a row of as many values as there are targets, the
    /// first row a query gives, of no row the null values [§14.5 SR1];
    /// none of NULL.
    pub(crate) source: Option<Leaf>,
    /// The scope its targets are named in.
    pub(crate) scope: Arc<RoutineScope>,
    /// Where SET is written.
    pub(crate) at: usize,
}

/// [§14.5] A part of a `<modified field reference>` or a `<mutator
/// reference>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Modified {
    /// The field at this position of a row [SR6].
    Field(usize),
    /// The attribute at this position of a structured value, set as its
    /// mutator sets it [SR5; ISO/IEC 9075-2:2023 §4.9.3.3].
    Attribute(usize),
}

/// [§14.5] An `<assignment target>`.
#[derive(Debug, Clone)]
pub(crate) struct Target {
    /// The `<target specification>`: an SQL variable or parameter, a new
    /// transition variable's column, or an element of an array one holds.
    pub(crate) spec: ParameterSpecification,
    /// Of a `<modified field reference>` or a `<mutator reference>`, each
    /// field or attribute of the value of the one before, the outermost
    /// first (Features T051, S023).
    pub(crate) fields: Vec<Modified>,
    /// The declared type of what is assigned: of its last field, if it has
    /// one, else of the target.
    pub(crate) data_type: DataType,
}

/// [§14.6, §14.7] Conditions, each with its steps, and the steps of ELSE.
#[derive(Debug, Clone)]
pub(crate) struct Branches {
    /// Each condition and its steps.
    pub(crate) arms: Vec<(Leaf, Vec<Step>)>,
    /// The steps of ELSE, if written.
    pub(crate) otherwise: Option<Vec<Step>>,
    /// Where it is written.
    pub(crate) at: usize,
}

/// [§14.10 to §14.12] An iterated statement.
#[derive(Debug, Clone)]
pub(crate) struct Looped {
    /// The number of its label, explicit or implicit.
    pub(crate) label: usize,
    /// How it repeats.
    pub(crate) kind: Repeating,
    /// Its statements.
    pub(crate) statements: Vec<Step>,
}

/// How an iterated statement repeats.
#[derive(Debug, Clone)]
pub(crate) enum Repeating {
    /// LOOP: until it is left.
    Always,
    /// WHILE: while the condition is true, read before each time.
    While(Leaf),
    /// REPEAT: until the condition is true, read after each time.
    Until(Leaf),
}

/// [§14.13] A `<for statement>`: for each row of its cursor, its columns
/// assigned to its variables, its statements.
#[derive(Debug, Clone)]
pub(crate) struct ForEach {
    /// The number of its label, explicit or implicit, which ITERATE and
    /// LEAVE name.
    pub(crate) label: usize,
    /// The number of its cursor.
    pub(crate) cursor: usize,
    /// The slot of the variable of each column.
    pub(crate) slots: Vec<usize>,
    /// Its statements.
    pub(crate) statements: Vec<Step>,
}

/// [§17.2, §17.3] A `<signal statement>` or `<resignal statement>`.
#[derive(Debug, Clone)]
pub(crate) struct Signal {
    /// Whether it is RESIGNAL.
    pub(crate) resignal: bool,
    /// The SQLSTATE value of its `<signal value>`: its own, or its
    /// condition's; none of a condition declared for none, or of RESIGNAL
    /// of none.
    pub(crate) state: Option<SqlState>,
    /// The `<condition name>` of its `<signal value>`, if it is one.
    pub(crate) name: Option<String>,
    /// Each `<signal information item>`, and its value.
    pub(crate) items: Vec<(ConditionItem, Leaf)>,
    /// Where it is written.
    pub(crate) at: usize,
}

/// A `<select statement: single row>`, bound.
#[derive(Debug, Clone)]
pub(crate) struct Into {
    /// Its query.
    pub(crate) query: Leaf,
    /// Its targets.
    pub(crate) targets: Vec<ParameterSpecification>,
    /// The scope its targets are named in.
    pub(crate) scope: Arc<RoutineScope>,
}

/// [§12.1] A local cursor.
#[derive(Debug, Clone)]
pub(crate) struct Cursor {
    /// Its `<cursor name>`.
    pub(crate) name: String,
    /// The session's name of it, in this execution of the program: no
    /// `<cursor name>` (`binder/psm.rs`, `cursor_key`).
    pub(crate) key: String,
    /// Its query, as a leaf.
    pub(crate) query: Leaf,
    /// Its `<cursor specification>` as written, which the session declares.
    pub(crate) text: String,
    /// Whether it is INSENSITIVE.
    pub(crate) insensitive: bool,
    /// Whether it is SCROLL.
    pub(crate) scroll: bool,
    /// Whether it is read only though its text does not say so: a FOR
    /// statement's of no `<cursor name>`, whose body runs no statement
    /// that may write (`Step::writes`, RULES.md D613).
    pub(crate) read_only: bool,
    /// The scope of its specification's variables.
    pub(crate) scope: Arc<RoutineScope>,
}

/// [§12.2 to §12.4] OPEN, FETCH or CLOSE of a local cursor.
#[derive(Debug, Clone)]
pub(crate) struct CursorStep {
    /// The number of the cursor.
    pub(crate) cursor: usize,
    /// What it does.
    pub(crate) action: CursorAction,
}

/// What a cursor's statement does.
#[derive(Debug, Clone)]
pub(crate) enum CursorAction {
    /// OPEN.
    Open,
    /// FETCH of the orientation into the targets, named in the scope; and,
    /// if a value is assigned by an appropriate user-defined cast [ISO/IEC
    /// 9075-2:2023 §9.2 GR2], a VALUES of the values assigned, over the
    /// fetched row's values in its outer reference slots.
    Fetch(FetchOrientation, Vec<ParameterSpecification>, Arc<RoutineScope>, Option<Leaf>),
    /// CLOSE.
    Close,
}

/// [ISO/IEC 9075-2:2023 §16.1, §9.18] A CALL of the body of a function, or
/// of a procedure one invokes, bound with the program it is in.
#[derive(Debug, Clone)]
pub(crate) struct Called {
    /// The procedure's name, which a condition of its parameters carries
    /// [§23.1 GR4q].
    pub(crate) name: QualifiedName,
    /// Its specific name.
    pub(crate) specific: QualifiedName,
    /// Each parameter's name, mode and declared type.
    pub(crate) parameters: Vec<(String, ParameterMode, DataType)>,
    /// The value each parameter is given, a row of them, over the frame:
    /// its argument's, else the null value [§9.18 GR4c].
    pub(crate) inputs: Leaf,
    /// The procedure's body, bound.
    pub(crate) program: Arc<Program>,
    /// Each output parameter's ordinal position, from 0, and its target.
    pub(crate) targets: Vec<(usize, ParameterSpecification)>,
    /// The variables in scope where the CALL is, which its targets name.
    pub(crate) scope: Arc<RoutineScope>,
    /// [ISO/IEC 9075-2:2023 §9.18 GR7, §11.60 GR3k] Whether the procedure
    /// is of NEW SAVEPOINT LEVEL: run at a savepoint level of its own.
    pub(crate) new_level: bool,
}

/// A statement the session runs, and the scope it sees.
#[derive(Debug, Clone)]
pub(crate) struct Contained {
    /// The statement, kept behind the boundary `Syntax` draws.
    syntax: Arc<dyn Syntax>,
    /// The variables and cursors in scope where it is.
    pub(crate) scope: Arc<RoutineScope>,
}

/// \[DESIGN\] The parsed form of a statement a program keeps to run, seen
/// only through this trait. A program is part of every plan that invokes
/// a routine or a polymorphic table function, and the compiler proves a
/// type `Send` or `Sync` by walking each type its fields hold. Were the
/// statement held as it is, each such proof would walk the whole syntax
/// tree after the plan's own types, a chain longer than the compiler's
/// default recursion limit, so that every program using the library
/// would have to raise it. Behind a trait object the proof stops at the
/// object's `Send + Sync`, and the plan's proof no longer grows with the
/// syntax.
trait Syntax: std::fmt::Debug + Send + Sync {
    /// The statement.
    fn statement(&self) -> &SqlStatement;
}

impl Syntax for SqlStatement {
    fn statement(&self) -> &SqlStatement {
        self
    }
}

impl Contained {
    /// `statement`, run by the session in the scope `scope`.
    pub(crate) fn new(statement: SqlStatement, scope: Arc<RoutineScope>) -> Contained {
        Contained { syntax: Arc::new(statement), scope }
    }

    /// The statement.
    pub(crate) fn statement(&self) -> &SqlStatement {
        self.syntax.statement()
    }

    /// Where its statement is held: the same for each copy of it, so a
    /// compiled program's copy finds the plans the program keeps of it.
    pub(crate) fn address(&self) -> usize {
        Arc::as_ptr(&self.syntax).cast::<u8>().addr()
    }
}

impl Step {
    /// Whether it, or a step it holds at any depth, runs a statement of
    /// the session or a CALL, either of which may write SQL-data.
    pub(crate) fn writes(&self) -> bool {
        let mut work = vec![self];
        while let Some(step) = work.pop() {
            if matches!(step, Step::Statement(_) | Step::Call(_)) {
                return true;
            }
            work.extend(step.steps());
        }
        false
    }

    /// The steps it holds directly.
    pub(crate) fn steps(&self) -> Vec<&Step> {
        match self {
            Step::Block(b) => b.handlers.iter().map(|h| &h.action).chain(&b.statements).collect(),
            Step::If(b) | Step::Case(b) => b.arms.iter().flat_map(|(_, s)| s).chain(b.otherwise.iter().flatten()).collect(),
            Step::Loop(l) => l.statements.iter().collect(),
            Step::For(f) => f.statements.iter().collect(),
            Step::Assign(_)
            | Step::Leave(_)
            | Step::Iterate(_)
            | Step::Return(_)
            | Step::Signal(_)
            | Step::Into(_)
            | Step::Cursor(_)
            | Step::Statement(_)
            | Step::Call(_) => Vec::new(),
        }
    }

    /// The leaves it holds directly.
    pub(crate) fn leaves(&self) -> Vec<&Leaf> {
        match self {
            Step::Block(b) => b
                .variables
                .iter()
                .filter_map(|v| match &v.initial {
                    Initial::Value(l) => Some(l),
                    Initial::Null | Initial::Empty => None,
                })
                .collect(),
            Step::Assign(a) => a.source.iter().collect(),
            Step::If(b) | Step::Case(b) => b.arms.iter().map(|(l, _)| l).collect(),
            Step::Loop(l) => match &l.kind {
                Repeating::Always => Vec::new(),
                Repeating::While(c) | Repeating::Until(c) => vec![c],
            },
            Step::Return(l) => l.iter().collect(),
            Step::Signal(s) => s.items.iter().map(|(_, l)| l).collect(),
            Step::Into(i) => vec![&i.query],
            Step::Call(c) => vec![&c.inputs],
            Step::For(_) | Step::Cursor(_) | Step::Leave(_) | Step::Iterate(_) | Step::Statement(_) => Vec::new(),
        }
    }
}

impl Program {
    /// Every leaf of it, its cursors' queries and those of the procedures
    /// it calls among them: a worklist, not recursion, so no depth bounds
    /// it.
    pub(crate) fn all_leaves(&self) -> Vec<&Leaf> {
        let mut leaves: Vec<&Leaf> = Vec::new();
        let mut programs = vec![self];
        while let Some(program) = programs.pop() {
            leaves.extend(program.cursors.iter().map(|c| &c.query));
            let mut work = vec![&program.body];
            while let Some(step) = work.pop() {
                leaves.extend(step.leaves());
                if let Step::Call(c) = step {
                    programs.push(&c.program);
                }
                work.extend(step.steps());
            }
        }
        leaves
    }

    /// Every statement the session runs of it.
    pub(crate) fn contained(&self) -> Vec<&Contained> {
        let mut found = Vec::new();
        let mut work = vec![&self.body];
        while let Some(step) = work.pop() {
            match step {
                Step::Statement(c) => found.push(&**c),
                Step::Block(_)
                | Step::Cursor(_)
                | Step::For(_)
                | Step::Assign(_)
                | Step::If(_)
                | Step::Case(_)
                | Step::Loop(_)
                | Step::Leave(_)
                | Step::Iterate(_)
                | Step::Return(_)
                | Step::Signal(_)
                | Step::Into(_)
                | Step::Call(_) => {}
            }
            work.extend(step.steps());
        }
        found
    }
}
