//! \[DESIGN\] Running a Snowball program on a word (RULES.md D320), by the
//! semantics of Snowball's manual (snowballstem.org, `compiler/
//! snowman.html`) as its generated code and runtime carry them out
//! (`compiler/generator_python.c`, `python/snowballstemmer/
//! basestemmer.py`): each command signals success or failure, and the
//! commands that try something put the cursor back where they found it.
//!
//! The string is held as characters, so the cursor, the limits and the
//! slice are places between characters, and `size` and `sizeof` count
//! characters as `len` and `lenof` do. Running backward, a saved cursor is
//! kept as its distance from the limit, so it stays right as the string
//! before it changes. Variables start each word as zero, false and empty.
//!
//! A run is stopped, and the word left unstemmed, when it nests deeper
//! than `MAX_DEPTH`, takes more than `MAX_STEPS` commands, or does what
//! Snowball calls an error, as a slice out of order or an integer
//! overflow; the shipped programs do none of these.

mod among;
mod edit;

use super::ast::{Ae, Arith, Assignment, Command, Program, Relation};

/// \[DESIGN\] The deepest commands and routine calls nest while running.
/// The shipped programs nest a few tens deep.
const MAX_DEPTH: usize = 400;

/// \[DESIGN\] The most commands one word runs. The shipped programs run a
/// few thousand on the longest words.
const MAX_STEPS: u64 = 1 << 22;

/// Why a run stopped before its end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Halt;

/// A command's signal: success or failure, or a stop.
type Signal = Result<bool, Halt>;

/// The string a command works on and its places.
#[derive(Debug, Clone, Default)]
struct Env {
    s: Vec<char>,
    /// The cursor.
    c: usize,
    /// The forward limit.
    l: usize,
    /// The backward limit.
    lb: usize,
    /// The slice's start.
    bra: usize,
    /// The slice's end.
    ket: usize,
}

/// What the string of an among found chooses.
#[derive(Debug, Clone, Copy)]
enum Chosen {
    /// The among's command of this place.
    Command(usize),
    /// No command; the among succeeds.
    Nothing,
}

/// A program running on a word.
struct Machine<'p> {
    program: &'p Program,
    env: Env,
    /// Whether commands run backward.
    backward: bool,
    integers: Vec<i64>,
    booleans: Vec<bool>,
    strings: Vec<Vec<char>>,
    /// What each among's `substring` matched, if it matched.
    matched: Vec<Option<Chosen>>,
    depth: usize,
    steps: u64,
}

/// The stem of `word` by `program`: the word as its routine `stem`
/// leaves it, or the word itself if the run stops.
pub(super) fn stem(program: &Program, word: &str) -> String {
    let s: Vec<char> = word.chars().collect();
    let l = s.len();
    let mut machine = Machine {
        program,
        env: Env { s, c: 0, l, lb: 0, bra: 0, ket: l },
        backward: false,
        integers: vec![0; program.integers],
        booleans: vec![false; program.booleans],
        strings: vec![Vec::new(); program.strings],
        matched: vec![None; program.amongs.len()],
        depth: 0,
        steps: 0,
    };
    match machine.call(program.stem) {
        Ok(_) => machine.env.s.into_iter().collect(),
        Err(Halt) => word.to_owned(),
    }
}

impl<'p> Machine<'p> {
    /// `command` run, its depth and steps counted.
    fn run(&mut self, command: &'p Command) -> Signal {
        self.depth = self.depth.saturating_add(1);
        self.steps = self.steps.saturating_add(1);
        if self.depth > MAX_DEPTH || self.steps > MAX_STEPS {
            return Err(Halt);
        }
        let signal = self.dispatch(command);
        self.depth = self.depth.saturating_sub(1);
        signal
    }

