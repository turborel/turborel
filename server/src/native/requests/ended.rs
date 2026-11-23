//! \[EXTENSION\] The end of a transaction with its request's statement:
//! committed after its completion, rolled back after its failure, and of
//! a completion told before the end, the wait for what it rests on
//! (RULES.md D850).

use turborel_protocol::{Failure, Reply};

use super::Held;

impl Held {
    /// \[EXTENSION\] `last`, the reply of a statement whose request ends the
    /// transaction with it, once it is ended: committed after a
    /// completion, the commit's failure in its place if it fails; rolled
    /// back after a failure, the rollback's failure in its place if that
    /// fails, the tables then damaged (RULES.md D19). A query's cursor
    /// left open, its rows not all sent, keeps the transaction.
    pub(super) fn ended(&mut self, last: Reply) -> Reply {
        let outcome = match &last {
            Reply::Complete(_) => self.session.commit(),
            Reply::Error(_) => self.session.rollback(),
            Reply::Suspended
            | Reply::Lob(_)
            | Reply::Prepared { .. }
            | Reply::Description(_)
            | Reply::Rows(_)
            | Reply::Challenge(_)
            | Reply::Ready { .. }
            | Reply::Notice(_)
            | Reply::Status { .. }
            | Reply::Pong => {
                return last;
            }
        };
        match outcome {
            Ok(()) => last,
            Err(e) => Reply::Error(Failure::from(&e)),
        }
    }

    /// \[DESIGN\] RULES.md D850: `last`, told before its transaction ends
    /// though its statement ran to be told at the end: once what it rests
    /// on is durable, the failure of that wait in its place.
    pub(super) fn settled(&self, last: Reply) -> Reply {
        match self.session.settle_told() {
            Ok(()) => last,
            Err(e) => Reply::Error(Failure::from(&e)),
        }
    }
}
