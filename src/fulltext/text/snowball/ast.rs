//! \[DESIGN\] A Snowball program parsed (RULES.md D320): its commands,
//! arithmetic expressions, groupings and amongs, every name resolved to
//! the place of what it names. A command's direction is not kept, as the
//! machine knows it as it runs (`run.rs`): a routine runs only in the
//! direction it was defined for, so it is the direction of its caller.

use std::collections::HashMap;

/// An integer operator of an arithmetic expression.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Arith {
    Add,
    Subtract,
    Multiply,
    Divide,
}

/// An arithmetic expression (`AE`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Ae {
    Number(i64),
    /// An integer variable, by its place.
    Integer(usize),
    Cursor,
    /// The limit of the direction run in.
    Limit,
    /// The length of the string, in characters, as `size` is too.
    Len,
    /// The length of a string variable, in characters, as `sizeof` is too.
    LenOf(usize),
    Maxint,
    Minint,
    Negate(Box<Ae>),
    Binary(Box<Ae>, Arith, Box<Ae>),
}

/// A comparison of integers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Relation {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// An assignment to an integer variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Assignment {
    Set,
    Add,
    Subtract,
    Multiply,
    Divide,
}

/// A string a command takes: a literal, or a string variable's value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Text {
    Literal(Vec<char>),
    Variable(usize),
}

/// A command (`C`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Command {
    /// Commands in turn, `( C1 C2 ... )`.
    Sequence(Vec<Command>),
    Or(Vec<Command>),
    And(Vec<Command>),
    Not(Box<Command>),
    Try(Box<Command>),
    Test(Box<Command>),
    Do(Box<Command>),
    Fail(Box<Command>),
    Goto(Box<Command>),
    Gopast(Box<Command>),
    Repeat(Box<Command>),
    Loop(Ae, Box<Command>),
    Atleast(Ae, Box<Command>),
    Backwards(Box<Command>),
    /// `setlimit C1 for C2`.
    Setlimit(Box<Command>, Box<Command>),
    Hop(Ae),
    Next,
    Tomark(Ae),
    Tolimit,
    Atlimit,
    /// `[`.
    LeftSlice,
    /// `]`.
    RightSlice,
    /// `<- S`, `delete` too.
    SliceFrom(Text),
    /// `-> s`.
    SliceTo(usize),
    /// `=> s`.
    AssignTo(usize),
    Insert(Text),
    Attach(Text),
    /// `= S`.
    Assign(Text),
    /// A literal string or string variable as a test.
    Match(Text),
    /// A grouping by its place, or `non` of it.
    Grouping {
        grouping: usize,
        non: bool,
    },
    /// A routine by its place.
    Call(usize),
    Set(usize),
    Unset(usize),
    /// A boolean variable as a test.
    BooleanTest(usize),
    /// `$x = AE` and the like, `setmark x` too.
    IntegerAssign(usize, Assignment, Ae),
    /// `$(AE1 op AE2)` and `$x op AE`, `atmark AE` too.
    IntegerTest(Ae, Relation, Ae),
    /// `$s C`.
    OnString(usize, Box<Command>),
    /// `substring`, of the among by its place.
    Substring(usize),
    /// `among`, by its place, which matched already at its `substring` if
    /// it has one.
    Among {
        among: usize,
        substring: bool,
    },
    True,
    False,
}

/// A string of an among.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Entry {
    /// The routine that must succeed after it, if any.
    pub routine: Option<usize>,
    /// The place of its command in `Among::actions`, if it has one.
    pub action: Option<usize>,
}

/// An among: its strings, and the commands they choose.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(super) struct Among {
    pub entries: Vec<Entry>,
    /// Each string's place in `entries`.
    pub index: HashMap<Vec<char>, usize>,
    /// The lengths of its strings, longest first, each once.
    pub lengths: Vec<usize>,
    pub actions: Vec<Command>,
}

/// A routine: its body and whether it was defined backward.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Routine {
    pub body: Command,
    pub backward: bool,
}

/// A Snowball program, parsed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Program {
    pub routines: Vec<Routine>,
    /// Each grouping's characters, sorted.
    pub groupings: Vec<Vec<char>>,
    pub amongs: Vec<Among>,
    pub integers: usize,
    pub booleans: usize,
    pub strings: usize,
    /// The place of the external `stem`.
    pub stem: usize,
}
