//! \[EXTENSION\] The authentication mechanisms extensions bring (RULES.md
//! D169): a mechanism no authenticator of the server has is asked of the
//! extensions of the database served, so CREATE EXTENSION of one with an
//! AUTHENTICATOR makes its mechanism one clients may ask for, and DROP
//! EXTENSION takes it away. Each exchange is a conversation with the
//! extension's program on a process of its own; the user it accepts is the
//! user the client named.

use std::sync::Arc;

use turborel::db::Database;
use turborel::plugin::bridge::auth::{AuthStep, Connecting, PluginAuthenticator, PluginExchange};

use crate::auth::{Authenticator, Exchange, Peer, Step};

/// What an extension's mechanism is named, as the server tells of it: the
/// name the client asked for is the extension's.
const MECHANISM: &str = "extension";

/// The authenticator of `mechanism` an extension of `db` brings, if one
/// does.
pub(crate) fn extended(db: &Database, mechanism: &str) -> Option<Arc<dyn Authenticator>> {
    let found = db.extension_authenticator(mechanism).ok().flatten()?;
    Some(Arc::new(Extended(found)))
}

/// \[EXTENSION\] An extension's authenticator, as the server's.
struct Extended(Arc<PluginAuthenticator>);

impl Authenticator for Extended {
    fn mechanism(&self) -> &'static str {
        MECHANISM
    }

    fn begin(&self, user: &str, peer: &Peer) -> Box<dyn Exchange> {
        let connecting = Connecting { user, peer: &peer.name(), local: peer.is_local(), secure: peer.secure };
        match self.0.begin(&connecting) {
            Ok(exchange) => Box::new(Begun { exchange, user: user.to_owned() }),
            Err(e) => Box::new(Failed(e.msg)),
        }
    }
}

/// An exchange begun with the extension's program.
struct Begun {
    exchange: PluginExchange,
    user: String,
}

impl Exchange for Begun {
    fn step(&mut self, input: &[u8]) -> Step {
        match self.exchange.step(input) {
            Ok(AuthStep::Challenge(bytes)) => Step::Challenge(bytes),
            Ok(AuthStep::Accept) => Step::Accept(self.user.clone()),
            Ok(AuthStep::Refuse(why)) => Step::Refuse(why),
            Err(e) => Step::Refuse(e.msg),
        }
    }
}

/// An exchange the program could not begin: refused at its first step.
struct Failed(String);

impl Exchange for Failed {
    fn step(&mut self, _: &[u8]) -> Step {
        Step::Refuse(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] RULES.md D169: an exchange an extension's
    /// program could not begin is refused at its first step, with why.
    #[test]
    fn an_exchange_not_begun_is_refused() {
        let mut failed = Failed("no program".into());
        assert_eq!(failed.step(b"x"), Step::Refuse("no program".into()));
    }
}
