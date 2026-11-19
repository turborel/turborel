//! \[DESIGN\] The reader of a plug-in's standard input (RULES.md D167,
//! R-SAFE-5): a thread of the plug-in's own process, named, that reads
//! each frame the database writes and hands its message on, so a call
//! running hears a CANCEL. It ends with its input.

use std::io::Read;
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use turborel::plugin::wire::{Message, read_frame};

/// The name of the thread.
const NAME: &str = "turborel-plugin-input";
/// \[DESIGN\] The longest frame read: the database never writes one past
/// PLUGINS.MAX_MESSAGE_BYTES, whose greatest value this is.
const MOST: u64 = 1 << 30;

/// The messages `input` holds, read on a thread of their own: none once
/// it ends, or holds what is no message.
///
/// # Errors
///
/// If the thread cannot be started.
pub(super) fn messages(mut input: impl Read + Send + 'static) -> std::io::Result<Receiver<Message>> {
    let (to, from) = channel();
    thread::Builder::new().name(NAME.to_owned()).spawn(move || {
        while let Ok(Some(body)) = read_frame(&mut input, MOST) {
            let Ok(message) = Message::decode(&body) else { return };
            if to.send(message).is_err() {
                return;
            }
        }
    })?;
    Ok(from)
}
