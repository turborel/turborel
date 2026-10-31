//! \[DESIGN\] The server's side of SCRAM-SHA-256 and SCRAM-SHA-256-PLUS (RFC
//! 5802 §3, §6, §7): the client's first message read, its channel binding
//! weighed against what the server offers, the server's sent, and the
//! client's proof checked against StoredKey and its binding against the
//! connection's, the server's signature its answer.

use std::num::NonZeroU32;

use aws_lc_rs::constant_time;
use turborel::error::Result;

use super::{
    Channel, GS2_END_POINT, GS2_NONE, GS2_UNOFFERED, Key, Keys, attribute, decode, encode, hmac, is_nonce, refused, sha256, text, xor,
};

/// \[DESIGN\] The flag of a GS2 header asking for channel binding by
/// another type [RFC 5802 §7: `"p=" cb-name`].
const OTHER_BINDING: &str = "p=";

/// What a server offers an exchange [RFC 5802 §6]: whether the client
/// asked for the mechanism bound to its channel, SCRAM-SHA-256-PLUS, and
/// the channel binding `tls-server-end-point` of its connection, if it has
/// one (`end_point`): over TLS, of a certificate that defines one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer<'a> {
    /// Whether the mechanism is SCRAM-SHA-256-PLUS.
    pub plus: bool,
    /// The connection's channel binding data, if it has one.
    pub end_point: Option<&'a [u8]>,
}

/// The client's first message, read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct First {
    /// Its channel binding, as its GS2 header asks and the server offers.
    channel: Channel,
    /// The message without its GS2 header.
    bare: String,
    /// The client's nonce.
    nonce: String,
}

/// The channel binding the GS2 header of `message` asks for, of the data
/// `offer` binds, and the message without it [RFC 5802 §6]: the PLUS
/// mechanism binds the channel and the other does not; a client that
/// could bind, but believes the server offers none, is refused where the
/// server does, as a mechanism list stripped of the PLUS one would be.
fn binding_of<'m>(message: &'m str, offer: Offer<'_>) -> Result<(Channel, &'m str)> {
    let flags = [(GS2_NONE, Flag::Unbound), (GS2_UNOFFERED, Flag::Unoffered), (GS2_END_POINT, Flag::EndPoint)];
    let Some((flag, bare)) = flags.into_iter().find_map(|(header, flag)| Some((flag, message.strip_prefix(header)?))) else {
        // The flag of `p=tls-server-end-point`, before its authzid.
        let end_point = GS2_END_POINT.strip_suffix(',').unwrap_or(GS2_END_POINT);
        if message.starts_with(OTHER_BINDING) && !message.starts_with(end_point) {
            return Err(refused("the client asks for a channel binding other than tls-server-end-point"));
        }
        return Err(refused("the client asks for another identity, or its header is malformed"));
    };
    let plus = "scram-sha-256-plus binds the channel: its header is p=tls-server-end-point";
    let bound = match (flag, offer.plus, offer.end_point) {
        (Flag::EndPoint, true, Some(data)) => Channel::EndPoint(data.to_vec()),
        (Flag::EndPoint, true, None) => {
            return Err(refused("this connection offers no channel binding: not TLS, or its certificate defines none"));
        }
        (Flag::EndPoint, false, _) => return Err(refused("a client binding its channel asks for scram-sha-256-plus")),
        (Flag::Unbound, true, _) | (Flag::Unoffered, true, _) => return Err(refused(plus)),
        (Flag::Unoffered, false, Some(_)) => {
            return Err(refused("the client believes the server offers no channel binding, which it does: a downgrade"));
        }
        (Flag::Unoffered, false, None) => Channel::Unoffered,
        (Flag::Unbound, false, _) => Channel::Unbound,
    };
    Ok((bound, bare))
}

/// The flag of a GS2 header this server reads.
#[derive(Clone, Copy)]
enum Flag {
    /// `n`.
    Unbound,
    /// `y`.
    Unoffered,
    /// `p=tls-server-end-point`.
    EndPoint,
}

