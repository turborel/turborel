//! \[EXTENSION\] The registries (design document §13.3, §13.6): each a
//! directory of static files, an index, its signature and the package
//! files, served over HTTPS or read where it is. An index is trusted only
//! once its signature verifies by a key trusted and it has not expired;
//! a package fetched is trusted only if its digest is the one the index
//! lists. Packages fetched are kept in the cache by their digest, so one
//! installed again is not fetched again.

use std::path::{Path, PathBuf};

use turborel::plugin::package::PackageCrypto;
use turborel::plugin::package::crypto::Digest;
use turborel::plugin::package::hex;
use turborel::plugin::package::index::{INDEX_FILE, Index, IndexSource, SIGNATURE_FILE, now};
use turborel_client::fetch::Fetcher;
use turborel_client::packages::Crypto;
use turborel_client::tls;

use super::Context;

/// Where a registry is.
enum Place {
    /// Served at this URL, `/` at its end.
    Web(String),
    /// A directory of this machine.
    Directory(PathBuf),
}

/// Where the registry `url` is: a directory if it names no scheme, or
/// `file://`.
fn place(url: &str) -> Place {
    if let Some(path) = url.strip_prefix("file://") {
        return Place::Directory(PathBuf::from(path));
    }
    if url.contains("://") {
        let base = if url.ends_with('/') { url.to_owned() } else { format!("{url}/") };
        return Place::Web(base);
    }
    Place::Directory(PathBuf::from(url))
}

/// The fetcher of `cx`: HTTPS trusting `--tls`'s authorities or the
/// platform's.
fn fetcher(cx: &Context) -> Fetcher {
    let config = tls::roots(cx.roots.as_deref(), cx.roots.is_none()).and_then(tls::config).map_err(|e| e.to_string());
    let fetcher = Fetcher::new(config);
    if cx.allow_http { fetcher.allowing_http() } else { fetcher }
}

/// The file `file` of the registry `url`.
pub(super) fn get(cx: &Context, url: &str, file: &str) -> Result<Vec<u8>, String> {
    match place(url) {
        Place::Directory(dir) => {
            let path = file.split('/').fold(dir, |at, part| at.join(part));
            std::fs::read(&path).map_err(|e| format!("cannot read {}: {e}", path.display()))
        }
        Place::Web(base) => fetcher(cx).get(&format!("{base}{file}")).map_err(|e| format!("cannot fetch {file} of {url}: {e}")),
    }
}

/// The index of the registry `url`, verified.
pub(super) fn index(cx: &Context, url: &str) -> Result<Index, String> {
    let text = get(cx, url, INDEX_FILE)?;
    let signature = get(cx, url, SIGNATURE_FILE).unwrap_or_default();
    let signature = String::from_utf8_lossy(&signature);
    Index::verified(&text, &signature, (&Crypto, &cx.trust), now()).map_err(|e| format!("the registry {url}: {e}"))
}

/// The indexes of every registry, in priority order.
pub(super) fn indexes(cx: &Context) -> Result<IndexSource, String> {
    let indexes = cx.registries.urls.iter().map(|url| Ok((url.clone(), index(cx, url)?))).collect::<Result<_, String>>()?;
    Ok(IndexSource { indexes, pins: cx.registries.pins.clone() })
}

/// The cache's file of the package of `digest`.
fn cached(cx: &Context, digest: &Digest) -> Result<PathBuf, String> {
    Ok(cx.cache_dir()?.join(format!("{}.{}", hex(digest), turborel::plugin::package::PACKAGE_EXTENSION)))
}

/// The package file of `digest`, from the cache if it holds it, else from
/// the registry `url` at `file`, kept in the cache.
///
/// # Errors
///
/// If it cannot be had, or what the registry gives is not of `digest`
/// (SQLSTATE 38T07, as told).
pub(super) fn package(cx: &Context, (url, file): (&str, &str), digest: &Digest) -> Result<Vec<u8>, String> {
    if let Some(bytes) = from_cache(cx, digest) {
        return Ok(bytes);
    }
    let bytes = get(cx, url, file)?;
    if Crypto.sha256(&bytes) != *digest {
        return Err(
            turborel::plugin::package::errors::changed(&format!("{file} of {url} is not what the registry's index says")).to_string()
        );
    }
    keep(cx, &bytes)?;
    Ok(bytes)
}

/// Keeps the package file `bytes` in the cache.
pub(super) fn keep(cx: &Context, bytes: &[u8]) -> Result<(), String> {
    let at = cached(cx, &Crypto.sha256(bytes))?;
    let dir = at.parent().map(Path::to_path_buf).unwrap_or_default();
    std::fs::create_dir_all(&dir).map_err(|e| format!("cannot make the cache {}: {e}", dir.display()))?;
    std::fs::write(&at, bytes).map_err(|e| format!("cannot write {}: {e}", at.display()))
}

/// The package file of `digest` in the cache, if it holds it.
pub(super) fn from_cache(cx: &Context, digest: &Digest) -> Option<Vec<u8>> {
    let bytes = std::fs::read(cached(cx, digest).ok()?).ok()?;
    (Crypto.sha256(&bytes) == *digest).then_some(bytes)
}
