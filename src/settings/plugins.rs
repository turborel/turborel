//! \[DESIGN\] The settings of extensions from other parties, run as
//! plug-in processes (RULES.md D170-D173): where a plug-in may be, how many
//! of an extension's processes run, how long a call may take, how much a
//! program may send and use, and how packages are trusted.
//!
//! PLUGINS.DIRECTORIES, PLUGINS.TRUSTED_KEYS and PLUGINS.REQUIRE_SIGNED
//! are the node's, given by the host as the database opens, and SQL may
//! not set them, so no session widens what the operator allows (D173).

use super::builtin::{text, whole};
use super::{Definition, Effect, Key, Kind, OFF, SWITCHES};
use crate::plugin::limits::{
    DEFAULT_CALL_TIMEOUT_MS, DEFAULT_CANCEL_GRACE_MS, DEFAULT_MAX_CALL_BYTES, DEFAULT_MAX_MESSAGE_BYTES, DEFAULT_MEMORY_BYTES,
    DEFAULT_PROCESSES,
};

/// PLUGINS.DIRECTORIES.
pub const PLUGINS_DIRECTORIES: Key = Key(62);
/// PLUGINS.PROCESSES.
pub const PLUGINS_PROCESSES: Key = Key(63);
/// PLUGINS.CALL_TIMEOUT_MS.
pub const PLUGINS_CALL_TIMEOUT_MS: Key = Key(64);
/// PLUGINS.MAX_MESSAGE_BYTES.
pub const PLUGINS_MAX_MESSAGE_BYTES: Key = Key(65);
/// PLUGINS.CANCEL_GRACE_MS.
pub const PLUGINS_CANCEL_GRACE_MS: Key = Key(66);
/// PLUGINS.MAX_CALL_BYTES.
pub const PLUGINS_MAX_CALL_BYTES: Key = Key(67);
/// PLUGINS.MEMORY_BYTES.
pub const PLUGINS_MEMORY_BYTES: Key = Key(68);
/// PLUGINS.TRUSTED_KEYS.
pub const PLUGINS_TRUSTED_KEYS: Key = Key(69);
/// PLUGINS.REQUIRE_SIGNED.
pub const PLUGINS_REQUIRE_SIGNED: Key = Key(70);

/// The settings SQL may not set, the node's, which the host gives.
pub const HOST_ONLY: [&str; 4] = ["PLUGINS.DIRECTORIES", "PLUGINS.TRUSTED_KEYS", "PLUGINS.REQUIRE_SIGNED", "PLUGINS.HOST"];

/// The largest whole number a count setting holds.
const MOST: i128 = 9_223_372_036_854_775_807;
/// A whole number from 0 up.
const COUNT: Kind = Kind::Integer { min: 0, max: MOST };
/// The most characters of a list of directories, keys or addresses.
const LIST: Kind = Kind::Text { max: 65_536 };

/// A setting of the database's, taking effect from the next call.
fn database((name, kind, default): (&str, Kind, crate::value::Value), doc: &str) -> Definition {
    Definition { name: name.to_owned(), kind, default, per_session: false, node: false, effect: Effect::Statement, doc: doc.to_owned() }
}

/// A setting of the node's, which the host gives.
fn node((name, kind, default): (&str, Kind, crate::value::Value), doc: &str) -> Definition {
    Definition { node: true, ..database((name, kind, default), doc) }
}

/// A count, as a whole number of a setting.
fn count(n: u64) -> crate::value::Value {
    whole(i128::from(n))
}

/// The settings of extensions, in the order of their keys.
pub(super) fn definitions() -> [Definition; 9] {
    [
        node(
            ("PLUGINS.DIRECTORIES", LIST, text("")),
            "The directories an extension's plug-in may be in, separated by ';', each holding any directory below it; empty, as it is until the host sets it, for none. The host sets it as the database opens; SQL may not.",
        ),
        database(
            ("PLUGINS.PROCESSES", COUNT, count(DEFAULT_PROCESSES)),
            "The most processes of one extension running at once; a call waits for one to be free. 0 is taken as 1.",
        ),
        database(
            ("PLUGINS.CALL_TIMEOUT_MS", COUNT, count(DEFAULT_CALL_TIMEOUT_MS)),
            "The milliseconds a call of a plug-in may take before its process is stopped and the call fails with SQLSTATE 38T02; 0 for no limit. A program starting has as long to answer HELLO, but never less than two minutes.",
        ),
        database(
            ("PLUGINS.MAX_MESSAGE_BYTES", COUNT, count(DEFAULT_MAX_MESSAGE_BYTES)),
            "The most bytes of one message a plug-in sends; a longer one stops its process, and the call fails with SQLSTATE 38T04.",
        ),
        database(
            ("PLUGINS.CANCEL_GRACE_MS", COUNT, count(DEFAULT_CANCEL_GRACE_MS)),
            "The milliseconds a plug-in has to answer a cancelled call before its process is stopped.",
        ),
        database(
            ("PLUGINS.MAX_CALL_BYTES", COUNT, count(DEFAULT_MAX_CALL_BYTES)),
            "The most bytes a plug-in sends in answer to one call, all its messages together; more stops its process, and the call fails with SQLSTATE 38T04.",
        ),
        database(
            ("PLUGINS.MEMORY_BYTES", COUNT, count(DEFAULT_MEMORY_BYTES)),
            "The memory a plug-in is told it may use, in its HELLO; a program that uses more is the operator's to stop, as the database makes no calls of the operating system to cap it.",
        ),
        node(
            ("PLUGINS.TRUSTED_KEYS", LIST, text("")),
            "The Ed25519 public keys whose signatures on a package are trusted, each 64 hexadecimal digits, separated by ';', or '@' and a file of one a line. The host sets it; SQL may not.",
        ),
        node(
            ("PLUGINS.REQUIRE_SIGNED", Kind::Choice(SWITCHES), text(OFF)),
            "Whether a package must be signed by a key of PLUGINS.TRUSTED_KEYS to be installed or loaded, ON, or may be unsigned, OFF. The host sets it; SQL may not.",
        ),
    ]
}
