//! `turborel`, the command-line client of the turborel server: `turborel
//! --help`. All it does is in its library, where tests reach it.

#![expect(clippy::print_stdout, clippy::print_stderr, reason = "a program tells its user by printing (RULES.md R-DEV-3)")]

use std::io::IsTerminal;
use std::process::ExitCode;

use turborel_cli::{Command, Credentials, Dialer, Input, Interrupts, Io, Options};
use turborel_client::agent::Agent;

/// The status of a program Ctrl-C ended, as shells give it: 128 and SIGINT.
const INTERRUPTED: i32 = 130;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let env = |name: &str| std::env::var(name).ok();
    match turborel_cli::parse(&args, &env) {
        Ok(Command::Help) => {
            println!("{}", turborel_cli::USAGE);
            ExitCode::SUCCESS
        }
        Ok(Command::Run(options)) => session(&options).unwrap_or_else(|e| {
            eprintln!("turborel: {e}");
            ExitCode::FAILURE
        }),
        Err(e) => {
            eprintln!("turborel: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Ctrl-C before a statement can be cancelled: the program ends, as it
/// would have.
#[expect(clippy::exit, reason = "Ctrl-C ends the program as its default would (RULES.md D46)")]
fn interrupted() {
    std::process::exit(INTERRUPTED);
}

/// A failure to cancel, told.
fn not_cancelled(failure: &turborel_cli::Failure) {
    eprintln!("turborel: cannot cancel: {failure}");
}

/// Runs the session `options` ask for: success if every statement did.
fn session(options: &Options) -> Result<ExitCode, String> {
    // Typed before Ctrl-C is taken, so it ends the program as it would.
    let password = matches!(options.credentials, Credentials::Ask).then(|| rpassword::prompt_password("password: "));
    let interrupts = Interrupts::listen((interrupted, not_cancelled))?;
    let connection = turborel_cli::connect(options, || password.unwrap_or_else(|| Err(std::io::Error::other("none was typed"))))?;
    interrupts.cancel_by(connection.canceller());
    // Each connection CONNECT makes asks for its own password.
    let dialer = Dialer::new(options.clone(), Box::new(|| rpassword::prompt_password("password: ")));
    let mut agent = Agent::connected(dialer, connection);
    let stdin = std::io::stdin();
    let typed = stdin.is_terminal() && options.inputs.contains(&Input::Standard);
    let io = Io {
        input: &mut stdin.lock(),
        typed,
        out: &mut std::io::stdout().lock(),
        err: &mut std::io::stderr(),
        interrupts: Some(&interrupts),
    };
    let succeeded = turborel_cli::run(&mut agent, options, io);
    agent.terminate().map_err(|e| e.to_string())?;
    Ok(if succeeded { ExitCode::SUCCESS } else { ExitCode::FAILURE })
}
