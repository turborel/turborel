//! Unit tests of shadow files made and removed: at commits, so after a
//! crash a directory is as the last commit left it (RULES.md R-TEST-1,
//! R-TEST-2).

use super::*;

const OTHER: &str = "/d/g";

/// The paths in `/d` of `fs`.
fn listed(fs: &dyn FileSystem) -> Vec<PathBuf> {
    fs.read_dir(Path::new("/d")).unwrap()
}

/// Verifies: [DESIGN] RULES.md D30, files made: one made is not on the
/// disk until a commit, so a crash before loses it, side files and all,
/// and it may be made again; opened again while known, it is the same
/// file; a handle dropped loses no write, the file forgotten once a commit
/// leaves it nothing to commit and no handle.
#[test]
fn a_file_made_is_made_at_its_commit() {
    let disk = disk();
    {
        let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
        shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap().write_all_at(b"lost", 0).unwrap();
        assert_eq!(listed(&shadows), [PathBuf::from(PATH)], "there as written");
    }
    assert!(!disk.exists(Path::new(PATH)).unwrap() && disk.exists(&side_path(Path::new(PATH), 0)).unwrap());
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    shadows.recover(Path::new("/d")).unwrap();
    assert_eq!(listed(&*disk), Vec::<PathBuf>::new(), "the side file thrown away");
    assert!(!shadows.exists(Path::new(PATH)).unwrap());
    shadows.recover(Path::new("/nowhere")).unwrap();
    let first = shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap();
    first.write_all_at(b"made", 0).unwrap();
    let again = shadows.open(Path::new(PATH), OpenMode::ReadWrite).unwrap();
    let mut back = vec![0_u8; 4];
    again.read_exact_at(&mut back, 0).unwrap();
    assert_eq!(back, b"made", "the same file");
    let code = |e: Result<Box<dyn DiskFile>>| e.err().map(|e| e.code);
    assert_eq!(code(shadows.open(Path::new(PATH), OpenMode::CreateNew)), Some(Code::Io), "there");
    assert_eq!(code(shadows.open(Path::new(OTHER), OpenMode::ReadWrite)), Some(Code::Io), "not there");
    drop((first, again));
    shadows.commit().unwrap();
    assert_eq!(content(&*disk), b"made", "no write lost");
    assert_eq!(lock_builtin(&shadows.files).len(), 0, "forgotten");
    assert_eq!(content_of(&shadows, PATH), b"made", "opened again from the disk");
}

/// Verifies: [DESIGN] RULES.md D30, files removed: one removed is gone as
/// written at once, refused to open and to remove again with SQLCODE
/// -915, but on the disk until a commit, so a crash before brings it back
/// as the last commit left it; after the commit it is gone. One never
/// committed is removed at once, side files and all.
#[test]
fn a_file_removed_is_removed_at_the_commit() {
    let disk = disk();
    {
        let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
        shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap().write_all_at(b"kept", 0).unwrap();
        shadows.commit().unwrap();
        shadows.remove(Path::new(PATH)).unwrap();
        assert!(!shadows.exists(Path::new(PATH)).unwrap() && listed(&shadows).is_empty(), "gone as written");
        assert_eq!(shadows.open(Path::new(PATH), OpenMode::ReadOnly).err().map(|e| e.code), Some(Code::Io));
        assert_eq!(shadows.remove(Path::new(PATH)).err().map(|e| e.code), Some(Code::Io));
        assert_eq!(content(&*disk), b"kept", "on the disk still");
    }
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    shadows.recover(Path::new("/d")).unwrap();
    assert_eq!(content_of(&shadows, PATH), b"kept", "back after the crash");
    shadows.remove(Path::new(PATH)).unwrap();
    shadows.commit().unwrap();
    assert!(!disk.exists(Path::new(PATH)).unwrap(), "gone at the commit");
    assert_eq!(shadows.remove(Path::new(OTHER)).err().map(|e| e.code), Some(Code::Io), "never there");
    let never = shadows.open(Path::new(OTHER), OpenMode::CreateNew).unwrap();
    never.write_all_at(b"never committed", 0).unwrap();
    shadows.remove(Path::new(OTHER)).unwrap();
    assert_eq!(listed(&*disk), Vec::<PathBuf>::new(), "removed at once, its side file too");
    drop(never);
    shadows.commit().unwrap();
    assert_eq!(listed(&*disk), Vec::<PathBuf>::new());
}

