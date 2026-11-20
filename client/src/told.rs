//! \[DESIGN\] What a connection asks for as it opens, and what the server
//! has told it since, beside each statement's replies (RULES.md D185):
//! the capabilities granted, the session's settings as Status gave them
//! (D190), the notices the server sent (D187), handed to a handler as
//! they come, the log position of the last commit (D194), and the last
//! statement's completion: its rows (D197), the identity values it
//! generated (D198) and its conditions (D187). It also
//! keeps what each request carries that the caller set: the query
//! attributes (D191).

use std::collections::HashMap;
use std::fmt;

use turborel_protocol::capability::{ALL, COMPRESSION};
use turborel_protocol::compress::DEFAULT_THRESHOLD;
use turborel_protocol::{Attributes, Capabilities, Notice};

/// \[DESIGN\] The connection attribute that names the program.
pub const APPLICATION: &str = "application";

/// What a connection asks for as it opens.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offer {
    /// The capabilities asked for, by name
    /// (`turborel_protocol::capability`).
    pub capabilities: Vec<String>,
    /// The connection's attributes, shown by the server in
    /// INFORMATION_SCHEMA.CONNECTION_ATTRIBUTES: at most
    /// `turborel_protocol::MAX_ATTRIBUTES`, each name of at most
    /// `MAX_ATTRIBUTE_NAME` bytes and value of at most
    /// `MAX_ATTRIBUTE_VALUE`. `APPLICATION` names the program.
    pub attributes: Attributes,
    /// The shortest request sent compressed, once compression is granted.
    pub compress_from: usize,
}

impl Default for Offer {
    /// Every capability but compression, which costs time where the
    /// network is fast; no attributes.
    fn default() -> Offer {
        let capabilities = ALL.iter().filter(|&&name| name != COMPRESSION).map(|&name| name.to_owned()).collect();
        Offer { capabilities, attributes: Vec::new(), compress_from: DEFAULT_THRESHOLD }
    }
}

impl Offer {
    /// No capability: the protocol's first form, as a server before them
    /// speaks it.
    pub fn none() -> Offer {
        Offer { capabilities: Vec::new(), ..Offer::default() }
    }

    /// As it is, asking for compression too.
    #[must_use]
    pub fn compressed(mut self) -> Offer {
        if !self.capabilities.iter().any(|name| name == COMPRESSION) {
            self.capabilities.push(COMPRESSION.to_owned());
        }
        self
    }

    /// As it is, with the attribute `name` of `value`.
    #[must_use]
    pub fn attribute(mut self, name: &str, value: &str) -> Offer {
        self.attributes.push((name.to_owned(), value.to_owned()));
        self
    }
}

/// What takes the notices the server sends.
pub type NoticeHandler = Box<dyn FnMut(&Notice) + Send>;

/// What the server has told a connection, and what it sends with each
/// request.
#[derive(Default)]
pub(crate) struct Told {
    pub(crate) granted: Capabilities,
    pub(crate) settings: HashMap<String, String>,
    pub(crate) handler: Option<NoticeHandler>,
    /// The notices that came while no handler was set, the last
    /// `KEPT_NOTICES`.
    pub(crate) notices: Vec<Notice>,
    pub(crate) position: Option<u64>,
    /// The last statement's completion (RULES.md D197, D198, D187).
    pub(crate) completed: Option<turborel_protocol::Completed>,
    /// The shortest request sent compressed, if compression was granted.
    pub(crate) compress: Option<usize>,
    pub(crate) attributes: Attributes,
}

/// \[DESIGN\] The most notices kept for a connection with no handler: the
/// oldest dropped past them.
pub const KEPT_NOTICES: usize = 64;

impl fmt::Debug for Told {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Told").field("granted", &self.granted).field("settings", &self.settings).finish_non_exhaustive()
    }
}

impl Told {
    /// Takes `notice`: to the handler, or kept.
    pub(crate) fn notice(&mut self, notice: Notice) {
        if let Some(handler) = self.handler.as_mut() {
            handler(&notice);
            return;
        }
        if self.notices.len() == KEPT_NOTICES {
            self.notices.remove(0);
        }
        self.notices.push(notice);
    }

    /// The attributes a Query or an Execute carries: none unless granted.
    pub(crate) fn attributes(&self) -> Attributes {
        if self.granted.has(turborel_protocol::capability::QUERY_ATTRIBUTES) { self.attributes.clone() } else { Vec::new() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D185, D187, D396: a connection asks for
    /// every capability but compression unless told, `declared-types`
    /// among them; a notice with no handler is
    /// kept, the oldest dropped past `KEPT_NOTICES`; with one, handed to it.
    #[test]
    fn an_offer_asks_and_notices_are_kept_or_handed() {
        let offer = Offer::default();
        assert_eq!(offer.capabilities.len(), ALL.len() - 1);
        assert!(offer.capabilities.iter().any(|c| c == turborel_protocol::capability::DECLARED_TYPES), "declared types asked");
        assert!(offer.clone().compressed().capabilities.iter().any(|c| c == COMPRESSION));
        assert_eq!(Offer::none().capabilities.len(), 0);
        let mut told = Told::default();
        let notice =
            |n: usize| Notice { sqlstate: "01000".into(), message: n.to_string(), diagnostics: turborel_protocol::Diagnostics::default() };
        (0..=KEPT_NOTICES).for_each(|n| told.notice(notice(n)));
        assert_eq!(told.notices.len(), KEPT_NOTICES);
        assert_eq!(told.notices[0].message, "1", "the oldest dropped");
        let (sender, heard) = std::sync::mpsc::channel();
        told.handler = Some(Box::new(move |n: &Notice| sender.send(n.message.clone()).unwrap()));
        told.notice(notice(7));
        assert_eq!(heard.recv().unwrap(), "7");
        assert_eq!(told.notices.len(), KEPT_NOTICES, "handed, not kept");
    }
}
