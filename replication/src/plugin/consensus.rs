//! \[EXTENSION\] A consensus served by a host plug-in (kind 35, RULES.md
//! D273): the protocol's state in its program's process, each input one
//! STEP, whose RESULT carries every output of it, so the driver carries
//! out all of a step's outputs or, the program having ended, none. A
//! step that fails answers nothing and keeps the failure, which the
//! driver reads (`Consensus::failed`) and halts on; the replica, started
//! again, makes its consensus again from its journal (OPEN), as after a
//! crash. The contract of `Consensus` is the program's to keep, as VR
//! keeps it.

use turborel::error::{Result, SqlError};
use turborel::plugin::bridge::leaked;
use turborel::plugin::payload::replication::consensus::{
    CONSENSUS_DESCRIBE, CONSENSUS_OPEN, CONSENSUS_STEP, outputs, put_input, put_open,
};

use super::convert::{configuration_to, input_to, output_from, recovered_to};
use super::{Pipe, Stopping, text};
use crate::{Consensus, Input, Member, Output, Recovered, ReplicaId};

/// \[EXTENSION\] A host plug-in's consensus.
pub struct PluginConsensus {
    pipe: Stopping,
    name: &'static str,
}

impl PluginConsensus {
    /// The consensus of replica `me` of `configuration` over `pipe`,
    /// opened new, or made again from what its journal `recovered`, its
    /// recovery numbered `nonce`.
    ///
    /// # Errors
    ///
    /// The pipe's.
    pub fn open(pipe: Box<dyn Pipe>, (configuration, me): (&[Member], ReplicaId), recovered: Option<&Recovered>, nonce: u64) -> Result<PluginConsensus> {
        let pipe = Stopping::new(pipe, "consensus");
        let held = recovered.map(|r| recovered_to(r, true));
        let mut payload = Vec::new();
        put_open(&mut payload, (me.0, &configuration_to(configuration)), held.as_ref(), nonce);
        let name = pipe.ask(CONSENSUS_OPEN, payload, text)?;
        Ok(PluginConsensus { pipe, name: leaked(&name) })
    }
}

impl Consensus for PluginConsensus {
    fn name(&self) -> &'static str {
        self.name
    }

    fn step(&mut self, input: Input, out: &mut Vec<Output>) {
        let mut payload = Vec::new();
        put_input(&mut payload, &input_to(input));
        // A failure is kept by the pipe, and read by the driver after the
        // step: no output of a step that failed is carried out.
        if let Ok(answered) = self.pipe.ask(CONSENSUS_STEP, payload, outputs) {
            out.extend(answered.into_iter().map(output_from));
        }
    }

    fn describe(&self) -> String {
        match self.pipe.failed() {
            Some(e) => e.msg,
            None => self.pipe.ask(CONSENSUS_DESCRIBE, Vec::new(), text).unwrap_or_else(|e| e.msg),
        }
    }

    fn failed(&self) -> Option<SqlError> {
        self.pipe.failed()
    }
}
