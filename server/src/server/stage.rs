//! \[DESIGN\] How far a server has come to stopping, told to its
//! connections. Each connection's worker asks before each request whether
//! the server still serves: that is read from one byte alone on its cache
//! lines, written only as the stage moves, so no request writes a line
//! another reads (RULES.md D815). The watch beside it wakes what waits for
//! a stage.

use std::sync::Arc;
use std::sync::atomic::{AtomicU8, Ordering};

use tokio::sync::watch;

/// How far a server has come to stopping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    /// Taking connections.
    Serving,
    /// Told to stop: each connection finishes its statement.
    Stopping,
    /// Past SERVER.SHUTDOWN_TIMEOUT_MS, or told again: statements cancelled,
    /// waits on clients ended.
    Hurried,
}

/// The bytes of the stages, in the order a server comes to them.
const SERVING: u8 = 0;
const STOPPING: u8 = 1;
const HURRIED: u8 = 2;

impl Stage {
    /// Its byte.
    fn byte(self) -> u8 {
        match self {
            Stage::Serving => SERVING,
            Stage::Stopping => STOPPING,
            Stage::Hurried => HURRIED,
        }
    }
}

/// The stage now, as a byte alone on its cache lines: 128 bytes, a pair of
/// 64-byte lines, as a processor's adjacent-line prefetcher moves them
/// (RULES.md D782).
#[repr(align(128))]
#[derive(Default)]
struct Now(AtomicU8);

/// What tells the stage.
pub(super) struct Announce {
    sender: watch::Sender<Stage>,
    now: Arc<Now>,
}

/// What hears it.
pub(super) struct Heard {
    receiver: watch::Receiver<Stage>,
    now: Arc<Now>,
}

/// An announcer of the stage, serving, and what hears it.
pub(super) fn channel() -> (Announce, Heard) {
    let (sender, receiver) = watch::channel(Stage::Serving);
    let now = Arc::new(Now::default());
    (Announce { sender, now: Arc::clone(&now) }, Heard { receiver, now })
}

impl Announce {
    /// Moves the stage to `stage`: its byte first, then its watch.
    pub(super) fn send(&self, stage: Stage) {
        self.now.0.store(stage.byte(), Ordering::SeqCst);
        // Nothing can fail to hear it: every receiver is kept with it.
        let _ = self.sender.send(stage);
    }
}

impl Heard {
    /// Whether the stage is `stage`, read with no lock.
    pub(super) fn is(&self, stage: Stage) -> bool {
        self.now.0.load(Ordering::SeqCst) == stage.byte()
    }

    /// The watch of the stage, to wait for one.
    pub(super) fn watch(&self) -> watch::Receiver<Stage> {
        self.receiver.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D815: the stage is read from its byte
    /// as it is told, and its watch hears it too.
    #[test]
    fn the_stage_is_read_as_told() {
        let (announce, heard) = channel();
        assert!(heard.is(Stage::Serving));
        announce.send(Stage::Stopping);
        assert!(heard.is(Stage::Stopping) && !heard.is(Stage::Serving));
        assert_eq!(*heard.watch().borrow(), Stage::Stopping);
        announce.send(Stage::Hurried);
        assert!(heard.is(Stage::Hurried));
    }
}
