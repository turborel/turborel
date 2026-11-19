//! \[EXTENSION\] How a plug-in's program is told the values of the
//! settings its manifest declares and reads (RULES.md D282).
//!
//! The values go with the calls, never in a call of their own. Before a
//! CALL, the values that call runs with are compared with the last a
//! process was sent; if they differ, a SETTINGS frame goes first on the
//! same pipe, so it is read before the CALL. A call costs no round trip
//! more, and a frame only when a value changed for that process.
//!
//! The values a call runs with are its statement's. A session's call
//! enters the state it runs against (`Calling`), its own values over the
//! database's, on its thread. A call made with no statement, as a host
//! plug-in's or a job's, runs with the database's values, which the
//! database pushes to each pool as they are published (`Feed::tell`).

use std::cell::RefCell;
use std::sync::{Arc, RwLock};

use crate::error::Result;
use crate::settings::Settings;
use crate::value::Value;

use super::manifest::Manifest;
use super::wire::Message;

/// Where a call reads a setting's value by its name.
pub type Source = Arc<dyn Fn(&str) -> Option<Value> + Send + Sync>;

std::thread_local! {
    /// The settings of the statement this thread runs a call of, if one.
    static CALLING: RefCell<Option<Source>> = const { RefCell::new(None) };
}

/// While held, the calls of plug-ins this thread makes run with the
/// settings `source` gives; dropped, as before it was held.
#[must_use]
pub struct Calling(Option<Source>);

impl Calling {
    /// Calls made on this thread read `source` until it is dropped.
    pub fn enter(source: Source) -> Calling {
        Calling(CALLING.with(|c| c.replace(Some(source))))
    }
}

impl Drop for Calling {
    fn drop(&mut self) {
        let before = self.0.take();
        CALLING.with(|c| *c.borrow_mut() = before);
    }
}

/// The settings one plug-in's program is told: those its manifest
/// declares, under its name, and the built-in ones it reads.
#[derive(Debug)]
pub struct Feed {
    names: Vec<String>,
    /// The database's values, as last published.
    told: RwLock<Vec<(String, Value)>>,
}

impl Feed {
    /// The feed of `manifest`'s settings, a host plug-in's if `host`; none
    /// if it declares and reads none.
    pub fn of(manifest: &Manifest, host: bool) -> Option<Arc<Feed>> {
        let own = manifest.settings.iter().map(|d| d.qualified(&manifest.name));
        let names: Vec<String> = own.chain(manifest.reads_settings.iter().cloned()).collect();
        if names.is_empty() {
            return None;
        }
        let mut defaults = Settings::default();
        for d in manifest.settings.iter().filter_map(|d| d.definition(&manifest.name, host).ok()) {
            // Checked as the manifest was read, so a refusal cannot be.
            drop(defaults.declare(d));
        }
        let feed = Feed { names, told: RwLock::new(Vec::new()) };
        feed.tell(&defaults);
        Some(Arc::new(feed))
    }

    /// The names of the settings it tells.
    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Takes the database's values of `settings`, as they are published.
    pub fn tell(&self, settings: &Settings) {
        let told = self.names.iter().filter_map(|n| Some((n.clone(), settings.get(settings.key(n)?).clone()))).collect();
        *self.told.write().unwrap_or_else(std::sync::PoisonError::into_inner) = told;
    }

    /// The values a call made now runs with: its statement's, if it has
    /// one on this thread, else the database's.
    pub fn values(&self) -> Vec<(String, Value)> {
        let told = self.told.read().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
        let source = CALLING.with(|c| c.borrow().clone());
        let Some(source) = source else { return told };
        told.into_iter().map(|(n, v)| (n.clone(), source(&n).unwrap_or(v))).collect()
    }

    /// The SETTINGS a process last sent the values `sent` is to be sent
    /// before its next call, if they changed; `sent` then the values sent.
    ///
    /// # Errors
    ///
    /// SQLSTATE 38T03 if a value cannot be written in protocol 1.
    pub(super) fn frame(&self, sent: &mut Vec<u8>) -> Result<Option<Message>> {
        let message = Message::Settings(self.values());
        let bytes = message.frame()?;
        if bytes == *sent {
            return Ok(None);
        }
        *sent = bytes;
        Ok(Some(message))
    }
}

/// That the SQL `text` the program of the extension `extension` runs sets
/// no setting but one its manifest declares. It never writes another
/// extension's, nor a built-in one (RULES.md D282). A text that does not
/// parse is left to the statement's own error.
///
/// # Errors
///
/// SQLCODE -104 if it sets another's setting.
pub fn own_settings_only(extension: &str, text: &str) -> Result<()> {
    let Ok(statement) = crate::parser::Parser::new(text).and_then(|mut p| p.sql_statement()) else { return Ok(()) };
    let name = if let crate::ast::SqlStatement::SetSetting { name, .. } = &statement {
        name
    } else if let crate::ast::SqlStatement::SetDatabase { name, .. } = &statement {
        name
    } else {
        return Ok(());
    };
    let group = format!("{}.", extension.to_ascii_uppercase());
    if name.trim().to_ascii_uppercase().starts_with(&group) {
        return Ok(());
    }
    Err(crate::error::SqlError::new(
        crate::error::Code::Privilege,
        format!("[EXTENSION] the program of extension {extension} sets its own settings alone, not {name}"),
    ))
}
