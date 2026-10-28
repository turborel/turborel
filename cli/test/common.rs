//! Fixtures shared by the client program's tests.

use turborel_cli::{Credentials, Input, Io, Options, Server};

pub use turborel_test_support::Scratch;
pub use turborel_test_support::server::{Running, start, start_with};
pub use turborel_test_support::tls::{Pki, pki, secured};

/// Options to reach `server` as X, trusted, to run `inputs`.
pub fn options(server: &Running, inputs: Vec<Input>) -> Options {
    Options {
        server: Server::Tcp { host: "127.0.0.1".into(), port: server.address.port(), tls: None },
        user: "X".into(),
        credentials: Credentials::Trust,
        inputs,
        stop_on_error: false,
        null: String::new(),
    }
}

/// What a run printed, and what it told of failures.
pub struct Ran {
    pub succeeded: bool,
    pub out: String,
    pub err: String,
}

/// Runs `options`, `standard` its standard input, typed if `typed`.
pub fn ran(options: &Options, standard: &str, typed: bool) -> Ran {
    let mut connection = turborel_cli::connect(options, || Ok(String::new())).unwrap();
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let io = Io { input: &mut standard.as_bytes(), typed, out: &mut out, err: &mut err };
    let succeeded = turborel_cli::run(&mut connection, options, io);
    connection.terminate().unwrap();
    Ran { succeeded, out: String::from_utf8(out).unwrap(), err: String::from_utf8(err).unwrap() }
}

/// Statements given on the command line.
pub fn command(sql: &str) -> Input {
    Input::Command(sql.to_owned())
}
