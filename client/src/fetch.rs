//! \[EXTENSION\] Fetching a file over HTTPS (design document §13.3), with
//! the feature `tls`: an HTTP/1.1 GET over rustls (RFC 9112), as the
//! command-line client fetches a registry's index and packages. One
//! request per connection, closed after; the body by its length, in
//! chunks, or to the connection's end; a redirect followed a few times,
//! never from HTTPS to plain HTTP. Plain HTTP is refused unless asked for
//! by `Fetcher::allowing_http`, which only tests call.

use std::io::{ErrorKind, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use rustls::ClientConfig;

/// The redirects followed at most.
pub const MAX_REDIRECTS: u32 = 5;
/// The default of the most bytes a response may have: 256 MiB.
pub const DEFAULT_MAX_BYTES: u64 = 256 * 1024 * 1024;
/// How long connecting and each read may wait.
pub const TIMEOUT: Duration = Duration::from_secs(30);
/// The most bytes of a response's head.
const MAX_HEAD: usize = 64 * 1024;

/// Where a URL points.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Url {
    /// Whether over TLS.
    pub secure: bool,
    /// The host, an IPv6 address without its brackets.
    pub host: String,
    /// The port.
    pub port: u16,
    /// The path and query, `/` at least.
    pub path: String,
}

impl Url {
    /// The URL `text` is, `https://` or `http://`.
    ///
    /// # Errors
    ///
    /// Why it is not one.
    pub fn parse(text: &str) -> Result<Url, String> {
        let (secure, rest) = match (text.strip_prefix("https://"), text.strip_prefix("http://")) {
            (Some(rest), _) => (true, rest),
            (None, Some(rest)) => (false, rest),
            (None, None) => return Err(format!("{text} is not an https:// URL")),
        };
        let (authority, path) = rest.find('/').map_or((rest, "/"), |at| rest.split_at(at));
        let (host, port) = match authority.strip_prefix('[') {
            Some(v6) => {
                let (host, after) = v6.split_once(']').ok_or_else(|| format!("{text} has an unclosed IPv6 address"))?;
                (host, after.strip_prefix(':'))
            }
            None => authority.split_once(':').map_or((authority, None), |(h, p)| (h, Some(p))),
        };
        let default = if secure { 443 } else { 80 };
        let port = port.map_or(Ok(default), |p| p.parse().map_err(|_| format!("{text} has the port {p}")))?;
        if host.is_empty() {
            return Err(format!("{text} names no host"));
        }
        Ok(Url { secure, host: host.to_owned(), port, path: path.to_owned() })
    }

    /// The URL `location` names, relative to this one.
    fn join(&self, location: &str) -> Result<Url, String> {
        if location.contains("://") {
            return Url::parse(location);
        }
        let path = if location.starts_with('/') {
            location.to_owned()
        } else {
            let dir = self.path.rsplit_once('/').map_or("", |(d, _)| d);
            format!("{dir}/{location}")
        };
        Ok(Url { path, ..self.clone() })
    }
}

/// Fetches files.
#[derive(Debug, Clone)]
pub struct Fetcher {
    tls: Result<Arc<ClientConfig>, String>,
    allow_http: bool,
    max_bytes: u64,
}

impl Fetcher {
    /// A fetcher over HTTPS by `tls`, which says the authorities trusted,
    /// or why none can be.
    pub fn new(tls: Result<Arc<ClientConfig>, String>) -> Fetcher {
        Fetcher { tls, allow_http: false, max_bytes: DEFAULT_MAX_BYTES }
    }

    /// The same, allowing plain HTTP: for tests alone, against a server
    /// of their own process.
    #[must_use]
    pub fn allowing_http(mut self) -> Fetcher {
        self.allow_http = true;
        self
    }

    /// The same, a response at most `max` bytes.
    #[must_use]
    pub fn at_most(mut self, max: u64) -> Fetcher {
        self.max_bytes = max;
        self
    }

