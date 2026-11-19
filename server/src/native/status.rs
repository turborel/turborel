//! \[DESIGN\] Session status (RULES.md D190): a client granted `status` is
//! sent a Status of each of its session's settings as the session begins,
//! before Ready or a Reset's Complete, and one of each that a statement
//! changed, before that statement's completion; so `setting(name)` on the
//! client reads what the session runs with, at no round trip.
//!
//! The settings are the session's authorization identifier, its default
//! schema, whether its transaction is READ ONLY, the server's version,
//! and every setting a session may set for itself, SESSION.TIME_ZONE and
//! TRANSACTION.ISOLATION among them. After a statement, only what it could
//! have changed is looked at: the identifier, the schema, the access mode,
//! and the settings the session set or reset since; so a statement that
//! changes nothing costs a few comparisons.

use turborel::ast::AccessMode;
use turborel::db::Session;
use turborel::value::Value;
use turborel_protocol::Reply;

/// The status of the session's current authorization identifier.
pub const AUTHORIZATION: &str = "AUTHORIZATION";
/// The status of the session's default schema.
pub const SCHEMA: &str = "SCHEMA";
/// The status of whether the session's transaction is READ ONLY: YES or NO.
pub const READ_ONLY: &str = "READ_ONLY";
/// The status of the server's product and version.
pub const SERVER_VERSION: &str = "SERVER_VERSION";
/// The words of a status of yes or no.
const YES: &str = "YES";
const NO: &str = "NO";

/// What the client was last told of each setting, in the order first told.
#[derive(Debug, Default)]
pub(super) struct Status {
    told: Vec<(String, String)>,
    /// The names of the settings the session had set for itself, as last
    /// looked at.
    own: Vec<String>,
}

/// `value` as a status: as SQL writes it, a null empty.
fn text(value: &Value) -> String {
    if value.is_null() { String::new() } else { value.to_string() }
}

/// Whether `session`'s transaction is READ ONLY.
fn read_only(session: &Session) -> &'static str {
    let access = session.transaction_mode().map(|mode| mode.access);
    if access == Some(AccessMode::ReadOnly) { YES } else { NO }
}

impl Status {
    /// The Status of each of `session`'s settings, as it begins: all told.
    pub(super) fn begun(&mut self, session: &Session) -> Vec<Reply> {
        *self = Status::default();
        let mut now = vec![
            (SERVER_VERSION.to_owned(), turborel::product()),
            (AUTHORIZATION.to_owned(), session.current_authorization()),
            (SCHEMA.to_owned(), session.default_schema()),
            (READ_ONLY.to_owned(), read_only(session).to_owned()),
        ];
        let listed = session.settings().unwrap_or_default();
        now.extend(listed.into_iter().filter(|l| l.definition.per_session).map(|l| (l.definition.name, text(&l.value))));
        self.own = session.own_settings().into_iter().map(|(name, _)| name).collect();
        now.into_iter().filter_map(|(name, value)| self.tell(name, value)).collect()
    }

    /// The Status of each of `session`'s settings a statement changed
    /// since the client was last told.
    pub(super) fn changed(&mut self, session: &Session) -> Vec<Reply> {
        let mut now = vec![
            (AUTHORIZATION.to_owned(), session.current_authorization()),
            (SCHEMA.to_owned(), session.default_schema()),
            (READ_ONLY.to_owned(), read_only(session).to_owned()),
        ];
        let own = session.own_settings();
        // A setting reset takes the database's value again.
        for name in &self.own {
            if !own.iter().any(|(n, _)| n == name) {
                now.push((name.clone(), session.setting(name).map(|v| text(&v)).unwrap_or_default()));
            }
        }
        self.own = own.iter().map(|(name, _)| name.clone()).collect();
        now.extend(own.into_iter().map(|(name, value)| (name, text(&value))));
        now.into_iter().filter_map(|(name, value)| self.tell(name, value)).collect()
    }

    /// The Status of `name` at `value`, if the client was told otherwise.
    fn tell(&mut self, name: String, value: String) -> Option<Reply> {
        if let Some((_, was)) = self.told.iter_mut().find(|(n, _)| *n == name) {
            if *was == value {
                return None;
            }
            was.clone_from(&value);
        } else {
            self.told.push((name.clone(), value.clone()));
        }
        Some(Reply::Status { name, value })
    }
}

#[cfg(test)]
mod tests {
    use turborel::db::Database;

    use super::*;

    /// The names and values of `replies`, each a Status.
    fn told(replies: &[Reply]) -> Vec<(String, String)> {
        replies
            .iter()
            .filter_map(|r| if let Reply::Status { name, value } = r { Some((name.clone(), value.clone())) } else { None })
            .collect()
    }

    /// Verifies: [DESIGN] RULES.md D190: a session's settings are all told
    /// as it begins, the session's own settings among them; then only what
    /// a statement changed: a setting set, then reset to the database's
    /// value, the schema, nothing for a statement that changed nothing.
    #[test]
    fn only_what_changed_is_told_again() {
        let db = Database::new();
        db.schema("CREATE SCHEMA AUTHORIZATION X").unwrap();
        db.schema("CREATE SCHEMA AUTHORIZATION Y").unwrap();
        let session = db.session("X");
        let mut status = Status::default();
        let begun = told(&status.begun(&session));
        assert!(begun.contains(&(AUTHORIZATION.into(), "X".into())) && begun.contains(&(READ_ONLY.into(), NO.into())));
        let zone = begun.iter().find(|(n, _)| n == "SESSION.TIME_ZONE").cloned().expect("a session's own setting");
        assert_eq!(told(&status.changed(&session)).len(), 0, "nothing changed");
        session.direct("SET TIME ZONE INTERVAL '+05:00' HOUR TO MINUTE").unwrap();
        assert_eq!(told(&status.changed(&session)), [("SESSION.TIME_ZONE".to_owned(), "+05:00".to_owned())]);
        session.direct("SET TIME ZONE LOCAL").unwrap();
        assert_eq!(told(&status.changed(&session)), [zone], "back to the database's");
        session.direct("SET SCHEMA 'Y'").unwrap();
        assert_eq!(told(&status.changed(&session)), [(SCHEMA.to_owned(), "Y".to_owned())]);
    }
}
