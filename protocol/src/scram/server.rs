//! \[DESIGN\] The server's side of SCRAM-SHA-256 and SCRAM-SHA-256-PLUS (RFC
//! 5802 §3, §6, §7): the client's first message read, its channel binding
//! weighed against what the server offers, the server's sent, and the
//! client's proof checked against StoredKey and its binding against the
//! connection's, the server's signature its answer.

use std::num::NonZeroU32;

use aws_lc_rs::constant_time;
use turborel::error::Result;

use super::{
    Channel, GS2_END_POINT, GS2_EXPORTER, GS2_NONE, GS2_UNOFFERED, Key, Keys, attribute, decode, encode, hmac, is_nonce, refused, sha256,
    text, xor,
};

/// \[DESIGN\] The flag of a GS2 header asking for channel binding by
/// another type [RFC 5802 §7: `"p=" cb-name`].
const OTHER_BINDING: &str = "p=";

/// What a server offers an exchange [RFC 5802 §6]: whether the client
/// asked for the mechanism bound to its channel, SCRAM-SHA-256-PLUS, and
/// the channel bindings its connection has: over TLS,
/// `tls-server-end-point` of a certificate that defines one (`end_point`),
/// and over TLS 1.3 `tls-exporter` [RFC 9266 §2]. Of the two, the server
/// takes the one the client names [RFC 5802 §6.1].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Offer<'a> {
    /// Whether the mechanism is SCRAM-SHA-256-PLUS.
    pub plus: bool,
    /// The connection's `tls-server-end-point` data, if it has one.
    pub end_point: Option<&'a [u8]>,
    /// The connection's `tls-exporter` data, if it has one.
    pub exporter: Option<&'a [u8]>,
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

/// The flag of the GS2 header `message` begins with, and the message
/// without that header [RFC 5802 §7]: another binding type, or another
/// identity, refused.
fn flag_of(message: &str) -> Result<(Flag, &str)> {
    let flags =
        [(GS2_NONE, Flag::Unbound), (GS2_UNOFFERED, Flag::Unoffered), (GS2_END_POINT, Flag::EndPoint), (GS2_EXPORTER, Flag::Exporter)];
    if let Some(found) = flags.into_iter().find_map(|(header, flag)| Some((flag, message.strip_prefix(header)?))) {
        return Ok(found);
    }
    // The flags `p=tls-server-end-point` and `p=tls-exporter`, before
    // their authzid.
    let known = [GS2_END_POINT, GS2_EXPORTER].map(|header| header.strip_suffix(',').unwrap_or(header));
    if message.starts_with(OTHER_BINDING) && !known.iter().any(|flag| message.starts_with(flag)) {
        return Err(refused("the client asks for a channel binding other than tls-exporter or tls-server-end-point"));
    }
    Err(refused("the client asks for another identity, or its header is malformed"))
}

/// The data of a binding of the connection, `data`; refused, saying
/// `why`, if it has none [RFC 5802 §6: "the server does not support the
/// indicated channel binding type"].
fn offered(data: Option<&[u8]>, why: &str) -> Result<Vec<u8>> {
    data.map(<[u8]>::to_vec).ok_or_else(|| refused(why))
}

/// The channel binding the GS2 header of `message` asks for, of the data
/// `offer` binds, and the message without it [RFC 5802 §6]: the PLUS
/// mechanism binds the channel, by the type the client names, and the
/// other does not; a client that could bind, but believes the server
/// offers none, is refused where the server offers either type, as a
/// mechanism list stripped of the PLUS one would be.
fn binding_of<'m>(message: &'m str, offer: Offer<'_>) -> Result<(Channel, &'m str)> {
    let (flag, bare) = flag_of(message)?;
    let plus = "scram-sha-256-plus binds the channel: its header is p=tls-exporter or p=tls-server-end-point";
    let no_end_point = "this connection offers no channel binding tls-server-end-point: not TLS, or its certificate defines none";
    let no_exporter = "this connection offers no channel binding tls-exporter: not TLS 1.3";
    let binds = offer.end_point.is_some() || offer.exporter.is_some();
    let bound = match (flag, offer.plus, binds) {
        (Flag::EndPoint, true, _) => Channel::EndPoint(offered(offer.end_point, no_end_point)?),
        (Flag::Exporter, true, _) => Channel::Exporter(offered(offer.exporter, no_exporter)?),
        (Flag::EndPoint, false, _) | (Flag::Exporter, false, _) => {
            return Err(refused("a client binding its channel asks for scram-sha-256-plus"));
        }
        (Flag::Unbound, true, _) | (Flag::Unoffered, true, _) => return Err(refused(plus)),
        (Flag::Unoffered, false, true) => {
            return Err(refused("the client believes the server offers no channel binding, which it does: a downgrade"));
        }
        (Flag::Unoffered, false, false) => Channel::Unoffered,
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
    /// `p=tls-exporter`.
    Exporter,
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
