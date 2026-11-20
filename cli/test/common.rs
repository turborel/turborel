//! Fixtures shared by the client program's tests.

use turborel_cli::{Credentials, Dialer, Input, Io, Options, Server, Settings, Terminal};
use turborel_client::agent::Agent;

pub use turborel_test_support::Scratch;
pub use turborel_test_support::server::{Running, start, start_with};
pub use turborel_test_support::tls::{Pki, client_certificate, pki, secured};

/// Options to reach `server` as X, trusted, to run `inputs`.
pub fn options(server: &Running, inputs: Vec<Input>) -> Options {
    Options {
        server: Server::Tcp { host: "127.0.0.1".into(), port: server.address.port(), tls: None },
        user: "X".into(),
        credentials: Credentials::Trust,
        inputs,
        stop_on_error: false,
        null: String::new(),
        timeouts: turborel_client::timeouts::Timeouts::default(),
        import: None,
        settings: Settings::default(),
    }
}

/// What a run printed, and what it told of failures.
pub struct Ran {
    pub succeeded: bool,
    pub out: String,
    pub err: String,
}

/// Runs `options`, `standard` its standard input, typed if `typed`: by an
/// SQL-agent, as the program does, connected as `options` say.
pub fn ran(options: &Options, standard: &str, typed: bool) -> Ran {
    let connection = turborel_cli::connect(options, || Ok(String::new())).unwrap();
    let mut agent = Agent::connected(Dialer::new(options.clone(), Box::new(|| Ok(String::new()))), connection);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let io = Io { input: &mut standard.as_bytes(), typed, out: &mut out, err: &mut err, interrupts: None, terminal: Terminal::default() };
    let succeeded = turborel_cli::run(&mut agent, options, io);
    agent.terminate().unwrap();
    Ran { succeeded, out: String::from_utf8(out).unwrap(), err: String::from_utf8(err).unwrap() }
}

/// Output that cannot be written.
pub struct Closed;

impl std::io::Write for Closed {
    fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
        Err(std::io::Error::other("closed"))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Statements given on the command line.
pub fn command(sql: &str) -> Input {
    Input::Command(sql.to_owned())
}

/// The example plug-in's program, `plugin/examples/plugin_example`, as
/// the workspace's tests built it.
pub fn plugin_example() -> std::path::PathBuf {
    let exe = std::env::current_exe().unwrap();
    let debug = exe.parent().and_then(std::path::Path::parent).unwrap();
    let built = debug.join("examples").join(format!("plugin_example{}", std::env::consts::EXE_SUFFIX));
    assert!(built.exists(), "{} is not built: cargo test --workspace builds it", built.display());
    built
}

/// What a script run printed and told, and how it ended.
pub struct Script {
    pub outcome: turborel_cli::Outcome,
    pub out: String,
    pub err: String,
}

/// What reads a password typed.
pub type Typed = dyn FnMut(&str) -> std::io::Result<String>;

/// What a session's terminal has, owned by the test: its screen, a line
/// editor, what reads a password, and the environment.
#[derive(Default)]
pub struct Seat {
    pub screen: Option<turborel_cli::Screen>,
    pub editor: Option<Box<dyn turborel_cli::Lines>>,
    pub secret: Option<Box<Typed>>,
    /// The environment's variables the command line is read with.
    pub environment: Vec<(&'static str, String)>,
}

/// Runs `text` as standard input, `typed` if so, at `seat`, with the
/// command line `args` besides the connection's to `server` as X, over an
/// SQL-agent as the program does.
pub fn session(server: &Running, args: &[&str], text: &str, typed: bool, mut seat: Seat) -> Script {
    let port = server.address.port().to_string();
    let mut line: Vec<String> = ["-U", "X", "-p", port.as_str()].iter().map(|a| (*a).to_owned()).collect();
    line.extend(args.iter().map(|a| (*a).to_owned()));
    let environment = std::mem::take(&mut seat.environment);
    let env = |name: &str| environment.iter().find(|(n, _)| *n == name).map(|(_, v)| v.clone());
    let Ok(turborel_cli::Command::Run(options)) = turborel_cli::parse(&line, &env) else { panic!("{line:?}") };
    let connection = turborel_cli::connect(&options, || Ok(String::new())).unwrap();
    let mut agent = Agent::connected(Dialer::new((*options).clone(), Box::new(|| Ok(String::new()))), connection);
    let (mut out, mut err) = (Vec::new(), Vec::new());
    let mut input = text.as_bytes();
    let terminal = Terminal {
        screen: seat.screen,
        editor: seat.editor.as_deref_mut().map(|e| e as &mut dyn turborel_cli::Lines),
        secret: seat.secret.as_deref_mut().map(|s| s as &mut dyn FnMut(&str) -> std::io::Result<String>),
    };
    let io = Io { input: &mut input, typed, out: &mut out, err: &mut err, interrupts: None, terminal };
    let outcome = turborel_cli::outcome(&mut agent, &options, io);
    agent.terminate().unwrap();
    Script { outcome, out: String::from_utf8(out).unwrap(), err: String::from_utf8(err).unwrap() }
}

/// Runs `text` as a script, not typed, as `session` does.
pub fn script(server: &Running, args: &[&str], text: &str) -> Script {
    session(server, args, text, false, Seat::default())
}
