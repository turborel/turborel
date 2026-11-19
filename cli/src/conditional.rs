//! \[DESIGN\] Conditional blocks (RULES.md D212): `\if`, `\elif`, `\else`
//! and `\endif`, nested, on the truth of their argument once variables are
//! read into it. In a branch not taken, lines are passed over unread but
//! for these four commands, so blocks nested in it still pair up.

/// The most blocks open at once (RULES.md R-CF-6).
pub const MAX_DEPTH: usize = 64;

/// Where one open block stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Branch {
    /// Its branch under way is taken.
    Taken,
    /// No branch is taken yet; a later `\elif` or `\else` may be.
    Waiting,
    /// A branch was taken; none after is.
    Done,
    /// The block is within a branch not taken: none of it is.
    Skipped,
}

/// Whether the block's `\else` was read, so neither `\elif` nor `\else`
/// may follow.
#[derive(Debug, Clone, Copy)]
struct Block {
    branch: Branch,
    in_else: bool,
}

/// The blocks open, outermost first.
#[derive(Debug, Default)]
pub struct Conditions {
    open: Vec<Block>,
}

impl Conditions {
    /// Whether lines are read: no block open, or the innermost's branch
    /// taken.
    pub fn active(&self) -> bool {
        self.open.last().is_none_or(|b| b.branch == Branch::Taken)
    }

    /// Whether any block is open, as the end of a file must find none.
    pub fn is_open(&self) -> bool {
        !self.open.is_empty()
    }

    /// `\if`, of the truth `value`, unread if the branch it is in is not
    /// taken.
    ///
    /// # Errors
    ///
    /// Past `MAX_DEPTH` blocks open.
    pub fn begin(&mut self, value: bool) -> Result<(), String> {
        if self.open.len() >= MAX_DEPTH {
            return Err(format!("\\if: at most {MAX_DEPTH} blocks are open at once"));
        }
        let branch = match (self.active(), value) {
            (true, true) => Branch::Taken,
            (true, false) => Branch::Waiting,
            (false, true) | (false, false) => Branch::Skipped,
        };
        self.open.push(Block { branch, in_else: false });
        Ok(())
    }

    /// `\elif`, of the truth `value`: taken if no branch before was.
    ///
    /// # Errors
    ///
    /// No block is open, or its `\else` was read.
    pub fn elif(&mut self, value: bool) -> Result<(), String> {
        let block = self.innermost("\\elif")?;
        block.branch = match (block.branch, value) {
            (Branch::Waiting, true) => Branch::Taken,
            (Branch::Waiting, false) => Branch::Waiting,
            (Branch::Taken | Branch::Done, _) => Branch::Done,
            (Branch::Skipped, _) => Branch::Skipped,
        };
        Ok(())
    }

    /// `\else`: taken if no branch before was.
    ///
    /// # Errors
    ///
    /// No block is open, or its `\else` was read.
    pub fn otherwise(&mut self) -> Result<(), String> {
        let block = self.innermost("\\else")?;
        block.in_else = true;
        block.branch = match block.branch {
            Branch::Waiting => Branch::Taken,
            Branch::Taken | Branch::Done => Branch::Done,
            Branch::Skipped => Branch::Skipped,
        };
        Ok(())
    }

    /// `\endif`: the innermost block closed.
    ///
    /// # Errors
    ///
    /// No block is open.
    pub fn end(&mut self) -> Result<(), String> {
        self.open.pop().map(drop).ok_or_else(|| "\\endif: no \\if is open".to_owned())
    }

    /// The innermost block, for `command`, which its `\else` must not
    /// have been read before.
    fn innermost(&mut self, command: &str) -> Result<&mut Block, String> {
        let block = self.open.last_mut().ok_or_else(|| format!("{command}: no \\if is open"))?;
        if block.in_else {
            return Err(format!("{command}: after \\else"));
        }
        Ok(block)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D212: of `\if`, `\elif` and `\else`,
    /// the first branch true is taken and no other; a block in a branch
    /// not taken takes none of its own, its branches still paired.
    #[test]
    fn the_first_true_branch_is_taken() {
        let mut c = Conditions::default();
        assert!(c.active());
        c.begin(false).unwrap();
        assert!(!c.active());
        c.elif(true).unwrap();
        assert!(c.active());
        c.begin(true).unwrap();
        assert!(c.active(), "nested in a branch taken");
        c.end().unwrap();
        c.elif(true).unwrap();
        assert!(!c.active(), "a branch after one taken");
        c.begin(true).unwrap();
        assert!(!c.active(), "nested in a branch not taken");
        c.otherwise().unwrap();
        assert!(!c.active());
        c.end().unwrap();
        c.otherwise().unwrap();
        assert!(!c.active());
        c.end().unwrap();
        assert!(!c.is_open());
        c.begin(false).unwrap();
        c.otherwise().unwrap();
        assert!(c.active(), "\\else after none taken");
    }

    /// Verifies: [DESIGN] RULES.md D212, R-CF-6: `\elif` and `\else`
    /// after `\else`, and any of them with no block open, are refused;
    /// so is a block past the most open at once.
    #[test]
    fn misplaced_branches_are_refused() {
        let mut c = Conditions::default();
        assert!(c.elif(true).is_err());
        assert!(c.otherwise().is_err());
        assert!(c.end().is_err());
        c.begin(true).unwrap();
        c.otherwise().unwrap();
        assert!(c.elif(true).is_err());
        assert!(c.otherwise().is_err());
        c.end().unwrap();
        for _ in 0..MAX_DEPTH {
            c.begin(true).unwrap();
        }
        assert!(c.begin(true).is_err());
    }
}
