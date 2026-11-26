//! \[DESIGN\] What a statement's last reply carries, and what is sent
//! before it, as the client was granted (RULES.md D185): whether a
//! transaction is active, the log position of a commit it made (D194),
//! whether another statement's replies follow (D192), the rows it counted
//! (D197) and the identity values it generated (D198); its conditions,
//! in it or as notices before it (D187), and the settings it changed
//! (D190). A Query of several statements runs
//! them here, one by one, as a Query of each would.

use std::sync::Arc;

use turborel_protocol::capability::{COMMIT_POSITION, CONDITIONS, GENERATED, NOTICES, ROW_COUNTS, STATUS};
use turborel_protocol::split::Splitter;
use turborel_protocol::{Attributes, Completed, Failure, Reply};

use super::{Counted, Held, Outgoing, Work};
use crate::native::notices;

/// The statements of `text`, split as the client's would be.
fn statements(text: &str) -> Vec<String> {
    let mut splitter = Splitter::default();
    let mut all = splitter.feed(text);
    all.extend(splitter.finish());
    all
}

impl Counted {
    /// What `work` counts.
    pub(super) fn of(work: &Work) -> Counted {
        match work {
            Work::Query { .. } | Work::Execute { .. } => Counted::Statement,
            Work::Fetch(..) => Counted::Fetched,
            Work::CopyEnd(_) => Counted::Copy,
            Work::Prepare(..)
            | Work::Close(_)
            | Work::Deallocate(_)
            | Work::Lob(_)
            | Work::Describe(_)
            | Work::CopyIn { .. }
            | Work::CopyData(_)
            | Work::CopyColumns(_)
            | Work::Debug(_) => Counted::Uncounted,
        }
    }
}

impl Held {
    /// `last`, a completion of work `counted` as it says, as the client was
    /// granted: a statement's rows, as GET DIAGNOSTICS ROW_COUNT gives
    /// them, a query's all it returned, and the identity values an INSERT
    /// generated, as SQL writes them; a copy's rows whatever was granted.
    pub(super) fn counted(&self, last: Reply, counted: Counted) -> Reply {
        let Reply::Complete(c) = last else { return last };
        let statement = counted == Counted::Statement;
        let rows = match counted {
            Counted::Statement => c.rows.or_else(|| u64::try_from(self.session.row_count()).ok()),
            Counted::Fetched => c.rows,
            Counted::Copy => return Reply::Complete(c),
            Counted::Uncounted => None,
        };
        let rows = rows.filter(|_| self.granted.has(ROW_COUNTS));
        let generated = if statement && self.granted.has(GENERATED) {
            self.session.generated().iter().map(ToString::to_string).collect()
        } else {
            Vec::new()
        };
        Reply::Complete(Completed { rows, generated, ..c })
    }

    /// `last`, the last reply of a statement: saying whether a transaction
    /// is active, and, of a completion, the position of the commit the
    /// statement made, if the client was granted it, and whether `more`
    /// replies follow.
    pub(super) fn finished(&self, last: Reply, more: bool) -> Reply {
        let active = self.session.transaction().is_some();
        // Taken whether granted or not, so a later statement's is its own.
        let committed = self.session.canceller().served().take_committed();
        let position = committed.filter(|_| self.granted.has(COMMIT_POSITION));
        if let Reply::Complete(c) = last {
            Reply::Complete(Completed { active, position, more, ..c })
        } else if let Reply::Error(f) = last {
            Reply::Error(Failure { active, ..f })
        } else {
            last
        }
    }

    /// `last`, carrying the statement's conditions if it is a completion
    /// and the client was granted `conditions`; else each gathered in
    /// `out` before it as a Notice, if granted `notices`; the settings it
    /// changed gathered too, if granted `status`. The conditions are taken
    /// anyway.
    pub(super) fn told(&mut self, last: Reply, out: &mut Outgoing<'_>) -> Reply {
        let raised = notices::raised(&self.session, &last);
        let last = if let (Reply::Complete(c), true) = (&last, self.granted.has(CONDITIONS)) {
            let conditions = raised.into_iter().map(|n| notices::condition(n, c.active)).collect();
            Reply::Complete(Completed { conditions, ..c.clone() })
        } else {
            if self.granted.has(NOTICES) {
                out.gathered.extend(raised.into_iter().map(Reply::Notice));
            }
            last
        };
        if self.granted.has(STATUS) {
            out.gathered.extend(self.status.changed(&self.session));
        }
        last
    }

    /// \[DESIGN\] RULES.md D192: runs each statement of `text` as a Query
    /// of it, in order, the transaction ended with each if `ends`, every
    /// completion but the last saying more follow; the first that fails
    /// is the last run. Of no statement, one completion.
    ///
    /// # Errors
    ///
    /// A write of replies failed: the connection is gone.
    pub(super) fn several(
        &mut self,
        text: &str,
        ends: bool,
        attributes: Option<&Arc<Attributes>>,
        out: &mut Outgoing<'_>,
    ) -> turborel::error::Result<()> {
        let all = statements(text);
        if all.is_empty() {
            let done = self.finished(Reply::Complete(turborel::db::Completion::Success.into()), false);
            out.gathered.push(done);
            return Ok(());
        }
        let last = all.len().saturating_sub(1);
        for (at, sql) in all.into_iter().enumerate() {
            let work = Work::Query { sql, ends, attributes: attributes.cloned(), multi: false };
            if self.one(work, out, (0, at < last))? {
                break;
            }
        }
        Ok(())
    }
}

impl Outgoing<'_> {
    /// Where a request's replies begin: how many times what was gathered
    /// was sent ahead, and how many replies are gathered.
    pub(super) fn mark(&self) -> (usize, usize) {
        (self.sends, self.gathered.len())
    }

    /// \[DESIGN\] RULES.md D42: a statement that failed, none of its result
    /// sent ahead since `mark`, sends none of it, its Description and
    /// Rows gathered dropped, so its client is told only the failure, as a
    /// statement cancelled before its first row is. Its notices and status
    /// still go.
    pub(super) fn unanswered(&mut self, (sends, from): (usize, usize)) {
        if self.sends != sends {
            return;
        }
        let mut at = 0_usize;
        self.gathered.retain(|r| {
            at = at.saturating_add(1);
            at <= from || !matches!(r, Reply::Description(_) | Reply::Rows(_))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D192: a Query of several statements is
    /// split as the client splits a script: at each semicolon outside a
    /// string, a comment or a compound statement; a last one unended taken
    /// too.
    #[test]
    fn several_statements_are_split_as_a_script_is() {
        let all = statements("SELECT 1; SELECT ';' ; BEGIN ATOMIC SELECT 2; END; VALUES 3");
        assert_eq!(all.len(), 4, "{all:?}");
        assert_eq!(statements("  ").len(), 0);
    }
}
