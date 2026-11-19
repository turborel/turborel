//! \[DESIGN\] A Snowball command (`C`), as the analyser's `read_C` reads
//! it (RULES.md D320). `or` and `and` join commands only inside
//! parentheses, of one precedence and from the left, so `a or b and c` is
//! `(a or b) and c`. `reverse` and `get` are not read, as no shipped
//! program uses them.

use super::{Kind, Parser};
use crate::error::Result;
use crate::fulltext::text::snowball::ast::{Ae, Assignment, Command, Relation, Text};
use crate::fulltext::text::snowball::damaged;
use crate::fulltext::text::snowball::lex::{Key, Sym, Token};

/// How a key word begins a command.
type Reader = fn(&mut Parser) -> Result<Command>;

/// The key words that begin a command, and how each is read.
const KEY_COMMANDS: [(Key, Reader); 31] = [
    (Key::Not, |p| Ok(Command::Not(p.boxed()?))),
    (Key::Try, |p| Ok(Command::Try(p.boxed()?))),
    (Key::Test, |p| Ok(Command::Test(p.boxed()?))),
    (Key::Do, |p| Ok(Command::Do(p.boxed()?))),
    (Key::Fail, |p| Ok(Command::Fail(p.boxed()?))),
    (Key::Goto, |p| Ok(Command::Goto(p.boxed()?))),
    (Key::Gopast, |p| Ok(Command::Gopast(p.boxed()?))),
    (Key::Repeat, |p| Ok(Command::Repeat(p.boxed()?))),
    (Key::Backwards, |p| Ok(Command::Backwards(p.boxed()?))),
    (Key::Loop, |p| Ok(Command::Loop(p.ae()?, p.boxed()?))),
    (Key::Atleast, |p| Ok(Command::Atleast(p.ae()?, p.boxed()?))),
    (Key::Setlimit, Parser::setlimit),
    (Key::Setmark, |p| Ok(Command::IntegerAssign(p.variable(Kind::Integer)?, Assignment::Set, Ae::Cursor))),
    (Key::Atmark, |p| Ok(Command::IntegerTest(Ae::Cursor, Relation::Eq, p.ae()?))),
    (Key::Tomark, |p| Ok(Command::Tomark(p.ae()?))),
    (Key::Hop, |p| Ok(Command::Hop(p.ae()?))),
    (Key::Delete, |_| Ok(Command::SliceFrom(Text::Literal(Vec::new())))),
    (Key::Next, |_| Ok(Command::Next)),
    (Key::Tolimit, |_| Ok(Command::Tolimit)),
    (Key::Atlimit, |_| Ok(Command::Atlimit)),
    (Key::True, |_| Ok(Command::True)),
    (Key::False, |_| Ok(Command::False)),
    (Key::Insert, |p| Ok(Command::Insert(p.text()?))),
    (Key::Attach, |p| Ok(Command::Attach(p.text()?))),
    (Key::Set, |p| Ok(Command::Set(p.variable(Kind::Boolean)?))),
    (Key::Unset, |p| Ok(Command::Unset(p.variable(Kind::Boolean)?))),
    (Key::Non, Parser::non),
    (Key::Among, Parser::among),
    (Key::Substring, Parser::substring),
    (Key::Or, |p| Err(damaged(p.line(), "or joins commands only in parentheses"))),
    (Key::And, |p| Err(damaged(p.line(), "and joins commands only in parentheses"))),
];

/// The assignments of `$x op AE`, by their symbols.
const ASSIGNMENTS: [(Sym, Assignment); 5] = [
    (Sym::Assign, Assignment::Set),
    (Sym::PlusAssign, Assignment::Add),
    (Sym::MinusAssign, Assignment::Subtract),
    (Sym::MultiplyAssign, Assignment::Multiply),
    (Sym::DivideAssign, Assignment::Divide),
];

/// The comparisons, by their symbols.
const RELATIONS: [(Sym, Relation); 6] =
    [(Sym::Eq, Relation::Eq), (Sym::Ne, Relation::Ne), (Sym::Lt, Relation::Lt), (Sym::Le, Relation::Le), (Sym::Gt, Relation::Gt), (Sym::Ge, Relation::Ge)];

