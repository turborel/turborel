//! \[DESIGN\] Cancelling a connection's running statement (RULES.md D45),
//! from another thread: a new connection to the same server, made as the
//! first was, whose first and only message is Cancel with the key the
//! first's Ready gave. The statement fails (SQLCODE -920), and is undone;
//! a cancel as nothing runs does nothing.

use std::io::{Read, Write};

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

impl Canceller {
    /// Cancels the statement its connection runs, if one runs.
    ///
    /// # Errors
    ///
    /// A connection failure if the server cannot be reached.
    pub fn cancel(&self) -> Result<()> {
        let mut stream = self.target.stream()?;
        let mut bytes = Vec::new();
        Request::Cancel(self.key).encode(&mut bytes);
        stream.write_all(&bytes).and_then(|()| stream.flush()).map_err(|e| failed(&format!("[DESIGN] cannot send: {e}")))?;
        // The server closes it, saying nothing.
        let mut rest = Vec::new();
        stream.read_to_end(&mut rest).map_err(|e| failed(&format!("[DESIGN] cannot receive: {e}")))?;
        Ok(())
    }
}
