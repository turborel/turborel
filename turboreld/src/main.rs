//! `turboreld`, the turborel server's program: `turboreld --help`. All it
//! does is in its library, where tests reach it.

#![expect(clippy::print_stdout, clippy::print_stderr, reason = "a program tells its operator by printing (RULES.md R-DEV-3)")]

use std::io::{BufRead, Cursor, IsTerminal};
use std::process::ExitCode;

use turboreld::{Command, UserCommand};

// \[DESIGN\] The program allocates through the system's allocator unless
// it is built with one of the features `mimalloc`, `snmalloc` or
// `rpmalloc`, an operator's choice (RULES.md D765). Only the program may
// set one, and the library crates leave it to whoever links them.
#[cfg(any(
    all(feature = "mimalloc", feature = "snmalloc"),
    all(feature = "mimalloc", feature = "rpmalloc"),
    all(feature = "snmalloc", feature = "rpmalloc")
))]
compile_error!("turboreld takes at most one of the features mimalloc, snmalloc and rpmalloc (RULES.md D765)");

/// mimalloc as the global allocator, by the feature `mimalloc` (RULES.md
/// D765).
#[cfg(feature = "mimalloc")]
#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;

/// snmalloc as the global allocator, by the feature `snmalloc` (RULES.md
/// D765).
#[cfg(feature = "snmalloc")]
#[global_allocator]
static ALLOCATOR: snmalloc_rs::SnMalloc = snmalloc_rs::SnMalloc;

/// rpmalloc as the global allocator, by the feature `rpmalloc` (RULES.md
/// D765).
#[cfg(feature = "rpmalloc")]
#[global_allocator]
static ALLOCATOR: rpmalloc::RpMalloc = rpmalloc::RpMalloc;

fn main() -> ExitCode {
    // Timing only, never merged.
    if let Ok(mode) = std::env::var("TURBOREL_TIMING") {
        turborel::engine::columns::timing::run(&turborel_simd::Simd::detect(), &mode);
        return ExitCode::SUCCESS;
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    let served = turboreld::parse(&args).and_then(|command| match command {
        Command::Help => {
            println!("{}", turboreld::USAGE);
            Ok(())
        }
        Command::Version => {
            println!("{}", turborel::product());
            Ok(())
        }
        Command::Serve(mut config) => {
            if config.owner.is_some() {
                turboreld::read_owner_password(&mut config, &mut *password_input(true)?)?;
            }
            let signals = (turboreld::signalled(), turboreld::signalled());
            turboreld::run(&config, |at| eprintln!("turboreld: listening at {}", at.join(", ")), signals)
        }
        Command::Reseal(config, table) => turboreld::run_reseal(&config, &table, &mut std::io::stdout()),
        Command::Rekey(config) => turboreld::run_rekey(&config, &mut std::io::stdout()),
        Command::Backup(config, backup) => turboreld::run_backup(&config, &backup, &mut std::io::stdout()),
        Command::User(config, user) => {
            let takes_one = matches!(user, UserCommand::Add { .. } | UserCommand::Password { .. });
            let mut input = password_input(takes_one)?;
            turboreld::run_user(&config, &user, (&mut *input, &mut std::io::stdout()))
        }
    });
    match served {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("turboreld: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Where a password is read from: typed without its echo at a terminal,
/// if one is taken (`takes_one`) and standard input is one (RULES.md D43);
/// else standard input, as a program gives it.
fn password_input(takes_one: bool) -> Result<Box<dyn BufRead>, String> {
    if !(takes_one && std::io::stdin().is_terminal()) {
        return Ok(Box::new(std::io::stdin().lock()));
    }
    let typed = rpassword::prompt_password("password: ").map_err(|e| format!("cannot read a password: {e}"))?;
    Ok(Box::new(Cursor::new(format!("{typed}\n"))))
}
