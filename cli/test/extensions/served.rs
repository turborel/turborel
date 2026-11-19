//! A registry served over plain HTTP by a thread of the test's own
//! process: each GET answered with the file of the directory, or 404.
//! Stopped, and its thread joined, as it is dropped.

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::thread::JoinHandle;

pub struct Served {
    pub url: String,
    address: std::net::SocketAddr,
    stop: Arc<AtomicBool>,
    pub requests: Arc<AtomicUsize>,
    thread: Option<JoinHandle<()>>,
}

impl Served {
    /// Serves `dir`.
    pub fn new(dir: &Path) -> Served {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (stop, requests) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicUsize::new(0)));
        let (dir, halt, count) = (dir.to_path_buf(), Arc::clone(&stop), Arc::clone(&requests));
        let thread = std::thread::spawn(move || {
            for stream in listener.incoming() {
                if halt.load(Ordering::SeqCst) {
                    return;
                }
                if let Ok(stream) = stream {
                    count.fetch_add(1, Ordering::SeqCst);
                    answer(stream, &dir);
                }
            }
        });
        Served { url: format!("http://{address}/"), address, stop, requests, thread: Some(thread) }
    }
}

impl Drop for Served {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(self.address);
        if let Some(t) = self.thread.take() {
            t.join().unwrap();
        }
    }
}

/// Answers one request for a file of `dir`.
fn answer(mut stream: TcpStream, dir: &Path) {
    let mut line = String::new();
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    if reader.read_line(&mut line).is_err() {
        return;
    }
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 || header == "\r\n" {
            break;
        }
    }
    let path = line.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/');
    let file: PathBuf = path.split('/').fold(dir.to_path_buf(), |at, part| at.join(part));
    let reply = match std::fs::read(&file).ok().filter(|_| !path.contains("..")) {
        Some(body) => {
            let mut reply = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len()).into_bytes();
            reply.extend(body);
            reply
        }
        None => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(),
    };
    let _ = stream.write_all(&reply);
}
