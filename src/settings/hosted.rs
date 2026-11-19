//! \[DESIGN\] The setting of host plug-ins (RULES.md D229): the programs
//! serving the points beneath the database, which the node declares as
//! it starts and SQL may not set, as the host gives PLUGINS.DIRECTORIES.

use super::builtin::text;
use super::{Definition, Effect, Key, Kind};

/// PLUGINS.HOST.
pub const PLUGINS_HOST: Key = Key(80);

/// The most characters of the list of host plug-ins.
const LIST: Kind = Kind::Text { max: 65_536 };

/// The settings of host plug-ins, in the order of their keys.
pub(super) fn definitions() -> [Definition; 1] {
    [Definition {
        name: "PLUGINS.HOST".to_owned(),
        kind: LIST,
        default: text(""),
        per_session: false,
        node: true,
        effect: Effect::Statement,
        doc: "The host plug-ins the node starts before the database opens, each name=directory, its program's SHA-256 digest pinned by @digest after the directory, separated by ';'; empty, as it is until the host sets it, for none. The host sets it; SQL may not.".to_owned(),
    }]
}
