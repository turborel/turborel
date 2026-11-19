//! \[EXTENSION\] Kind 38, the buffer pool's eviction policy (RULES.md
//! D283, D284): an advisor told the pool's report now and then, which
//! answers the policy the pool's own evictor applies, or none to keep the
//! last. It is never asked on a read. What it answers is a table of
//! weights the pool reads itself.

use turborel::bytes::Reader;
use turborel::plugin::manifest::HostKind;
use turborel::plugin::payload::eviction::{EVICTION_ADVISE, put_answer, report};

pub use turborel::disk::buffer::{DEFAULT_SAMPLE, NEUTRAL, Policy, RANGES, Range, Report, Sampled};

use super::super::registry::Plugin;
use super::super::serve::Cx;
use super::super::{Answer, PluginError};
use super::HostObject;

/// Kind 38, a buffer eviction policy.
pub trait EvictionAdvisor {
    /// The policy the pool is to evict by, given its `report`; none to
    /// keep the last. `cx` gives the plug-in's settings (`Cx::setting`).
    ///
    /// # Errors
    ///
    /// The advisor's, which stops the node.
    fn advise(&mut self, cx: &mut Cx<'_>, report: &Report) -> Answer<Option<Policy>>;
}

/// Policies of the shapes the evictor's weights express.
pub mod policies {
    use super::{Policy, Range};

    /// Least recently used.
    pub fn lru() -> Policy {
        Policy::default()
    }

    /// LRU-2: a page's age taken from its use before the last, so a page
    /// read once is the first evicted.
    pub fn lru_2() -> Policy {
        Policy { history: 2, ..Policy::default() }
    }

    /// Scan resistant, as 2Q's first queue is. A page read in one interval
    /// alone scores `once` more than one used again.
    pub fn scan_resistant(once: u32) -> Policy {
        Policy { once, ..Policy::default() }
    }

    /// Weighing frequency too, as ARC's second list keeps pages used
    /// often. Each interval a page was used in takes `frequency` off.
    pub fn frequent(frequency: u32) -> Policy {
        Policy { frequency, ..Policy::default() }
    }

    /// `policy`, the pages `first` to `last` of `file` weighed by
    /// `weight` hundredths: 0 a hint to keep them.
    pub fn weighing(mut policy: Policy, file: u32, (first, last): (u64, u64), weight: u32) -> Policy {
        policy.ranges.push(Range { file, first, last, weight });
        policy
    }
}

/// An advisor served.
struct Advised<T>(T);

impl<T: EvictionAdvisor> HostObject for Advised<T> {
    fn answer(&mut self, cx: &mut Cx<'_>, op: u8, payload: &[u8]) -> Answer<Vec<u8>> {
        if op != EVICTION_ADVISE {
            return Err(PluginError::external(&format!("no host operation {op}")));
        }
        let told = report(&mut Reader::received(payload, "host call"))?;
        let policy = self.0.advise(cx, &told)?;
        let mut out = Vec::new();
        put_answer(&mut out, policy.as_ref());
        Ok(out)
    }
}

impl Plugin {
    /// It, serving `advisor` as HOST BUFFER EVICTION `name`.
    #[must_use]
    pub fn host_eviction(self, name: &str, advisor: impl EvictionAdvisor + 'static) -> Plugin {
        self.host(HostKind::BufferEviction.code(), name, Advised(advisor))
    }
}