    /// The body of `url`.
    ///
    /// # Errors
    ///
    /// Why it cannot be had: a URL not HTTPS, unless plain HTTP is allowed;
    /// no connection; a status other than 200; a response malformed or
    /// past the limit; too many redirects.
    pub fn get(&self, url: &str) -> Result<Vec<u8>, String> {
        let mut at = Url::parse(url)?;
        for _ in 0..=MAX_REDIRECTS {
            if !at.secure && !self.allow_http {
                return Err(format!("{url} is plain HTTP; a registry is fetched over HTTPS"));
            }
            match self.once(&at)? {
                Response::Body(body) => return Ok(body),
                Response::Moved(location) => {
                    let next = at.join(&location)?;
                    if at.secure && !next.secure {
                        return Err(format!("{url} redirects from HTTPS to plain HTTP"));
                    }
                    at = next;
                }
            }
        }
        Err(format!("{url} redirects more than {MAX_REDIRECTS} times"))
    }

    /// One request and its response.
    fn once(&self, url: &Url) -> Result<Response, String> {
        let address = (url.host.as_str(), url.port).to_socket_addrs().map_err(|e| format!("cannot find {}: {e}", url.host))?;
        let mut last = format!("{} has no address", url.host);
        for a in address {
            match TcpStream::connect_timeout(&a, TIMEOUT) {
                Ok(tcp) => return self.exchange(tcp, url),
                Err(e) => last = format!("cannot connect to {a}: {e}"),
            }
        }
        Err(last)
    }

    /// The request of `url` over `tcp`, and its response.
    fn exchange(&self, tcp: TcpStream, url: &Url) -> Result<Response, String> {
        let set = |e: std::io::Error| format!("cannot set the connection up: {e}");
        tcp.set_read_timeout(Some(TIMEOUT)).map_err(set)?;
        tcp.set_write_timeout(Some(TIMEOUT)).map_err(set)?;
        let request = format!(
            "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: turborel/{}\r\nAccept: */*\r\nAccept-Encoding: identity\r\nConnection: close\r\n\r\n",
            url.path,
            url.host,
            turborel::VERSION
        );
        let limit = self.max_bytes.saturating_add(u64::try_from(MAX_HEAD).unwrap_or(u64::MAX));
        let raw = if url.secure {
            let config = self.tls.as_ref().map_err(|why| format!("cannot fetch over HTTPS: {why}"))?;
            let stream = crate::tls::secured(tcp, (&url.host, config)).map_err(|f| f.to_string())?;
            transfer(stream, &request, limit)?
        } else {
            transfer(tcp, &request, limit)?
        };
        response(&raw, self.max_bytes)
    }
}

/// What a response gave.
#[derive(Debug, PartialEq, Eq)]
enum Response {
    /// The body of a 200.
    Body(Vec<u8>),
    /// A redirect to a location.
    Moved(String),
}

/// Writes `request` to `stream` and reads to its end, at most `limit`
/// bytes.
fn transfer(mut stream: impl Read + Write, request: &str, limit: u64) -> Result<Vec<u8>, String> {
    stream.write_all(request.as_bytes()).and_then(|()| stream.flush()).map_err(|e| format!("cannot send the request: {e}"))?;
    let mut raw = Vec::new();
    let mut chunk = [0_u8; 16 * 1024];
    loop {
        match stream.read(&mut chunk) {
            Ok(0) => return Ok(raw),
            Ok(n) => raw.extend_from_slice(chunk.get(..n).unwrap_or_default()),
            // A server that closes without TLS's close_notify ends its body
            // so too; its length, if it gave one, is checked after.
            Err(e) => {
                if e.kind() == ErrorKind::UnexpectedEof {
                    return Ok(raw);
                }
                return Err(format!("cannot read the response: {e}"));
            }
        }
        if u64::try_from(raw.len()).unwrap_or(u64::MAX) > limit {
            return Err(format!("the response is longer than {limit} bytes"));
        }
    }
}