impl First {
    /// `client_first`, read, of a mechanism and connection that `offer`
    /// says.
    ///
    /// # Errors
    ///
    /// SQLSTATE 28000 if it is malformed: not UTF-8, its header asking for
    /// another identity, or a channel binding `offer` does not offer, or
    /// none where the server offers one it could have taken, its user not
    /// a saslname, its nonce not printable, or with attributes beyond the
    /// user and nonce.
    pub fn read(client_first: &[u8], offer: Offer<'_>) -> Result<First> {
        let message = text(client_first)?;
        let (channel, bare) = binding_of(message, offer)?;
        let mut fields = bare.split(',');
        let (user, nonce) = (attribute(fields.next(), 'n')?, attribute(fields.next(), 'r')?);
        if fields.next().is_some() {
            return Err(refused("the client's first message has more than a user and a nonce"));
        }
        if !is_saslname(user) || !is_nonce(nonce) {
            return Err(refused("the client's user or nonce is malformed"));
        }
        Ok(First { channel, bare: bare.to_owned(), nonce: nonce.to_owned() })
    }
}

/// Whether `name` is a saslname [RFC 5802 §7]: a `=` only in `=2C` or
/// `=3D`. An empty one is taken, the protocol's Hello naming the user.
fn is_saslname(name: &str) -> bool {
    name.split('=').skip(1).all(|after| after.starts_with("2C") || after.starts_with("3D"))
}

/// \[DESIGN\] A server's exchange of SCRAM-SHA-256 or SCRAM-SHA-256-PLUS,
/// its first message sent.
#[derive(Clone)]
pub struct Server {
    first: First,
    /// Its first message.
    message: String,
    /// Both nonces, the client's then the server's.
    nonce: String,
    keys: Keys,
}

impl std::fmt::Debug for Server {
    /// Never the keys.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Server({}, ..)", self.message)
    }
}

impl Server {
    /// The exchange of `first`, its server-first made of `nonce`, the
    /// server's, which should be new (`scram::nonce`), and the user's
    /// `salt`, `iterations` and `keys`.
    pub fn begin(first: First, nonce: &str, (salt, iterations): (&[u8], NonZeroU32), keys: Keys) -> Server {
        let both = format!("{}{nonce}", first.nonce);
        let server_first = format!("r={both},s={},i={iterations}", encode(salt));
        Server { first, message: server_first, nonce: both, keys }
    }

    /// server-first.
    pub fn first(&self) -> Vec<u8> {
        self.message.clone().into_bytes()
    }

    /// server-final, once `client_final` proves the client knows the
    /// password.
    ///
    /// # Errors
    ///
    /// SQLSTATE 28000 if it is malformed, of another header or nonce, as a
    /// message replayed from another exchange is, or its proof is wrong.
    pub fn finish(&self, client_final: &[u8]) -> Result<Vec<u8>> {
        let message = text(client_final)?;
        let (without_proof, proof) = message.rsplit_once(",p=").ok_or_else(|| refused("the client's final message has no proof"))?;
        let mut fields = without_proof.split(',');
        let (binding, nonce) = (attribute(fields.next(), 'c')?, attribute(fields.next(), 'r')?);
        if fields.next().is_some() {
            return Err(refused("the client's final message has more than it should"));
        }
        if decode(binding) != Some(self.first.channel.input()) {
            return Err(refused("the client's channel binding is not its first message's, of this connection"));
        }
        if nonce != self.nonce {
            return Err(refused("the nonce is not this exchange's"));
        }
        let proof: Key = decode(proof).and_then(|p| p.try_into().ok()).ok_or_else(|| refused("the proof is malformed"))?;
        let auth = format!("{},{},{without_proof}", self.first.bare, self.message);
        let client_key = xor(&proof, &hmac(&self.keys.stored, auth.as_bytes()));
        constant_time::verify_slices_are_equal(&sha256(&client_key), &self.keys.stored).map_err(|_| refused("the proof is wrong"))?;
        Ok(format!("v={}", encode(&hmac(&self.keys.server, auth.as_bytes()))).into_bytes())
    }
}
