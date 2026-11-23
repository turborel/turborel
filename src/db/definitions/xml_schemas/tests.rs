//! Tests of the document of a registered XML Schema as it is logged: read
//! back from its literal, and kept octet for octet through a failure and a
//! checkpoint, its components the same.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::db::{Database, OpenOptions};
use crate::disk::io::{DiskFile, FileLock, FileSystem, OpenMode};
use crate::disk::memory::MemoryFileSystem;
use crate::value::{Binary, Value};

/// Verifies: [DESIGN] RULES.md D106: a document logged is read back from a
/// binary string literal of its UTF-8 octets, as the same text; anything
/// else, which no statement made here writes, is SQLCODE -910.
#[test]
fn a_logged_document_is_read_back() {
    let of = |l: Literal| ValueExpression::Value(ValueSpecification::Literal(l));
    let text = "<a>\r\n\t\u{e9}\u{4e2d}\u{1f600}\r</a>";
    assert_eq!(logged_document(&of(Literal::Binary(Binary::new(text.into())))).unwrap(), text);
    assert_eq!(logged_document(&of(Literal::Character("<a/>".into()))).unwrap_err().sqlcode(), -910_i32);
    assert_eq!(logged_document(&of(Literal::Binary(Binary::new(vec![0xFF])))).unwrap_err().sqlcode(), -910_i32);
    assert_eq!(logged_document(&of(Literal::Boolean(Some(true)))).unwrap_err().sqlcode(), -910_i32);
}

/// A file system in memory that, as the system does once the openers of a
/// database have crashed, holds no lock against another opener.
struct AfterCrash(MemoryFileSystem);

impl FileSystem for AfterCrash {
    fn open(&self, path: &Path, mode: OpenMode) -> Result<Box<dyn DiskFile>> {
        self.0.open(path, mode)
    }
    fn remove(&self, path: &Path) -> Result<()> {
        self.0.remove(path)
    }
    fn exists(&self, path: &Path) -> Result<bool> {
        self.0.exists(path)
    }
    fn discard(&self, path: &Path) {
        self.0.discard(path);
    }
    fn create_dir_all(&self, path: &Path) -> Result<()> {
        self.0.create_dir_all(path)
    }
    fn create_dir_durably(&self, path: &Path) -> Result<()> {
        self.0.create_dir_durably(path)
    }
    fn read_dir(&self, path: &Path) -> Result<Vec<PathBuf>> {
        self.0.read_dir(path)
    }
    fn sync_dir(&self, path: &Path) -> Result<()> {
        self.0.sync_dir(path)
    }
    fn lock(&self, _path: &Path) -> Result<FileLock> {
        Ok(FileLock::new(()))
    }
}

/// A schema document of every line end, a CR LF, a CR and a LF, of tabs,
/// and of characters of two, three and four UTF-8 octets, in its
/// documentation, its names and an enumeration value; its declaration
/// names an encoding other than the octets logged, as a character string
/// may.
const DOCUMENT: &str = "<?xml version=\"1.0\" encoding=\"ISO-8859-1\"?>\r\n\
<xs:schema xmlns:xs=\"http://www.w3.org/2001/XMLSchema\" targetNamespace=\"urn:caf\u{e9}\" xmlns:c=\"urn:caf\u{e9}\">\r\
\t<xs:annotation><xs:documentation>Caf\u{e9}\t\u{4e2d}\u{6587} \u{1f600}\r\n\u{e9}t\u{e9}</xs:documentation></xs:annotation>\n\
\t<xs:simpleType name=\"th\u{e9}\"><xs:restriction base=\"xs:string\">\r\n\
\t\t<xs:enumeration value=\"th\u{e9} \u{4e2d} \u{1f600}\"/>\n\
\t</xs:restriction></xs:simpleType>\n\
\t<xs:element name=\"caf\u{e9}\" type=\"c:th\u{e9}\"/>\r</xs:schema>\n";

/// What `db` keeps of registered XML Schema A.S: its document, and the
/// names of its element declarations and type definitions.
fn kept(db: &Database) -> (String, Vec<String>, Vec<String>) {
    let st = db.snapshot().unwrap();
    let desc = st.catalog.xml_schema(&QualifiedName { auth: "A".into(), ident: "S".into() }).unwrap();
    let schema = desc.schema.as_ref().unwrap();
    let elements = schema.element_names.keys().map(|n| format!("{n:?}")).collect();
    let types = schema.type_names.keys().map(|n| format!("{n:?}")).collect();
    (desc.document.clone(), elements, types)
}

/// Whether the element `café` of `value` is valid by A.S, of each value.
fn outcomes(db: &Database) -> Vec<String> {
    let s = db.session("A");
    let valid = |v: &str| {
        let xml = format!("<caf\u{e9} xmlns=\"urn:caf\u{e9}\">{v}</caf\u{e9}>");
        let sql = format!("VALUES (XMLPARSE(DOCUMENT '{xml}' PRESERVE WHITESPACE) IS VALID DOCUMENT ACCORDING TO XMLSCHEMA ID A.S)");
        let row = s.open_cursor(&sql).unwrap().fetch().unwrap().unwrap();
        row.iter().map(ToString::to_string).collect::<String>()
    };
    ["th\u{e9} \u{4e2d} \u{1f600}", "the \u{4e2d} \u{1f600}"].map(valid).to_vec()
}

/// Verifies: [DESIGN] RULES.md D25, D106: a document of line ends of each
/// kind, tabs and characters past ASCII, registered and logged, is the
/// same octet for octet once the database is opened again, from the log
/// alone after a failure and from its checkpoint after a close; its
/// elements and types are the same, and its enumeration validates the
/// same value. Were the octets parsed as a binary string document is,
/// by the encoding it declares, its names and value would not be.
#[test]
fn a_document_is_kept_octet_for_octet() {
    for checkpointed in [false, true] {
        let fs: Arc<dyn FileSystem> = Arc::new(AfterCrash(MemoryFileSystem::new()));
        let dir = Path::new("/xml-schema");
        let crashed = Database::new();
        crashed.open_files(&fs, dir, &OpenOptions::default()).unwrap();
        crashed.schema("CREATE SCHEMA AUTHORIZATION A").unwrap();
        let a = crashed.session("A");
        a.execute_prepared(&a.prepare("CREATE XML SCHEMA S AS ?").unwrap(), &[Value::Char(DOCUMENT.into())]).unwrap();
        a.commit().unwrap();
        let registered = kept(&crashed);
        assert_eq!(registered.0, DOCUMENT);
        assert_eq!(outcomes(&crashed), ["TRUE", "FALSE"]);
        if checkpointed {
            crashed.checkpoint().unwrap();
            crashed.close().unwrap();
        }
        let db = Database::new();
        db.open_files(&fs, dir, &OpenOptions::default()).unwrap();
        assert_eq!(kept(&db), registered, "checkpointed: {checkpointed}");
        assert_eq!(outcomes(&db), ["TRUE", "FALSE"], "checkpointed: {checkpointed}");
        db.close().unwrap();
        drop((db, a, crashed));
    }
}
