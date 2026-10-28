//! \[EXTENSION\] Where keys come from: an extension point (RULES.md D26,
//! D48). A key is never kept in the database's files; its host gives the
//! encryption a keyring, and a table names a key in it
//! (`ENCODINGS = 'AES-256-GCM:PAYROLL'`). A key service, or an operator's
//! secret store, is a keyring of its own; `KeyDirectory` is the built-in
//! one, of files an operator keeps.

use std::path::{Path, PathBuf};

use turborel::error::{Code, Result, SqlError};

/// Bytes of a key: AES-256's.
pub const KEY_BYTES: usize = 32;
/// \[DESIGN\] The longest name of a key.
pub const MAX_KEY_NAME: usize = 64;

/// \[EXTENSION\] A keyring: the keys an encryption may use, by name.
///
/// Contract (RULES.md R-SAFE-3): `key` gives the same bytes for a name
/// every time it gives any, for as long as data encrypted with them is to
/// be read: a key given once and lost loses that data. It may be called
/// from several threads at once, and never calls back into the database.
pub trait Keyring: Send + Sync {
    /// Its name, for messages.
    fn name(&self) -> &'static str;
    /// The key named `name`, if the keyring has it.
    ///
    /// # Errors
    ///
    /// The keyring cannot be read, or holds a malformed key.
    fn key(&self, name: &str) -> Result<Option<[u8; KEY_BYTES]>>;
}

/// Whether `name` may name a key: 1 to `MAX_KEY_NAME` letters, digits,
/// `_`, `-` or `.`, not beginning with `.`; so it is a plain file's name
/// in any keyring kept as files, and never a path.
pub fn valid_key_name(name: &str) -> bool {
    let allowed = |c: char| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.');
    !name.is_empty() && name.len() <= MAX_KEY_NAME && !name.starts_with('.') && name.chars().all(allowed)
}

/// \[EXTENSION\] The built-in keyring: a directory an operator keeps, each
/// key a file named as the key, holding its 32 bytes as 64 hexadecimal
/// digits (`openssl rand -hex 32`), and nothing else but a line end.
pub struct KeyDirectory {
    dir: PathBuf,
}

impl KeyDirectory {
    /// The keys in `dir`.
    pub fn new(dir: &Path) -> KeyDirectory {
        KeyDirectory { dir: dir.to_path_buf() }
    }
}

fn malformed(name: &str, why: &str) -> SqlError {
    SqlError::new(Code::Api, format!("[DESIGN] the key {name} is malformed: {why}"))
}

impl Keyring for KeyDirectory {
    fn name(&self) -> &'static str {
        "key directory"
    }

    fn key(&self, name: &str) -> Result<Option<[u8; KEY_BYTES]>> {
        if !valid_key_name(name) {
            return Ok(None);
        }
        let text = match std::fs::read_to_string(self.dir.join(name)) {
            Ok(text) => text,
            Err(e) => {
                if e.kind() == std::io::ErrorKind::NotFound {
                    return Ok(None);
                }
                return Err(SqlError::new(Code::Io, format!("[DESIGN] the key {name} cannot be read: {e}")));
            }
        };
        let digits = text.trim_end_matches(['\n', '\r']);
        if digits.len() != KEY_BYTES.saturating_mul(2) {
            return Err(malformed(name, "it is not 64 hexadecimal digits"));
        }
        let mut key = [0_u8; KEY_BYTES];
        for (byte, pair) in key.iter_mut().zip(digits.as_bytes().as_chunks::<2>().0) {
            let pair = std::str::from_utf8(pair).map_err(|_| malformed(name, "it is not hexadecimal"))?;
            *byte = u8::from_str_radix(pair, 16).map_err(|_| malformed(name, "it is not hexadecimal"))?;
        }
        Ok(Some(key))
    }
}