impl Parser {
    /// A command.
    ///
    /// # Errors
    ///
    /// SQLCODE -916 if none is next.
    pub(super) fn command(&mut self) -> Result<Command> {
        self.enter()?;
        let command = match self.advance() {
            Some(Token::Key(key)) => self.key_command(key),
            Some(Token::Sym(sym)) => self.sym_command(sym),
            Some(Token::Name(name)) => self.name_command(&name),
            Some(Token::Text(text)) => Ok(Command::Match(Text::Literal(text))),
            Some(Token::Number(_)) | None => Err(damaged(self.line(), "expected a command")),
        };
        self.leave();
        command
    }

    /// A command, boxed.
    fn boxed(&mut self) -> Result<Box<Command>> {
        self.command().map(Box::new)
    }

    /// The command the key word `key` begins.
    fn key_command(&mut self, key: Key) -> Result<Command> {
        match KEY_COMMANDS.iter().find(|(begins, _)| *begins == key) {
            Some((_, read)) => read(self),
            None => Err(damaged(self.line(), &format!("{key:?} begins no command read here"))),
        }
    }

    /// The command the symbol `sym` begins.
    fn sym_command(&mut self, sym: Sym) -> Result<Command> {
        match sym {
            Sym::Bra => self.list(),
            Sym::Dollar => self.dollar(),
            Sym::LeftSlice => Ok(Command::LeftSlice),
            Sym::RightSlice => Ok(Command::RightSlice),
            Sym::Debug => Ok(Command::True),
            Sym::SliceTo => Ok(Command::SliceTo(self.variable(Kind::String)?)),
            Sym::AssignTo => Ok(Command::AssignTo(self.variable(Kind::String)?)),
            Sym::Assign => Ok(Command::Assign(self.text()?)),
            Sym::InsertSym => Ok(Command::Insert(self.text()?)),
            Sym::SliceFrom => Ok(Command::SliceFrom(self.text()?)),
            Sym::Ket
            | Sym::Multiply
            | Sym::Plus
            | Sym::Minus
            | Sym::Divide
            | Sym::Lt
            | Sym::Gt
            | Sym::Ne
            | Sym::MultiplyAssign
            | Sym::PlusAssign
            | Sym::MinusAssign
            | Sym::DivideAssign
            | Sym::Le
            | Sym::Eq
            | Sym::Ge => Err(damaged(self.line(), &format!("{sym:?} begins no command"))),
        }
    }

    /// The command a name is: a boolean's test, a routine's call, a
    /// grouping's test or a string variable's match.
    fn name_command(&mut self, name: &str) -> Result<Command> {
        let Some((kind, place)) = self.names.get(name).copied() else {
            return Err(damaged(self.line(), &format!("{name} is not declared")));
        };
        match kind {
            Kind::Boolean => Ok(Command::BooleanTest(place)),
            Kind::Routine => Ok(Command::Call(place)),
            Kind::Grouping => Ok(Command::Grouping { grouping: place, non: false }),
            Kind::String => Ok(Command::Match(Text::Variable(place))),
            Kind::Integer => Err(damaged(self.line(), &format!("the integer {name} is no command"))),
        }
    }

    /// The place of the variable of `kind` named next.
    fn variable(&mut self, kind: Kind) -> Result<usize> {
        let name = self.expect_name()?;
        self.resolve(&name, kind)
    }

    /// A string a command takes: a literal, or a string variable.
    pub(super) fn text(&mut self) -> Result<Text> {
        match self.advance() {
            Some(Token::Text(text)) => Ok(Text::Literal(text)),
            Some(Token::Name(name)) => Ok(Text::Variable(self.resolve(&name, Kind::String)?)),
            Some(Token::Key(_) | Token::Number(_) | Token::Sym(_)) | None => Err(damaged(self.line(), "expected a string")),
        }
    }