    /// `command` run.
    fn dispatch(&mut self, command: &'p Command) -> Signal {
        match command {
            Command::Sequence(commands) => self.sequence(commands),
            Command::Or(commands) => self.or(commands),
            Command::And(commands) => self.and(commands),
            Command::Not(inner) => self.not(inner),
            Command::Try(inner) => self.attempt(inner),
            Command::Test(inner) => self.test(inner),
            Command::Do(inner) => self.run_do(inner),
            Command::Fail(inner) => self.run(inner).map(|_| false),
            Command::Goto(inner) => self.go(inner, true),
            Command::Gopast(inner) => self.go(inner, false),
            Command::Repeat(inner) => self.repeat(inner),
            Command::Loop(count, inner) => self.run_loop(count, inner),
            Command::Atleast(count, inner) => self.atleast(count, inner),
            Command::Backwards(inner) => self.backwards(inner),
            Command::Setlimit(limit, inner) => self.setlimit(limit, inner),
            Command::Hop(count) => self.hop(count),
            Command::Next => Ok(self.step()),
            Command::Tomark(at) => self.tomark(at),
            Command::Tolimit => Ok(self.tolimit()),
            Command::Atlimit => Ok(self.env.c == self.limit()),
            Command::LeftSlice => Ok(self.slice_end(false)),
            Command::RightSlice => Ok(self.slice_end(true)),
            Command::SliceFrom(text) => self.slice_from(text),
            Command::SliceTo(variable) => self.slice_to(*variable),
            Command::AssignTo(variable) => self.assign_to(*variable),
            Command::Insert(text) => self.insert(text, self.backward),
            Command::Attach(text) => self.insert(text, !self.backward),
            Command::Assign(text) => self.assign(text),
            Command::Match(text) => Ok(self.matches(text)),
            Command::Grouping { grouping, non } => Ok(self.grouping(*grouping, *non)),
            Command::Call(routine) => self.call(*routine),
            Command::Set(variable) => Ok(self.set(*variable, true)),
            Command::Unset(variable) => Ok(self.set(*variable, false)),
            Command::BooleanTest(variable) => Ok(self.booleans.get(*variable).copied().unwrap_or(false)),
            Command::IntegerAssign(variable, assignment, value) => self.integer_assign(*variable, *assignment, value),
            Command::IntegerTest(left, relation, right) => self.integer_test(left, *relation, right),
            Command::OnString(variable, inner) => self.on_string(*variable, inner),
            Command::Substring(among) => self.substring(*among),
            Command::Among { among, substring } => self.among(*among, *substring),
            Command::True => Ok(true),
            Command::False => Ok(false),
        }
    }

    /// The cursor, saved: as its distance from the limit, running backward.
    fn save(&self) -> usize {
        if self.backward { self.env.l.saturating_sub(self.env.c) } else { self.env.c }
    }

    /// The cursor `saved` put back.
    fn restore(&mut self, saved: usize) {
        self.env.c = if self.backward { self.env.l.saturating_sub(saved) } else { saved };
    }

    /// The limit of the direction run in.
    fn limit(&self) -> usize {
        if self.backward { self.env.lb } else { self.env.l }
    }

