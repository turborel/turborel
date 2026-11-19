//! \[EXTENSION\] A plug-in's aggregate function (RULES.md D169). Its
//! state is the program's own bytes, which the host keeps, so a program
//! holds nothing between calls and grouping may write a state out and
//! merge it back (`write_state`, `merge`).
//!
//! The values a group takes are held until BATCH of them are, then sent in
//! one step with the state; finishing sends what is held first. A state
//! written out carries the values held with it.

use std::sync::Arc;

use crate::bytes::{Put, Reader};
use crate::error::Result;
use crate::functions::{Accumulator, AggregateFunction};
use crate::plugin::manifest::Declared;
use crate::plugin::payload::{AGGREGATE_FINISH, AGGREGATE_MERGE, AGGREGATE_STEP};
use crate::plugin::values::{put_values, stored, value, values};
use crate::value::{DataType, Value};

use super::{Link, Resolve, answer, resolved};

/// What a state written out is called, if it is read wrongly.
const WRITTEN: &str = "a plug-in aggregate's state";

/// A plug-in's aggregate function.
#[derive(Debug)]
pub struct PluginAggregate {
    link: Arc<Link>,
    returns: DataType,
    batch: usize,
}

impl PluginAggregate {
    /// The aggregate `d` declares.
    ///
    /// # Errors
    ///
    /// A type `resolve` refuses.
    pub fn new(d: &Declared, link: Link, resolve: Resolve<'_>) -> Result<PluginAggregate> {
        resolved(&d.parameters, resolve)?;
        let returns = resolved(d.returns.as_slice(), resolve)?.pop().unwrap_or(DataType::Boolean);
        let batch = d.batch.and_then(|b| usize::try_from(b).ok()).unwrap_or(super::scalar::DEFAULT_BATCH);
        Ok(PluginAggregate { link: Arc::new(link), returns, batch })
    }
}

impl AggregateFunction for PluginAggregate {
    fn name(&self) -> &str {
        &self.link.object
    }
    fn result_type(&self, _arg: Option<DataType>) -> Result<DataType> {
        Ok(self.returns.clone())
    }
    fn accumulator(&self, result: DataType) -> Box<dyn Accumulator> {
        Box::new(PluginAccumulator { link: Arc::clone(&self.link), result, batch: self.batch.max(1), state: Vec::new(), held: Vec::new() })
    }
}

/// A group's state: the program's bytes, and the values not yet sent.
struct PluginAccumulator {
    link: Arc<Link>,
    result: DataType,
    batch: usize,
    state: Vec<u8>,
    held: Vec<Value>,
}

impl PluginAccumulator {
    /// Sends the values held, with the state, taking the state answered.
    fn flush(&mut self) -> Result<()> {
        if self.held.is_empty() {
            return Ok(());
        }
        let mut payload = Vec::new();
        payload.put_bytes(&self.state);
        put_values(&mut payload, &self.held)?;
        let got = self.link.plain(AGGREGATE_STEP, payload)?;
        self.state = answer(&got, |r| r.bytes().map(<[u8]>::to_vec))?;
        self.held.clear();
        Ok(())
    }
}

impl Accumulator for PluginAccumulator {
    fn update(&mut self, v: &Value) -> Result<()> {
        self.held.push(v.clone());
        if self.held.len() >= self.batch {
            return self.flush();
        }
        Ok(())
    }
    fn write_state(&self, out: &mut Vec<u8>) -> bool {
        out.put_bytes(&self.state);
        put_values(out, &self.held).is_ok()
    }
    fn merge(&mut self, written: &[u8]) -> Result<()> {
        let mut read = Reader::new(written, WRITTEN);
        let other = read.bytes()?.to_vec();
        let pending = values(&mut read)?;
        read.finish()?;
        for v in &pending {
            self.update(v)?;
        }
        let mut payload = Vec::new();
        payload.put_bytes(&self.state);
        payload.put_bytes(&other);
        let got = self.link.plain(AGGREGATE_MERGE, payload)?;
        self.state = answer(&got, |r| r.bytes().map(<[u8]>::to_vec))?;
        Ok(())
    }
    fn finish(&self) -> Result<Value> {
        let mut last = PluginAccumulator {
            link: Arc::clone(&self.link),
            result: self.result.clone(),
            batch: self.batch,
            state: self.state.clone(),
            held: self.held.clone(),
        };
        last.flush()?;
        let mut payload = Vec::new();
        payload.put_bytes(&last.state);
        let got = self.link.plain(AGGREGATE_FINISH, payload)?;
        stored(answer(&got, value)?, &self.result)
    }
}
