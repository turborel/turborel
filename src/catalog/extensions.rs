//! \[EXTENSION\] The extensions a database has created (RULES.md D168,
//! D171): each with its version, its directory, its manifest as it was
//! read, and what was checked of its package. Together they are the
//! database's lock of packages (design document §13.6), so an
//! installation elsewhere reproduces the same versions and digests.
//!
//! Written in the catalog's image after the spatial catalog, format 1.

use crate::bytes::{Put, Reader};
use crate::error::Result;

/// An extension created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionDesc {
    /// Its name, as its manifest's NAME.
    pub name: String,
    /// Its version, as its manifest's VERSION.
    pub version: String,
    /// Its directory, absolute.
    pub directory: String,
    /// Its manifest's text, as it was read.
    pub manifest: String,
    /// The key that signed its package, in hexadecimal, if one did.
    pub signer: Option<String>,
    /// The SHA-256 digest of its package's manifest file, in hexadecimal,
    /// if it was installed from a package.
    pub digest: Option<String>,
    /// Whether it was named by CREATE EXTENSION, not created only as
    /// another required it.
    pub explicit: bool,
}

/// `text`, if any, written.
fn put_optional(out: &mut Vec<u8>, text: Option<&str>) {
    out.put_u8(u8::from(text.is_some()));
    if let Some(t) = text {
        out.put_str(t);
    }
}

/// What `put_optional` wrote.
fn optional(r: &mut Reader<'_>) -> Result<Option<String>> {
    if r.u8()? == 0 { Ok(None) } else { r.str().map(Some) }
}

/// `all`, written.
pub(crate) fn put_extensions(out: &mut Vec<u8>, all: &[ExtensionDesc]) {
    out.put_u32(u32::try_from(all.len()).unwrap_or(u32::MAX));
    for d in all {
        out.put_str(&d.name);
        out.put_str(&d.version);
        out.put_str(&d.directory);
        out.put_str(&d.manifest);
        put_optional(out, d.signer.as_deref());
        put_optional(out, d.digest.as_deref());
        out.put_u8(u8::from(d.explicit));
    }
}

/// What `put_extensions` wrote.
///
/// # Errors
///
/// SQLCODE -916 if it is not that.
pub(crate) fn extensions(r: &mut Reader<'_>) -> Result<Vec<ExtensionDesc>> {
    (0..r.u32()?)
        .map(|_| {
            Ok(ExtensionDesc {
                name: r.str()?,
                version: r.str()?,
                directory: r.str()?,
                manifest: r.str()?,
                signer: optional(r)?,
                digest: optional(r)?,
                explicit: r.u8()? != 0,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Verifies: [EXTENSION] RULES.md D171: the extensions of a catalog
    /// image are read back as they were written, each optional field
    /// present or not.
    #[test]
    fn extensions_read_back_as_written() {
        let one = ExtensionDesc {
            name: "GEO".into(),
            version: "1.2.0".into(),
            directory: "/p/geo".into(),
            manifest: "NAME GEO;".into(),
            signer: Some("ab".into()),
            digest: None,
            explicit: true,
        };
        let two = ExtensionDesc { signer: None, digest: Some("cd".into()), explicit: false, ..one.clone() };
        let mut out = Vec::new();
        put_extensions(&mut out, &[one.clone(), two.clone()]);
        let mut r = Reader::new(&out, "extensions");
        assert_eq!(extensions(&mut r).unwrap(), vec![one, two]);
        r.finish().unwrap();
    }
}
