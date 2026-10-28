//! `turboreld`, the turborel server's program: `turboreld --help`. All it
//! does is in its library, where tests reach it.

#![expect(clippy::print_stdout, clippy::print_stderr, reason = "a program tells its operator by printing (RULES.md R-DEV-3)")]

use std::io::{BufRead, Cursor, IsTerminal};
use std::process::ExitCode;

use turboreld::{Command, UserCommand};

fn main() -> ExitCode {
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
        Command::Serve(config) => {
            turboreld::run(&config, |at| eprintln!("turboreld: listening at {}", at.join(", ")), turboreld::signalled())
        }
        Command::User(config, user) => {
            let mut input = password_input(&user)?;
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

/// Where `user`'s password is read from: typed without its echo at a
/// terminal, if it takes one and standard input is one (RULES.md D43);
/// else standard input, as a program gives it.
fn password_input(user: &UserCommand) -> Result<Box<dyn BufRead>, String> {
    let takes_one = matches!(user, UserCommand::Add { .. } | UserCommand::Password { .. });
    if !(takes_one && std::io::stdin().is_terminal()) {
        return Ok(Box::new(std::io::stdin().lock()));
    }
    let typed = rpassword::prompt_password("password: ").map_err(|e| format!("cannot read a password: {e}"))?;
    Ok(Box::new(Cursor::new(format!("{typed}\n"))))
}