/// The response `raw` holds.
fn response(raw: &[u8], max: u64) -> Result<Response, String> {
    let end = raw.windows(4).position(|w| w == b"\r\n\r\n").ok_or("the response has no end to its head")?;
    let head = std::str::from_utf8(raw.get(..end).unwrap_or_default()).map_err(|_| "the response's head is not text")?;
    let body = raw.get(end.saturating_add(4)..).unwrap_or_default();
    let mut lines = head.split("\r\n");
    let status = lines.next().unwrap_or_default();
    let code: u16 =
        status.split_whitespace().nth(1).and_then(|c| c.parse().ok()).ok_or_else(|| format!("the status line {status} is malformed"))?;
    let headers: Vec<(String, String)> =
        lines.filter_map(|l| l.split_once(':')).map(|(n, v)| (n.trim().to_ascii_lowercase(), v.trim().to_owned())).collect();
    let header = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str());
    if matches!(code, 301 | 302 | 303 | 307 | 308) {
        return header("location").map(|l| Response::Moved(l.to_owned())).ok_or_else(|| format!("{status} names no location"));
    }
    if code != 200 {
        return Err(format!("the server answered {status}"));
    }
    let body = if header("transfer-encoding").is_some_and(|t| t.eq_ignore_ascii_case("chunked")) {
        dechunked(body)?
    } else if let Some(length) = header("content-length") {
        let length: usize = length.parse().map_err(|_| format!("the length {length} is malformed"))?;
        body.get(..length).ok_or("the response ended before its length")?.to_vec()
    } else {
        body.to_vec()
    };
    if u64::try_from(body.len()).unwrap_or(u64::MAX) > max {
        return Err(format!("the response is longer than {max} bytes"));
    }
    Ok(Response::Body(body))
}

/// The body of chunks `raw` holds [RFC 9112 §7.1].
fn dechunked(mut raw: &[u8]) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    loop {
        let line_end = raw.windows(2).position(|w| w == b"\r\n").ok_or("a chunk has no size")?;
        let line = std::str::from_utf8(raw.get(..line_end).unwrap_or_default()).map_err(|_| "a chunk's size is not text")?;
        let size = usize::from_str_radix(line.split(';').next().unwrap_or_default().trim(), 16)
            .map_err(|_| format!("the chunk size {line} is malformed"))?;
        let start = line_end.saturating_add(2);
        if size == 0 {
            return Ok(body);
        }
        let end = start.checked_add(size).ok_or("a chunk is too long")?;
        body.extend_from_slice(raw.get(start..end).ok_or("the response ended inside a chunk")?);
        raw = raw.get(end.saturating_add(2)..).ok_or("the response ended after a chunk")?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] design document §13.3: URLs of HTTPS and HTTP
    /// parse, with ports, IPv6 hosts and paths; a redirect's location joins
    /// to the URL; a response's body is read by its length or its chunks,
    /// a redirect told, and any status but 200 refused.
    #[test]
    fn urls_and_responses_parse() {
        let u = Url::parse("https://[::1]:8443/reg/index").unwrap();
        assert_eq!((u.secure, u.host.as_str(), u.port, u.path.as_str()), (true, "::1", 8443, "/reg/index"));
        assert_eq!(Url::parse("http://h").unwrap().path, "/");
        assert_eq!(Url::parse("https://h/a/b").unwrap().port, 443);
        assert!(Url::parse("ftp://h").is_err() && Url::parse("https://:1/").is_err());
        assert_eq!(u.join("other").unwrap().path, "/reg/other");
        assert_eq!(u.join("/x").unwrap().path, "/x");
        let plain = b"HTTP/1.1 200 OK\r\nContent-Length: 3\r\n\r\nabcdef";
        assert_eq!(response(plain, 10).unwrap(), Response::Body(b"abc".to_vec()));
        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n2;x\r\nde\r\n0\r\n\r\n";
        assert_eq!(response(chunked, 10).unwrap(), Response::Body(b"abcde".to_vec()));
        assert_eq!(response(b"HTTP/1.1 302 Found\r\nLocation: /n\r\n\r\n", 10).unwrap(), Response::Moved("/n".to_owned()));
        assert!(response(b"HTTP/1.1 404 Not Found\r\n\r\n", 10).is_err());
        assert!(response(plain, 2).is_err());
    }
}