/// Verifies: [DESIGN] RULES.md D30, a file removed and made again before
/// a commit is the file emptied: the commit leaves only what was written
/// since. One made, sealed, then removed, is made by that batch's commit
/// and removed by the next; a handle kept to a file removed leaves it
/// known, with nothing to commit.
#[test]
fn a_file_removed_and_made_again_is_emptied() {
    let disk = disk();
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    let file = shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap();
    file.write_all_at(&[4_u8; 9_000], 0).unwrap();
    shadows.commit().unwrap();
    shadows.remove(Path::new(PATH)).unwrap();
    let made = shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap();
    assert_eq!(made.size().unwrap(), 0, "empty");
    made.write_all_at(b"new", 0).unwrap();
    shadows.commit().unwrap();
    assert_eq!(content(&*disk), b"new");
    let other = shadows.open(Path::new(OTHER), OpenMode::CreateNew).unwrap();
    other.write_all_at(b"sealed", 0).unwrap();
    shadows.seal().unwrap();
    shadows.remove(Path::new(OTHER)).unwrap();
    shadows.commit_sealed().unwrap();
    assert_eq!(content_of(&*disk, OTHER), b"sealed", "made by the sealed batch");
    shadows.commit().unwrap();
    assert!(!disk.exists(Path::new(OTHER)).unwrap(), "removed by the next");
    shadows.commit().unwrap();
    assert!(lock_builtin(&shadows.files).contains_key(Path::new(OTHER)), "a handle kept");
    assert_eq!(shadows.pending_bytes(), 0);
    let revived = shadows.open(Path::new(OTHER), OpenMode::CreateNew).unwrap();
    shadows.commit().unwrap();
    assert_eq!(content_of(&*disk, OTHER), b"", "made again, empty");
    drop((file, made, other, revived));
}

/// Verifies: [DESIGN] RULES.md D30, a crash after the commit block of a
/// batch that made a file, or removed one, before the file was touched:
/// recovering the directory makes it, as written, or removes it.
#[test]
fn a_crash_after_the_commit_block_makes_or_removes() {
    let disk = disk();
    {
        let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
        shadows.open(Path::new(OTHER), OpenMode::CreateNew).unwrap();
        shadows.commit().unwrap();
        shadows.remove(Path::new(OTHER)).unwrap();
        shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap().write_all_at(b"made", 0).unwrap();
        shadows.seal().unwrap();
        crash_after_commit_block(&shadows, PATH, |sealed| vec![Op::Limit(sealed.side.limit), Op::Size(sealed.size)]);
        crash_after_commit_block(&shadows, OTHER, |_| vec![Op::Remove]);
    }
    assert!(!disk.exists(Path::new(PATH)).unwrap() && disk.exists(Path::new(OTHER)).unwrap(), "untouched");
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    shadows.recover(Path::new("/d")).unwrap();
    assert_eq!(listed(&*disk), [PathBuf::from(PATH)]);
    assert_eq!(content(&*disk), b"made");
}

/// Verifies: [DESIGN] RULES.md D30, recovering a directory leaves a file
/// known alone: its side file is the one written to, not one to recover;
/// a file of a directory under it is recovered too.
#[test]
fn recovering_a_directory_leaves_files_known() {
    let disk = disk();
    disk.create_dir_all(Path::new("/d/sub")).unwrap();
    {
        let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
        shadows.open(Path::new("/d/sub/h"), OpenMode::CreateNew).unwrap().write_all_at(b"lost", 0).unwrap();
    }
    let shadows = ShadowFileSystem::new(Arc::clone(&disk), hash());
    let file = shadows.open(Path::new(PATH), OpenMode::CreateNew).unwrap();
    file.write_all_at(b"written", 0).unwrap();
    shadows.recover(Path::new("/d")).unwrap();
    assert!(disk.exists(&side_path(Path::new(PATH), 0)).unwrap(), "left alone");
    assert_eq!(disk.read_dir(Path::new("/d/sub")).unwrap(), Vec::<PathBuf>::new(), "recovered under it");
    shadows.commit().unwrap();
    assert_eq!(content(&*disk), b"written");
}
