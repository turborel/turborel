//! \[DESIGN\] A session renewed on the same connection (RULES.md D189).
//! Reset puts it back to its state at login: a new session of the same
//! user, the old one's transaction rolled back, its prepared statements,
//! cursors, temporary tables and settings gone, its cancel key kept, its
//! authentication not asked again; its reply a Complete. ChangeUser
//! authenticates the connection again, as another user or the same, as
//! Hello does, and then renews the session so, with a new cancel key; its
//! reply Ready. A ChangeUser refused closes the connection, as a Hello
//! refused does. Before either reply come a notice that the transaction
//! active was rolled back, to a client granted `notices`, and the new
//! session's status, to one granted `status`.

use std::time::Instant;

use turborel::db::{Completion, Session};
use turborel::error::Result;
use turborel_protocol::capability::{NOTICES, STATUS};
use turborel_protocol::{Completed, Reply, VERSION};

use super::lent::Lent;
use super::{Login, notices};
use crate::auth::Peer;
use crate::server::Services;
use crate::shown::Present;
use crate::told;

/// Puts `session` in place of `lent`'s, the old one rolled back on a
/// worker: the replies that come before the renewal's own, the notice
/// that the transaction was rolled back by `what` and the new status, as
/// granted.
async fn renewed(lent: &mut Lent, services: &Services, session: Session, what: &str) -> Result<Vec<Reply>> {
    let active = lent.held.session.transaction().is_some();
    let old = lent.held.renew(session);
    services
        .run(move || {
            // A failed rollback has marked its tables damaged (RULES.md
            // D19): the new session is the client's either way.
            let _ = old.rollback();
        })
        .await?;
    let mut replies = Vec::new();
    if active && lent.held.granted.has(NOTICES) {
        replies.push(notices::rolled_back(what));
    }
    if lent.held.granted.has(STATUS) {
        let Lent { held, .. } = lent;
        replies.extend(held.status.begun(&held.session));
    }
    Ok(replies)
}

/// Resets `lent`'s session, of `login`: a new one of its user, under the
/// same cancel key.
///
/// # Errors
///
/// The worker failed, or the reply could not be sent: the connection is
/// gone.
pub(super) async fn reset(lent: &mut Lent, (services, present): (&Services, &Present), login: &Login) -> Result<()> {
    let session = services.database().session(&login.authorization);
    login.key.rebind(session.canceller());
    present.stoppable(session.canceller());
    let mut replies = renewed(lent, services, session, "Reset").await?;
    replies.push(Reply::Complete(Completed { active: false, ..Completion::Success.into() }));
    services.idle(lent.wire.send_all(&replies)).await
}

/// Authenticates `lent`'s connection again, as `asked`: a user, a
/// mechanism and its first bytes; then renews its session as that user,
/// under a new cancel key, `login` now its.
///
/// # Errors
///
/// The authentication refused, or not done within SERVER.LOGIN_TIMEOUT_MS;
/// the worker failed, or a reply could not be sent: the connection ends.
pub(super) async fn change_user(
    lent: &mut Lent,
    (services, present, peer): (&Services, &Present, &Peer),
    login: &mut Login,
    asked: (String, String, Vec<u8>),
) -> Result<()> {
    let (given, mechanism, initial) = asked;
    let (logins, db) = (services.logins(), services.database());
    logins.wait_turn(db, peer).await;
    let (user, authenticator) = super::admitted(services, VERSION, &given, &mechanism).inspect_err(|e| {
        logins.failed(db, peer);
        told::hello_refused(db, (&given, peer, &mechanism), e);
    })?;
    let exchange = authenticator.begin(&user, peer);
    let exchanging = super::exchanged(&mut lent.wire, (peer, present), services, (exchange, initial));
    let authorization = services.login(Instant::now(), exchanging).await?;
    super::not_a_role(services, &authorization)?;
    let member = db.is_user(&authorization)?.then(|| authorization.clone());
    let session = db.session(&authorization);
    let key = services.register_cancel(session.canceller())?;
    present.stoppable(session.canceller());
    present.authenticated(&authorization);
    let mut replies = renewed(lent, services, session, "ChangeUser").await?;
    let capabilities = lent.held.granted.names();
    replies.push(Reply::Ready { authorization: authorization.clone(), key: key.key, capabilities, server: turborel::product() });
    *login = Login { key, user: member, authorization };
    services.idle(lent.wire.send_all(&replies)).await
}
