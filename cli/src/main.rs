//! `turborel`, the command-line client of the turborel server: `turborel
//! --help`. All it does is in its library, where tests reach it.

#![expect(clippy::print_stdout, clippy::print_stderr, reason = "a program tells its user by printing (RULES.md R-DEV-3)")]

use std::io::IsTerminal;
use std::process::ExitCode;

use std::io::{BufRead, Write};

use turborel_cli::admin::{self, Console, Invocation};
use turborel_cli::{Command, Credentials, Dialer, Input, Interrupts, Io, Lines, Options, Outcome};
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
        Ok(Command::Version) => {
            println!("{}", turborel::product());
            ExitCode::SUCCESS
        }
        Ok(Command::ExtensionHelp) => {
            println!("{}", turborel_cli::extension::EXTENSION_USAGE);
            ExitCode::SUCCESS
        }
        Ok(Command::Extension(invocation)) => {
            let mut out = std::io::stdout().lock();
            turborel_cli::extension::run(&invocation, &mut out).map_or_else(
                |e| {
                    eprintln!("turborel extension: {e}");
                    ExitCode::FAILURE
                },
                |()| ExitCode::SUCCESS,
            )
        }
        Ok(Command::AdminHelp) => {
            println!("{}", admin::ADMIN_USAGE);
            ExitCode::SUCCESS
        }
        Ok(Command::Admin(invocation)) => administer(&invocation).unwrap_or_else(|e| {
            eprintln!("turborel admin: {e}");
            ExitCode::FAILURE
        }),
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

/// A password typed after `prompt`, without its echo.
fn secret(prompt: &str) -> std::io::Result<String> {
    rpassword::prompt_password(prompt)
}

/// Whether `question`, asked at the terminal, is answered `y`.
fn confirm(question: &str) -> std::io::Result<bool> {
    print!("{question}");
    std::io::stdout().flush()?;
    let mut answer = String::new();
    std::io::stdin().lock().read_line(&mut answer)?;
    Ok(answer.trim().eq_ignore_ascii_case("y"))
}

/// Runs the administrative command `invocation` names (RULES.md D218).
fn administer(invocation: &Invocation) -> Result<ExitCode, String> {
    let options = &invocation.options;
    let password = matches!(options.credentials, Credentials::Ask).then(|| secret("password: "));
    let connection = turborel_cli::connect(options, || password.unwrap_or_else(|| Err(std::io::Error::other("none was typed"))))?;
    let dialer = Dialer::new(options.clone(), Box::new(|| secret("password: ")));
    let mut agent = Agent::connected(dialer, connection);
    let terminal = std::io::stdin().is_terminal();
    let (mut asking, mut typing) = (confirm, secret);
    let asked: Option<admin::Confirm<'_>> = if terminal { Some(&mut asking) } else { None };
    let typed: Option<turborel_cli::Secret<'_>> = if terminal { Some(&mut typing) } else { None };
    let mut console = Console { out: &mut std::io::stdout().lock(), confirm: asked, secret: typed };
    let done = admin::run(&mut agent, invocation, &mut console);
    let lost = agent.is_lost();
    // The command's outcome is what is told; the session ends either way.
    drop(agent.terminate());
    match done {
        Ok(()) => Ok(ExitCode::SUCCESS),
        Err(e) => {
            eprintln!("turborel admin: {e}");
            Ok(ExitCode::from(if lost { Outcome::Lost.code() } else { Outcome::Failed.code() }))
        }
    }
}

/// Runs the session `options` ask for: its outcome's exit status
/// (RULES.md D215).
fn session(options: &Options) -> Result<ExitCode, String> {
    // Typed before Ctrl-C is taken, so it ends the program as it would.
    let password = matches!(options.credentials, Credentials::Ask).then(|| rpassword::prompt_password("password: "));
    let interrupts = Interrupts::listen((interrupted, not_cancelled))?;
    // An import's jobs are proved by it too, not asked for it again.
    let typed_once = password.as_ref().and_then(|typed| typed.as_ref().ok()).cloned();
    let connection = turborel_cli::connect(options, || password.unwrap_or_else(|| Err(std::io::Error::other("none was typed"))))?;
    interrupts.cancel_by(connection.canceller());
    let stdin = std::io::stdin();
    let typed = stdin.is_terminal() && options.inputs.contains(&Input::Standard);
    if typed {
        println!("{}", turborel_cli::banner(options, &connection));
    }
    // Each connection CONNECT makes asks for its own password; an
    // import's jobs take the one typed.
    let dialer = Dialer::new(options.clone(), Box::new(|| rpassword::prompt_password("password: "))).with_typed(typed_once);
    let mut agent = Agent::connected(dialer, connection);
    let mut editor =
        typed.then(|| turborel_cli::line::terminal::LineEditor::open(options.settings.environment.history.as_deref())).flatten();
    let mut typing = secret;
    let lines: Option<&mut dyn Lines> = match editor.as_mut() {
        Some(e) => Some(e),
        None => None,
    };
    let asked: Option<turborel_cli::Secret<'_>> = if stdin.is_terminal() { Some(&mut typing) } else { None };
    let terminal = turborel_cli::Terminal { screen: turborel_cli::line::terminal::screen(), editor: lines, secret: asked };
    let io = Io {
        input: &mut stdin.lock(),
        typed,
        out: &mut std::io::stdout().lock(),
        err: &mut std::io::stderr(),
        interrupts: Some(&interrupts),
        terminal,
    };
    let outcome = turborel_cli::outcome(&mut agent, options, io);
    agent.terminate().map_err(|e| e.to_string())?;
    Ok(ExitCode::from(outcome.code()))
}
