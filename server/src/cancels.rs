//! \[DESIGN\] What cancels each connection's statements, by its key
//! (RULES.md D45): a client cancels the statement another connection of
//! its runs with the key it was given at login (`Services::cancel`), and a
//! server hurrying to stop cancels every session's.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use turborel::error::{Code, Result, SqlError};
use turborel::txn::Stop;
use turborel_protocol::{CancelKey, KEY_BYTES};

/// Each registered key, and what it cancels.
#[derive(Default)]
pub(crate) struct Cancels(Mutex<HashMap<CancelKey, Arc<Stop>>>);

/// \[DESIGN\] A connection's cancel key, registered until this is dropped.
pub struct Registered {
    cancels: Arc<Cancels>,
    /// The key.
    pub key: CancelKey,
}

impl Drop for Registered {
    fn drop(&mut self) {
        self.cancels.locked().remove(&self.key);
    }
}

impl Cancels {
    /// \[DESIGN\] A new key, of the system's randomness, that cancels
    /// `stop`'s statements from another connection (RULES.md D45); kept
    /// until what is returned is dropped.
    ///
    /// # Errors
    ///
    /// SQLCODE -915 if the system gives no randomness.
    pub(crate) fn register(self: &Arc<Self>, stop: Arc<Stop>) -> Result<Registered> {
        let mut key = [0_u8; KEY_BYTES];
        aws_lc_rs::rand::fill(&mut key).map_err(|_| SqlError::new(Code::Io, "[DESIGN] the system gave no randomness for a key"))?;
        let key = CancelKey(key);
        self.locked().insert(key, stop);
        Ok(Registered { cancels: Arc::clone(self), key })
    }

    /// Cancels the statement the session of `key` runs, if it is one's.
    pub(crate) fn cancel(&self, key: &CancelKey) {
        if let Some(stop) = self.locked().get(key) {
            stop.cancel();
        }
    }

    /// Cancels the statement every session runs: the sessions'.
    pub(crate) fn cancel_all(&self) -> u64 {
        let cancels = self.locked();
        cancels.values().for_each(|stop| stop.cancel());
        u64::try_from(cancels.len()).unwrap_or(u64::MAX)
    }

    fn locked(&self) -> MutexGuard<'_, HashMap<CancelKey, Arc<Stop>>> {
        // Each key is put and taken whole, so a poisoned lock is whole.
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
