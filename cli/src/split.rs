//! \[DESIGN\] Statements of text, split at each `;` that ends one: the
//! protocol's splitter (`turborel_protocol::split`), which the server
//! uses for a Query of several statements too (RULES.md D192), so the
//! command and the server split text alike.

pub use turborel_client::Splitter;
