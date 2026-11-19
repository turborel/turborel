//! The example's file system and listener (RULES.md D251, D254): PASS,
//! the operating system's files by the paths the database names, its
//! locks held by the process that took them; and TCP, each process
//! listening at a port of its own, written to `listener-<pid>.port` in
//! the plug-in's directory, for the one connection it keeps.
//! `crash.write` makes the process die halfway through writing a page.

use std::collections::HashMap;
use std::fs::{File, OpenOptions};
use std::io::{ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::time::{Duration, Instant};

use turborel_plugin::host::{Accept, Files, Mode};
use turborel_plugin::{Answer, Plugin, PluginError};

/// The plug-in, with these objects.
pub fn with(plugin: Plugin) -> Plugin {
    plugin.host_files("PASS", Pass::default()).host_listener("TCP", Tcp::default())
}

/// An I/O error, as the program's.
fn failed(e: &std::io::Error) -> PluginError {
    PluginError::new("58030", &e.to_string())
}

/// The file `path`, opened as `write` says.
fn opened(path: &str, write: bool) -> Answer<File> {
    OpenOptions::new().read(true).write(write).open(path).map_err(|e| failed(&e))
}

/// The operating system's files.
#[derive(Default)]
struct Pass {
    locks: HashMap<u64, String>,
    next: u64,
}

impl Files for Pass {
    fn open(&mut self, path: &str, mode: Mode) -> Answer<()> {
        let mut options = OpenOptions::new();
        match mode {
            Mode::Create => options.read(true).write(true).create_new(true),
            Mode::Write => options.read(true).write(true),
            Mode::Read => options.read(true),
        };
        options.open(path).map(drop).map_err(|e| failed(&e))
    }
    fn read(&mut self, path: &str, offset: u64, max: u32) -> Answer<Vec<u8>> {
        let mut f = opened(path, false)?;
        std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(offset)).map_err(|e| failed(&e))?;
        let mut out = Vec::new();
        f.take(u64::from(max)).read_to_end(&mut out).map_err(|e| failed(&e))?;
        Ok(out)
    }
    fn write(&mut self, path: &str, offset: u64, bytes: &[u8]) -> Answer<()> {
        let mut f = opened(path, true)?;
        std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(offset)).map_err(|e| failed(&e))?;
        if std::path::Path::new("crash.write").exists() {
            drop(f.write_all(bytes.get(..bytes.len().checked_div(2).unwrap_or(0)).unwrap_or_default()));
            std::process::abort();
        }
        f.write_all(bytes).map_err(|e| failed(&e))
    }
    fn size(&mut self, path: &str) -> Answer<u64> {
        std::fs::metadata(path).map(|m| m.len()).map_err(|e| failed(&e))
    }
    fn set_size(&mut self, path: &str, size: u64) -> Answer<()> {
        opened(path, true)?.set_len(size).map_err(|e| failed(&e))
    }
    fn sync(&mut self, path: &str, durability: u8) -> Answer<()> {
        if durability == 0 {
            return Ok(());
        }
        opened(path, false)?.sync_all().map_err(|e| failed(&e))
    }
    fn remove(&mut self, path: &str) -> Answer<()> {
        std::fs::remove_file(path).map_err(|e| failed(&e))
    }
    fn exists(&mut self, path: &str) -> Answer<bool> {
        std::path::Path::new(path).try_exists().map_err(|e| failed(&e))
    }
    fn create_dirs(&mut self, path: &str) -> Answer<()> {
        std::fs::create_dir_all(path).map_err(|e| failed(&e))
    }
    fn read_dir(&mut self, path: &str) -> Answer<Vec<String>> {
        let entries = std::fs::read_dir(path).map_err(|e| failed(&e))?;
        let mut paths: Vec<String> = entries.filter_map(Result::ok).map(|e| e.path().to_string_lossy().into_owned()).collect();
        paths.sort();
        Ok(paths)
    }
    fn sync_dir(&mut self, path: &str) -> Answer<()> {
        // A directory opens for reading where the system lets it; where it
        // does not, its entries are as durable as the system makes them.
        File::open(path).map_or(Ok(()), |d| d.sync_all().or(Ok(()))).map_err(|e: std::io::Error| failed(&e))
    }
    fn lock(&mut self, path: &str) -> Answer<u64> {
        if self.locks.values().any(|p| p == path) {
            return Err(PluginError::new("58030", &format!("{path} is locked")));
        }
        self.next = self.next.saturating_add(1);
        self.locks.insert(self.next, path.to_owned());
        Ok(self.next)
    }
    fn unlock(&mut self, token: u64) {
        self.locks.remove(&token);
    }
    fn rename(&mut self, from: &str, to: &str) -> Answer<()> {
        std::fs::rename(from, to).map_err(|e| failed(&e))
    }
    fn remove_tree(&mut self, path: &str) -> Answer<()> {
        std::fs::remove_dir_all(path).map_err(|e| failed(&e))
    }
}

/// A TCP listener of its own port, and the connection it keeps.
#[derive(Default)]
struct Tcp {
    listener: Option<TcpListener>,
    client: Option<TcpStream>,
}

impl Accept for Tcp {
    fn accept(&mut self, wait_ms: u32) -> Answer<Option<String>> {
        if self.listener.is_none() {
            let bound = TcpListener::bind("127.0.0.1:0").map_err(|e| failed(&e))?;
            bound.set_nonblocking(true).map_err(|e| failed(&e))?;
            let port = bound.local_addr().map_err(|e| failed(&e))?.port();
            std::fs::write(format!("listener-{}.port", std::process::id()), port.to_string()).map_err(|e| failed(&e))?;
            self.listener = Some(bound);
        }
        let listener = self.listener.as_ref().ok_or_else(|| PluginError::external("no listener"))?;
        let until = Instant::now() + Duration::from_millis(u64::from(wait_ms));
        loop {
            match listener.accept() {
                Ok((client, from)) => {
                    client.set_nonblocking(false).and_then(|()| client.set_nodelay(true)).map_err(|e| failed(&e))?;
                    self.client = Some(client);
                    return Ok(Some(from.to_string()));
                }
                Err(e) => {
                    if e.kind() != ErrorKind::WouldBlock {
                        return Err(failed(&e));
                    }
                }
            }
            if Instant::now() >= until {
                return Ok(None);
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    fn relay(&mut self, wait_ms: u32, out: &[u8]) -> Answer<(bool, Vec<u8>)> {
        let client = self.client.as_mut().ok_or_else(|| PluginError::external("no connection"))?;
        client.write_all(out).map_err(|e| failed(&e))?;
        let wait = Duration::from_millis(u64::from(wait_ms.max(1)));
        client.set_read_timeout(Some(wait)).map_err(|e| failed(&e))?;
        let mut back = vec![0_u8; 65_536];
        let read = match client.read(&mut back) {
            Ok(n) => n,
            Err(e) => {
                if e.kind() == ErrorKind::WouldBlock || e.kind() == ErrorKind::TimedOut {
                    back.clear();
                    return Ok((false, back));
                }
                0
            }
        };
        back.truncate(read);
        if read == 0 {
            self.client = None;
        }
        Ok((read == 0, back))
    }
}
