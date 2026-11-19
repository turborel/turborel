//! \[EXTENSION\] Packages of extensions (design document §13): one file
//! holding a plug-in's manifest, its executables, docs and data, each
//! entry with its SHA-256 digest and the whole optionally signed with
//! Ed25519; installed into a plug-in directory, checked on install and at
//! every load, resolved by name and semantic version. There is no
//! registry: a package is a file, which the operator distributes as they
//! like (RULES.md D260).
//!
//! * `semver.rs`: versions and their ranges.
//! * `archive.rs`: the package file, format 1; `project.rs`: packing one
//!   from a plug-in's project.
//! * `crypto.rs`: the digest and signatures, which the core declares and
//!   the protocol crate gives (`turborel_protocol::packages`), as only it
//!   may use the cryptography library (RULES.md D26).
//! * `trust.rs`: the keys trusted and whether a signature is required.
//! * `install.rs`: unpacking a package, and checking one installed.
//! * `resolve.rs`: choosing the versions of a set of packages.
//! * `lock.rs`: the lock file; `license.rs`: a package's license.
//! * `errors.rs`: the conditions a package is refused with.

pub mod archive;
pub mod crypto;
pub mod errors;
pub mod install;
pub mod license;
pub mod lock;
pub mod project;
pub mod resolve;
pub mod semver;
pub mod trust;

pub use crypto::PackageCrypto;
pub use install::{install_file, install_package, upgrade_scripts, verify_installed};
pub use license::{License, package_license};
pub use resolve::resolve_installed;
pub use trust::Trust;

/// \[DESIGN\] The extension of a package file's name.
pub const PACKAGE_EXTENSION: &str = "tbpkg";
/// \[DESIGN\] The file an installed package keeps its entries' digests and
/// its signature in.
pub const DIGESTS_FILE: &str = "turborel-package.digests";
/// \[DESIGN\] The file that says whether a package was installed by name
/// or only as another's dependency.
pub const STATE_FILE: &str = "turborel-package.state";
/// \[DESIGN\] The version of the package format, of the digests file and
/// of the lock file (VERSIONING.md).
pub const FORMAT: u8 = 1;

/// `bytes` as lower-case hexadecimal.
pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len().saturating_mul(2));
    for b in bytes {
        for nibble in [b.checked_shr(4).unwrap_or(0), b & 0x0f] {
            text.push(char::from(DIGITS.get(usize::from(nibble)).copied().unwrap_or(b'0')));
        }
    }
    text
}

/// The bytes the hexadecimal `text` writes, if it is hexadecimal of an
/// even length.
pub fn unhex(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u32> = text.trim().chars().map(|c| c.to_digit(16)).collect::<Option<_>>()?;
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    digits.chunks(2).map(|pair| u8::try_from(pair.iter().fold(0_u32, |n, d| n.saturating_mul(16).saturating_add(*d))).ok()).collect()
}

/// The `N` bytes the hexadecimal `text` writes, if it writes `N`.
pub fn unhex_array<const N: usize>(text: &str) -> Option<[u8; N]> {
    unhex(text)?.try_into().ok()
}

#[cfg(test)]
mod tests;