    /// Commands in turn, failing at the first that fails.
    fn sequence(&mut self, commands: &'p [Command]) -> Signal {
        for command in commands {
            if !self.run(command)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `or`: each command from the cursor until one succeeds.
    fn or(&mut self, commands: &'p [Command]) -> Signal {
        let saved = self.save();
        for command in commands {
            self.restore(saved);
            if self.run(command)? {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// `and`: each command from the cursor, all succeeding.
    fn and(&mut self, commands: &'p [Command]) -> Signal {
        let saved = self.save();
        for (n, command) in commands.iter().enumerate() {
            if n > 0 {
                self.restore(saved);
            }
            if !self.run(command)? {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// `not C`: success if C fails, the cursor put back.
    fn not(&mut self, inner: &'p Command) -> Signal {
        let saved = self.save();
        if self.run(inner)? {
            return Ok(false);
        }
        self.restore(saved);
        Ok(true)
    }

    /// `try C`: success, the cursor put back if C fails.
    fn attempt(&mut self, inner: &'p Command) -> Signal {
        let saved = self.save();
        if !self.run(inner)? {
            self.restore(saved);
        }
        Ok(true)
    }

    /// `test C`: C's signal, the cursor put back if it succeeds.
    fn test(&mut self, inner: &'p Command) -> Signal {
        let saved = self.save();
        if !self.run(inner)? {
            return Ok(false);
        }
        self.restore(saved);
        Ok(true)
    }

    /// `do C`: success, the cursor put back.
    fn run_do(&mut self, inner: &'p Command) -> Signal {
        let saved = self.save();
        self.run(inner)?;
        self.restore(saved);
        Ok(true)
    }

    /// `goto C` and `gopast C`: C tried at each place until it succeeds,
    /// the cursor left before it, for `goto`, or after it.
    fn go(&mut self, inner: &'p Command, before: bool) -> Signal {
        loop {
            let saved = self.save();
            if self.run(inner)? {
                if before {
                    self.restore(saved);
                }
                return Ok(true);
            }
            self.restore(saved);
            if !self.step() {
                return Ok(false);
            }
        }
    }

    /// `repeat C`: C until it fails, the cursor put back from that try.
    fn repeat(&mut self, inner: &'p Command) -> Signal {
        loop {
            let saved = self.save();
            if !self.run(inner)? {
                self.restore(saved);
                return Ok(true);
            }
        }
    }

    /// `loop AE C`: C as many times as AE, each succeeding.
    fn run_loop(&mut self, count: &Ae, inner: &'p Command) -> Signal {
        let mut left = self.eval(count)?;
        while left > 0 {
            if !self.run(inner)? {
                return Ok(false);
            }
            left = left.saturating_sub(1);
        }
        Ok(true)
    }

    /// `atleast AE C`: `repeat C`, succeeding if C did at least AE times.
    fn atleast(&mut self, count: &Ae, inner: &'p Command) -> Signal {
        let mut left = self.eval(count)?;
        loop {
            let saved = self.save();
            if !self.run(inner)? {
                self.restore(saved);
                return Ok(left <= 0);
            }
            left = left.saturating_sub(1);
        }
    }

    /// `backwards C`: C run backward from the limit, the backward limit
    /// the cursor, which C leaves where it found it if it succeeds.
    fn backwards(&mut self, inner: &'p Command) -> Signal {
        let forward = !self.backward;
        self.env.lb = self.env.c;
        self.env.c = self.env.l;
        self.backward = true;
        let signal = self.run(inner);
        self.backward = !forward;
        if signal? {
            self.env.c = self.env.lb;
            return Ok(true);
        }
        Ok(false)
    }

    /// `setlimit C1 for C2`: C2 with the limit where C1 leaves the cursor,
    /// from the cursor where it was.
    fn setlimit(&mut self, limit: &'p Command, inner: &'p Command) -> Signal {
        let saved = self.save();
        if !self.run(limit)? {
            return Ok(false);
        }
        if self.backward {
            let kept = std::mem::replace(&mut self.env.lb, self.env.c);
            self.restore(saved);
            let signal = self.run(inner)?;
            self.env.lb = kept;
            return Ok(signal);
        }
        let beyond = self.env.l.checked_sub(self.env.c).ok_or(Halt)?;
        self.env.l = self.env.c;
        self.restore(saved);
        let signal = self.run(inner)?;
        self.env.l = self.env.l.checked_add(beyond).ok_or(Halt)?;
        Ok(signal)
    }

    /// `next`: the cursor a character on, failing at the limit.
    fn step(&mut self) -> bool {
        if self.backward {
            if self.env.c <= self.env.lb {
                return false;
            }
            self.env.c = self.env.c.saturating_sub(1);
        } else {
            if self.env.c >= self.env.l {
                return false;
            }
            self.env.c = self.env.c.saturating_add(1);
        }
        true
    }

    /// `hop AE`: the cursor AE characters on, failing past the limit or
    /// for less than none.
    fn hop(&mut self, count: &Ae) -> Signal {
        let Ok(count) = usize::try_from(self.eval(count)?) else { return Ok(false) };
        let to = if self.backward {
            self.env.c.checked_sub(count).filter(|to| *to >= self.env.lb)
        } else {
            self.env.c.checked_add(count).filter(|to| *to <= self.env.l)
        };
        Ok(to.is_some_and(|to| {
            self.env.c = to;
            true
        }))
    }

    /// `tomark AE`: the cursor to AE, failing if it is past it already.
    fn tomark(&mut self, at: &Ae) -> Signal {
        let Ok(at) = usize::try_from(self.eval(at)?) else { return Ok(false) };
        let behind = if self.backward { self.env.c < at } else { self.env.c > at };
        if behind || at > self.env.s.len() {
            return Ok(false);
        }
        self.env.c = at;
        Ok(true)
    }

    /// `tolimit`: the cursor to the limit.
    fn tolimit(&mut self) -> bool {
        self.env.c = self.limit();
        true
    }

    /// `[`, or `]` if `right`: the slice's start, or end, at the cursor;
    /// the other end of it, running backward.
    fn slice_end(&mut self, right: bool) -> bool {
        if right == self.backward {
            self.env.bra = self.env.c;
        } else {
            self.env.ket = self.env.c;
        }
        true
    }

    /// `set b` or `unset b`.
    fn set(&mut self, variable: usize, value: bool) -> bool {
        if let Some(slot) = self.booleans.get_mut(variable) {
            *slot = value;
        }
        true
    }

    /// A routine run, in the direction it was defined for.
    fn call(&mut self, routine: usize) -> Signal {
        let program = self.program;
        let routine = program.routines.get(routine).ok_or(Halt)?;
        let direction = std::mem::replace(&mut self.backward, routine.backward);
        let signal = self.run(&routine.body);
        self.backward = direction;
        signal
    }

    /// The value of `ae`.
    fn eval(&self, ae: &Ae) -> Result<i64, Halt> {
        let place = |n: usize| i64::try_from(n).map_err(|_| Halt);
        match ae {
            Ae::Number(n) => Ok(*n),
            Ae::Integer(variable) => self.integers.get(*variable).copied().ok_or(Halt),
            Ae::Cursor => place(self.env.c),
            Ae::Limit => place(self.limit()),
            Ae::Len => place(self.env.s.len()),
            Ae::LenOf(variable) => place(self.strings.get(*variable).map_or(0, Vec::len)),
            Ae::Maxint => Ok(i64::MAX),
            Ae::Minint => Ok(i64::MIN),
            Ae::Negate(inner) => self.eval(inner)?.checked_neg().ok_or(Halt),
            Ae::Binary(left, op, right) => arith(self.eval(left)?, *op, self.eval(right)?),
        }
    }

    /// `$x op AE`.
    fn integer_assign(&mut self, variable: usize, assignment: Assignment, value: &Ae) -> Signal {
        let value = self.eval(value)?;
        let slot = self.integers.get_mut(variable).ok_or(Halt)?;
        *slot = match assignment {
            Assignment::Set => value,
            Assignment::Add => arith(*slot, Arith::Add, value)?,
            Assignment::Subtract => arith(*slot, Arith::Subtract, value)?,
            Assignment::Multiply => arith(*slot, Arith::Multiply, value)?,
            Assignment::Divide => arith(*slot, Arith::Divide, value)?,
        };
        Ok(true)
    }

    /// `$(AE1 op AE2)`.
    fn integer_test(&self, left: &Ae, relation: Relation, right: &Ae) -> Signal {
        let (left, right) = (self.eval(left)?, self.eval(right)?);
        Ok(match relation {
            Relation::Eq => left == right,
            Relation::Ne => left != right,
            Relation::Lt => left < right,
            Relation::Le => left <= right,
            Relation::Gt => left > right,
            Relation::Ge => left >= right,
        })
    }

    /// `$s C`: C run forward on the string variable `s`, which keeps
    /// what C leaves of it; the string and its places put back after.
    fn on_string(&mut self, variable: usize, inner: &'p Command) -> Signal {
        let string = std::mem::take(self.strings.get_mut(variable).ok_or(Halt)?);
        let l = string.len();
        let env = Env { s: string, c: 0, l, lb: self.env.lb, bra: self.env.bra, ket: self.env.ket };
        let outer = std::mem::replace(&mut self.env, env);
        let direction = std::mem::replace(&mut self.backward, false);
        let signal = self.run(inner);
        self.backward = direction;
        let inner_env = std::mem::replace(&mut self.env, outer);
        if let Some(slot) = self.strings.get_mut(variable) {
            *slot = inner_env.s;
        }
        signal
    }
}

/// `left op right`, as C computes it: division truncated toward zero.
fn arith(left: i64, op: Arith, right: i64) -> Result<i64, Halt> {
    match op {
        Arith::Add => left.checked_add(right),
        Arith::Subtract => left.checked_sub(right),
        Arith::Multiply => left.checked_mul(right),
        Arith::Divide => left.checked_div(right),
    }
    .ok_or(Halt)
}
