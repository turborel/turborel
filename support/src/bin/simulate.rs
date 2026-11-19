//! \[TOOL\] `cargo xtask simulate`: sweeps of the replication simulator
//! longer than `verify` runs (RULES.md D57, R-TEST-5): each plan named,
//! or every plan, from a first seed, as many seeds as asked, the group's
//! databases modelled or real ones kept in files. A violation names its
//! seed and the moment, which makes the run again under a test. Run as
//! `cargo run -p turborel-test-support --features replication --bin
//! simulate -- [--seeds N] [--from S] [--plan NAME] [--databases]
//! [--plugged PER_MILLE]`, the last serving every part of each replica by
//! a stand-in plug-in that dies on that per mille of calls (RULES.md
//! D270).
#![expect(clippy::print_stdout, clippy::print_stderr, reason = "a command-line tool reports by printing (RULES.md R-DEV-3)")]

use std::process::ExitCode;

use turborel_test_support::replication::sim::plans::{PLANS, named};
use turborel_test_support::replication::sim::{Plan, Plugging, simulate_databases_plugged, simulate_plugged};
use turborel_test_support::replication::vr_make;

/// What the command line asks.
struct Asked {
    seeds: u64,
    from: u64,
    plans: Vec<(&'static str, Plan)>,
    databases: bool,
    /// Every part plugged, its stand-ins dying on this per mille of calls.
    plugged: Option<u32>,
}

/// The usage text.
const USAGE: &str = "usage: simulate [--seeds N] [--from S] [--plan NAME] [--databases] [--plugged PER_MILLE]";

/// The command line read.
fn asked(args: &[String]) -> Result<Asked, String> {
    let (mut seeds, mut from, mut plan, mut databases, mut plugged) = (100_u64, 0_u64, None, false, None);
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--seeds" => seeds = number(rest.next())?,
            "--from" => from = number(rest.next())?,
            "--plan" => plan = Some(rest.next().ok_or_else(|| USAGE.to_owned())?.clone()),
            "--databases" => databases = true,
            "--plugged" => plugged = Some(u32::try_from(number(rest.next())?).map_err(|e| e.to_string())?),
            other => return Err(format!("{other}: no such argument\n{USAGE}")),
        }
    }
    let plans = match plan {
        Some(name) => vec![(
            PLANS.iter().find(|(n, _)| *n == name).map_or("", |(n, _)| n),
            named(&name)
                .ok_or_else(|| format!("no plan {name}; the plans: {}", PLANS.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(", ")))?,
        )],
        None => PLANS.iter().map(|(name, make)| (*name, make())).collect(),
    };
    Ok(Asked { seeds, from, plans, databases, plugged })
}

/// A count given after an option.
fn number(value: Option<&String>) -> Result<u64, String> {
    value.ok_or_else(|| USAGE.to_owned())?.parse().map_err(|e| format!("{e}\n{USAGE}"))
}

/// Runs `plan` from `asked.from`, `asked.seeds` seeds: the failures, by
/// seed.
fn sweep(name: &str, plan: &Plan, asked: &Asked) -> Vec<String> {
    let make = vr_make();
    let (mut failures, mut committed, mut transfers, mut lost, mut epochs) = (Vec::new(), 0_u64, 0_u64, 0_u64, 0_u64);
    for seed in asked.from..asked.from.saturating_add(asked.seeds) {
        let plugging = asked.plugged.map(|kill| Plugging::all(kill, kill.saturating_mul(10).min(1000)));
        let report = if asked.databases {
            simulate_databases_plugged(seed, plan, &make, plugging).map(|r| r.replication)
        } else {
            simulate_plugged(seed, plan, &make, plugging)
        };
        match report {
            Ok(report) => {
                committed = committed.saturating_add(u64::try_from(report.committed).unwrap_or(u64::MAX));
                transfers = transfers.saturating_add(report.transfers);
                lost = lost.saturating_add(report.journals_lost);
                epochs = epochs.saturating_add(report.epochs);
            }
            Err(why) => failures.push(why),
        }
    }
    let with = match (asked.databases, asked.plugged) {
        (true, None) => "real databases".to_owned(),
        (false, None) => "modelled databases".to_owned(),
        (true, Some(kill)) => format!("real databases, plugged, {kill} per mille killed"),
        (false, Some(kill)) => format!("modelled databases, plugged, {kill} per mille killed"),
    };
    println!(
        "{name}: {} seeds from {}, {with}: {committed} records committed, {transfers} state transfers, {lost} journals lost, {epochs} reconfigurations, {} failed",
        asked.seeds,
        asked.from,
        failures.len()
    );
    failures
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let asked = match asked(&args) {
        Ok(asked) => asked,
        Err(why) => {
            eprintln!("simulate: {why}");
            return ExitCode::FAILURE;
        }
    };
    let mut failures = Vec::new();
    for (name, plan) in &asked.plans {
        failures.extend(sweep(name, plan, &asked));
    }
    if failures.is_empty() {
        return ExitCode::SUCCESS;
    }
    for failure in &failures {
        eprintln!("simulate: {failure}");
    }
    ExitCode::FAILURE
}
