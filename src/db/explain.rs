//! \[DESIGN\] Showing how a query is planned: its logical plan, the rewrite
//! rules applied, and the physical plan. Split from db.rs (RULES.md R-SIZE-2).

use super::Session;
use crate::error::Result;
use crate::params::Params;

impl Session {
    /// Every stage for a `<cursor specification>`: the plan as bound, the rules
    /// that fired, the optimized plan, and the physical plan. Its parameters
    /// have the values in `params`, as at OPEN [X3.135-1986 §8.8 GR3a].
    ///
    /// # Errors
    ///
    /// Any error of parsing or binding the cursor specification.
    pub fn explain(&self, sql: &str, params: &Params) -> Result<String> {
        let call = self.enter()?;
        let st = call.state();
        let spec = Self::parse_cursor(st, sql, params)?;
        let c = self.compile(st, &spec, params)?;
        // Collapse repeats: "merge_filters x2".
        let mut runs: Vec<(&str, usize)> = Vec::new();
        for rule in &c.trace {
            match runs.last_mut().filter(|(name, _)| name == rule) {
                Some((_, count)) => *count = count.saturating_add(1),
                None => runs.push((rule, 1)),
            }
        }
        let rules: Vec<String> = runs.iter().map(|(r, n)| if *n > 1 { format!("{r} x{n}") } else { r.to_string() }).collect();
        Ok(format!(
            "-- bound:\n{}-- rules: {}\n-- optimized:\n{}-- physical:\n{}",
            c.bound,
            if rules.is_empty() { "none".to_string() } else { rules.join(", ") },
            c.optimized.explain(),
            c.physical.explain()
        ))
    }

    /// Just the physical plan, its parameters with the values in `params`.
    ///
    /// # Errors
    ///
    /// Any error of parsing or binding the cursor specification.
    pub fn physical_plan(&self, sql: &str, params: &Params) -> Result<String> {
        let call = self.enter()?;
        let st = call.state();
        let spec = Self::parse_cursor(st, sql, params)?;
        Ok(self.compile(st, &spec, params)?.physical.explain())
    }
}
