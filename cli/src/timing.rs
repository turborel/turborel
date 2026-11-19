//! \[DESIGN\] The time shown after a statement typed at a terminal
//! (RULES.md D46, D215).
//! The round trip the client measures always comes first. The time the
//! server reports for the statement, split as it splits it, follows it
//! once the client gives it. Until then the line shows the round trip
//! alone, as `Statements::server_time` gives nothing.

use std::time::Duration;

/// The time the server reports spending on one statement, each part
/// given when the server splits it so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServerTime {
    /// The whole of the statement's time at the server.
    pub total: Duration,
    /// Its planning, if given.
    pub planning: Option<Duration>,
    /// Its execution, if given.
    pub execution: Option<Duration>,
    /// Its waiting, on locks and input, if given.
    pub waiting: Option<Duration>,
}

/// The line written after a statement whose round trip took
/// `round_trip`, with `server`'s report beside it when there is one.
#[must_use]
pub fn line(round_trip: Duration, server: Option<&ServerTime>) -> String {
    let Some(server) = server else {
        return format!("Time: {} ns", round_trip.as_nanos());
    };
    let parts: Vec<String> = [("planning", server.planning), ("execution", server.execution), ("waiting", server.waiting)]
        .into_iter()
        .filter_map(|(name, part)| part.map(|time| format!("{name} {} ns", time.as_nanos())))
        .collect();
    let split = if parts.is_empty() { String::new() } else { format!(" ({})", parts.join(", ")) };
    format!("Time: {} ns, server {} ns{split}", round_trip.as_nanos(), server.total.as_nanos())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [DESIGN] RULES.md D215: the round trip is shown alone
    /// until the server reports its time, then the server's beside it,
    /// split only as far as the server splits it.
    #[test]
    fn the_server_time_follows_the_round_trip() {
        let micros = Duration::from_micros;
        assert_eq!(line(Duration::from_nanos(1500), None), "Time: 1500 ns");
        let whole = ServerTime { total: micros(1), ..ServerTime::default() };
        assert_eq!(line(micros(3), Some(&whole)), "Time: 3000 ns, server 1000 ns");
        let split = ServerTime { total: micros(6), planning: Some(micros(1)), execution: Some(micros(4)), waiting: Some(micros(1)) };
        assert_eq!(line(micros(9), Some(&split)), "Time: 9000 ns, server 6000 ns (planning 1000 ns, execution 4000 ns, waiting 1000 ns)");
        let some = ServerTime { execution: Some(micros(2)), ..whole };
        assert_eq!(line(micros(3), Some(&some)), "Time: 3000 ns, server 1000 ns (execution 2000 ns)");
    }
}
