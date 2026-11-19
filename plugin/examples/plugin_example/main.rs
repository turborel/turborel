//! An external plug-in of turborel, whole, as `doc/client/04-plugins.md`
//! shows it: a program built on the plug-in SDK (`turborel_plugin`) that serves an object of every kind a manifest declares,
//! speaking protocol 1 on its standard input and output. Its manifest,
//! `turborel-plugin.manifest`, is as the tests' `fixture.rs` writes it.
//!
//! SHOUT2 is SHOUT again, for the tests' second extension. The functions
//! PID, FAIL, CRASH, HANG, GARBAGE and HUGE show what the database does
//! of a program that fails: its process id, an ERROR of SQLSTATE 22012,
//! an exit, a call that never answers, a frame of no message, and an
//! answer longer than the database takes. A table
//! of FILES (a log of the changes applied) and an index of SORTED or
//! NEAR are a file each in the extension's directory, the program's
//! working directory; the sink EVENTS appends what it is told to
//! `events.log` there. The host objects (`host_small.rs`,
//! `host_store.rs`, `host_net.rs`, and LOOP and KEPT in
//! `replication.rs`, SHIPPED in `shipped.rs`) serve a host plug-in's
//! manifest, which declares them alone. The optimizer rules and the
//! procedure that asks for statistics are in `rules.rs`; FILES and SORTED
//! answer the planner's statistics.

mod files;
mod functions;
mod host_net;
mod host_small;
mod host_store;
mod indexes;
mod later;
mod replication;
mod rules;
mod services;
mod sha256;
mod shipped;

use turborel_plugin::{Plugin, serve};

/// The plug-in, every object by its name.
fn plugin() -> Plugin {
    let plugin = functions::with(Plugin::new("1.0.0"));
    let plugin = services::with(plugin);
    let plugin = later::with(plugin);
    let plugin = shipped::with(replication::with(host_net::with(host_store::with(host_small::with(plugin)))));
    let plugin = plugin.rule("CONTRADICTION", rules::Contradiction).rule("BROKEN", rules::Broken { fails: false });
    let plugin = plugin.rule("ERRING", rules::Broken { fails: true }).procedure("SKEW", rules::Skew);
    plugin.engine("FILES", files::Files::default()).index_method("SORTED", indexes::Sorted).index_method("NEAR", indexes::Sorted)
}

fn main() {
    if serve(plugin()).is_err() {
        std::process::exit(1);
    }
}