    /// A list of commands, its `(` taken, to its `)`: one command alone,
    /// else their sequence.
    fn list(&mut self) -> Result<Command> {
        let mut commands = Vec::new();
        while !self.accept_sym(Sym::Ket) {
            let mut command = self.command()?;
            loop {
                if self.accept_key(Key::Or) {
                    command = self.chain(command, Key::Or)?;
                } else if self.accept_key(Key::And) {
                    command = self.chain(command, Key::And)?;
                } else {
                    break;
                }
            }
            commands.push(command);
        }
        if commands.len() == 1 {
            return Ok(commands.pop().unwrap_or(Command::True));
        }
        Ok(Command::Sequence(commands))
    }

    /// `first` joined by `key`, `or` or `and`, its first `key` taken, to the
    /// many commands it joins.
    fn chain(&mut self, first: Command, key: Key) -> Result<Command> {
        let mut commands = vec![first];
        loop {
            commands.push(self.command()?);
            if !self.accept_key(key) {
                break;
            }
        }
        Ok(if key == Key::Or { Command::Or(commands) } else { Command::And(commands) })
    }

    /// `setlimit C1 for C2`.
    fn setlimit(&mut self) -> Result<Command> {
        let limit = self.boxed()?;
        if !self.accept_key(Key::For) {
            return Err(damaged(self.line(), "setlimit is followed by for"));
        }
        Ok(Command::Setlimit(limit, self.boxed()?))
    }

    /// `non G`, `non-G` too.
    fn non(&mut self) -> Result<Command> {
        self.accept_sym(Sym::Minus);
        Ok(Command::Grouping { grouping: self.variable(Kind::Grouping)?, non: true })
    }

    /// `substring`, its among to come.
    fn substring(&mut self) -> Result<Command> {
        if self.substring.is_some() {
            return Err(damaged(self.line(), "a substring has no among"));
        }
        let among = self.new_among();
        self.substring = Some(among);
        Ok(Command::Substring(among))
    }

    /// `$`: `$(AE1 op AE2)`, `$s C` of a string variable, or `$x op AE` of
    /// an integer one.
    fn dollar(&mut self) -> Result<Command> {
        if self.accept_sym(Sym::Bra) {
            let left = self.ae()?;
            let relation = self.relation()?;
            let right = self.ae()?;
            self.expect_sym(Sym::Ket)?;
            return Ok(Command::IntegerTest(left, relation, right));
        }
        let name = self.expect_name()?;
        if let Ok(string) = self.resolve(&name, Kind::String) {
            return Ok(Command::OnString(string, self.boxed()?));
        }
        let integer = self.resolve(&name, Kind::Integer)?;
        if let Some(assignment) = self.assignment() {
            return Ok(Command::IntegerAssign(integer, assignment, self.ae()?));
        }
        let relation = self.relation()?;
        Ok(Command::IntegerTest(Ae::Integer(integer), relation, self.ae()?))
    }

    /// The assignment next, if one is, taken.
    fn assignment(&mut self) -> Option<Assignment> {
        let Some(Token::Sym(sym)) = self.peek() else { return None };
        let found = ASSIGNMENTS.iter().find(|(written, _)| written == sym).map(|(_, assignment)| *assignment)?;
        self.next = self.next.saturating_add(1);
        Some(found)
    }

    /// The comparison next, taken. A `<-` there is `<` and then a minus,
    /// as the analyser reads `$x<-1`.
    fn relation(&mut self) -> Result<Relation> {
        let line = self.line();
        let Some(Token::Sym(sym)) = self.advance() else { return Err(damaged(line, "expected a comparison")) };
        if sym == Sym::SliceFrom {
            self.split_slice_from();
            return Ok(Relation::Lt);
        }
        RELATIONS.iter().find(|(written, _)| *written == sym).map(|(_, relation)| *relation).ok_or_else(|| damaged(line, "expected a comparison"))
    }

    /// The `<-` just taken, read as `<` then `-`: its minus put back.
    fn split_slice_from(&mut self) {
        self.next = self.next.saturating_sub(1);
        if let Some(lexed) = self.tokens.get_mut(self.next) {
            lexed.token = Token::Sym(Sym::Minus);
        }
    }
}
