//! \[DESIGN\] Cancelling a connection's running statement (RULES.md D45),
//! from another thread: a new connection to the same server, made as the
//! first was, whose first and only message is Cancel with the key the
//! first's Ready gave. The statement fails (SQLCODE -920), and is undone;
//! a cancel as nothing runs does nothing.

use std::io::{Read, Write};
use std::time::Duration;

use turborel_protocol::{CancelKey, Request};

use crate::connection::{Result, failed};
use crate::target::Target;

/// What cancels the statement a connection runs, from any thread.
#[derive(Clone)]
pub struct Canceller {
    pub(crate) target: Target,
    pub(crate) key: CancelKey,
}

impl std::fmt::Debug for Canceller {
    /// Never the key.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Canceller(..)")
    }
}

/// \[DESIGN\] How long a cancel waits for the server, to connect, to send
/// and to be closed, before it gives up: so a server that hangs leaves
/// the thread cancelling free, as for the next Ctrl-C.
pub const CANCEL_WITHIN: Duration = Duration::from_secs(10);

impl Canceller {
    /// Cancels the statement its connection runs, if one runs, within
    /// `CANCEL_WITHIN`.
    ///
    /// # Errors
    ///
    /// A connection failure if the server cannot be reached, or does not
    /// answer in time.
    pub fn cancel(&self) -> Result<()> {
        self.cancel_within(CANCEL_WITHIN)
    }

    /// Cancels as `cancel` does, waiting at most `limit` for each step.
    ///
    /// # Errors
    ///
    /// As `cancel`'s.
    pub fn cancel_within(&self, limit: Duration) -> Result<()> {
        let mut stream = self.target.stream_within(Some(limit))?;
        let mut bytes = Vec::new();
        Request::Cancel(self.key).encode(&mut bytes);
        stream.write_all(&bytes).and_then(|()| stream.flush()).map_err(|e| failed(&format!("[DESIGN] cannot send: {e}")))?;
        // The server closes it, saying nothing.
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).map_err(|e| failed(&format!("[DESIGN] cannot receive: {e}")))?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::net::TcpListener;
    use std::time::Instant;

    use super::*;

    /// Verifies: [DESIGN] RULES.md D45, D55: a cancel sent to a server that
    /// takes it and never closes the connection gives up within its limit,
    /// with a connection failure, so the thread cancelling is free again.
    #[test]
    fn a_cancel_gives_up_in_time() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let canceller = Canceller { target: Target::Tcp(listener.local_addr().unwrap()), key: CancelKey([7; 16]) };
        let began = Instant::now();
        let failed = canceller.cancel_within(Duration::from_millis(50));
        assert!(failed.is_err(), "it never answered");
        assert!(began.elapsed() < CANCEL_WITHIN, "{:?}", began.elapsed());
        drop(listener);
    }
}
