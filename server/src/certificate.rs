//! \[DESIGN\] The mechanism `certificate` (RULES.md D42, D43): a client
//! proves who it is by the certificate it presented over TLS, which TLS
//! verified against the authorities of SERVER.TLS_CLIENT_CA. The
//! certificate is the user's if the common name of its subject, read as an
//! `<identifier>` as a user's name is [ISO/IEC 9075:1992 §5.4], is the
//! user named, and the user is one of the database's: so `CN=ann` logs in
//! as ANN. One name, compared whole, is the simplest mapping that cannot
//! be stretched: whoever the authorities certify as ANN may log in as ANN,
//! and no one else by that certificate.

use turborel::db::Database;

use crate::auth::{Authenticator, Exchange, Peer, Step};

/// The name of the mechanism of `Certificate`.
pub const MECHANISM: &str = "certificate";

/// \[DESIGN\] The mechanism `certificate`.
pub struct Certificate {
    db: Database,
}

impl Certificate {
    /// The mechanism over the users of `db`.
    pub fn new(db: Database) -> Certificate {
        Certificate { db }
    }
}

impl Authenticator for Certificate {
    fn mechanism(&self) -> &'static str {
        MECHANISM
    }

    fn begin(&self, user: &str, peer: &Peer) -> Box<dyn Exchange> {
        Box::new(Presented { db: self.db.clone(), user: user.to_owned(), name: peer.certificate.clone() })
    }
}

/// An exchange of `Certificate`: the user named, and its certificate's
/// common name, if one was presented and verified.
struct Presented {
    db: Database,
    user: String,
    name: Option<String>,
}

impl Exchange for Presented {
    fn step(&mut self, _: &[u8]) -> Step {
        let Some(name) = &self.name else {
            return Step::Refuse("[DESIGN] no certificate of SERVER.TLS_CLIENT_CA's authorities, of one common name, was presented".into());
        };
        let named = turborel::lexer::authorization_identifier(name).is_ok_and(|id| id == self.user);
        if named && self.db.is_user(&self.user).unwrap_or(false) {
            return Step::Accept(self.user.clone());
        }
        Step::Refuse("[DESIGN] the certificate is not of the user named".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D43: a certificate's common name read as
    /// an identifier names the user, who must be one: `ann` is ANN; another
    /// name, a name no identifier, a user not there, or no certificate are
    /// refused.
    #[test]
    fn a_certificate_names_its_user() {
        let db = Database::new();
        db.create_user("ANN", b"v").unwrap();
        let mechanism = Certificate::new(db);
        let with = |name: Option<&str>| Peer { certificate: name.map(str::to_owned), ..Peer::local() };
        assert_eq!(mechanism.begin("ANN", &with(Some("ann"))).step(b""), Step::Accept("ANN".into()));
        for (user, name) in [("ANN", Some("bob")), ("ANN", Some("a b")), ("BOB", Some("bob")), ("ANN", None)] {
            assert!(matches!(mechanism.begin(user, &with(name)).step(b""), Step::Refuse(_)), "{user} {name:?}");
        }
        assert_eq!(mechanism.mechanism(), MECHANISM);
    }
}
