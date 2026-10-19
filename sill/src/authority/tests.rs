use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::process;
use std::time::{SystemTime, UNIX_EPOCH};

use super::{
    create_private_directory, directory_link_form, is_link_like, link_directory, owner_mode,
    read_directory_link, rename_no_replace, set_owner_mode,
};

struct Scratch(PathBuf);

impl Scratch {
    fn new(stem: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock follows the epoch")
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("sill-authority-{stem}-{}-{nanos}", process::id()));
        create_private_directory(&root).expect("create scratch root");
        Self(root)
    }

    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        reopen(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn reopen(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if is_link_like(&metadata) {
        return;
    }
    if metadata.is_dir() {
        let _ = set_owner_mode(path, 0o700);
        for entry in fs::read_dir(path).into_iter().flatten().flatten() {
            reopen(&entry.path());
        }
    } else {
        let _ = set_owner_mode(path, 0o600);
    }
}

#[test]
fn no_replace_rename_refuses_any_existing_destination() {
    let scratch = Scratch::new("rename");
    let source = scratch.path("source");
    let file = scratch.path("file");
    let directory = scratch.path("directory");
    fs::create_dir(&source).expect("create source");
    fs::write(&file, b"kept").expect("create file destination");
    fs::create_dir(&directory).expect("create empty directory destination");

    for destination in [&file, &directory] {
        let refused = rename_no_replace(&source, destination).expect_err("destination exists");
        assert_eq!(refused.kind(), io::ErrorKind::AlreadyExists);
    }
    assert!(source.is_dir(), "a refused rename leaves the source");
    assert_eq!(fs::read(&file).expect("read file"), b"kept");

    let published = scratch.path("published");
    rename_no_replace(&source, &published).expect("absent destination publishes");
    assert!(published.is_dir());
    assert!(!source.exists());
}

#[test]
fn private_directories_are_owner_only_and_never_reused() {
    let scratch = Scratch::new("private");
    let directory = scratch.path("private");
    create_private_directory(&directory).expect("create private directory");
    assert_eq!(owner_mode(&directory).expect("read mode"), 0o700);

    let again = create_private_directory(&directory).expect_err("name exists");
    assert_eq!(again.kind(), io::ErrorKind::AlreadyExists);
    let file = scratch.path("file");
    fs::write(&file, b"").expect("create file");
    let over_file = create_private_directory(&file).expect_err("name exists");
    assert_eq!(over_file.kind(), io::ErrorKind::AlreadyExists);
}

#[test]
fn owner_modes_round_trip_and_refuse_shared_access() {
    let scratch = Scratch::new("modes");
    let directory = scratch.path("directory");
    let file = scratch.path("file");
    fs::create_dir(&directory).expect("create directory");
    fs::write(&file, b"bytes").expect("create file");

    for mode in [0o700, 0o500] {
        set_owner_mode(&directory, mode).expect("set directory mode");
        assert_eq!(owner_mode(&directory).expect("read mode"), mode);
    }
    for mode in [0o700, 0o600, 0o500, 0o400] {
        set_owner_mode(&file, mode).expect("set file mode");
        assert_eq!(owner_mode(&file).expect("read mode"), mode);
    }
    for shared in [0o755, 0o640, 0o4700] {
        let refused = set_owner_mode(&file, shared).expect_err("shared mode refused");
        assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);
    }
    assert_eq!(owner_mode(&file).expect("read mode"), 0o400);
}

#[test]
fn a_sealed_directory_refuses_new_entries() {
    let scratch = Scratch::new("sealed");
    let directory = scratch.path("sealed");
    create_private_directory(&directory).expect("create directory");
    fs::write(directory.join("kept"), b"kept").expect("create entry");
    set_owner_mode(&directory, 0o500).expect("seal directory");

    let refused = fs::write(directory.join("added"), b"").expect_err("sealed directory");
    assert_eq!(refused.kind(), io::ErrorKind::PermissionDenied);
    assert!(fs::remove_file(directory.join("kept")).is_err());
    assert_eq!(
        fs::read(directory.join("kept")).expect("read entry"),
        b"kept"
    );
}

#[test]
fn directory_links_record_their_target_and_read_as_links() {
    let scratch = Scratch::new("link");
    let target = scratch.path("target");
    let link = scratch.path("link");
    fs::create_dir(&target).expect("create target");
    fs::write(target.join("entry"), b"through").expect("create entry");

    link_directory(&target, &link).expect("create directory link");
    assert_eq!(
        read_directory_link(&link).expect("read link"),
        directory_link_form(&target).expect("link form")
    );
    assert!(is_link_like(
        &fs::symlink_metadata(&link).expect("link metadata")
    ));
    assert!(!is_link_like(
        &fs::symlink_metadata(&target).expect("target metadata")
    ));
    assert_eq!(
        fs::read(link.join("entry")).expect("read through link"),
        b"through"
    );

    let occupied = link_directory(&target, &link).expect_err("link name exists");
    assert_eq!(occupied.kind(), io::ErrorKind::AlreadyExists);
}

#[cfg(windows)]
#[test]
fn entries_created_in_a_private_directory_are_private() {
    let scratch = Scratch::new("inherit");
    let file = scratch.path("file");
    let directory = scratch.path("directory");
    fs::write(&file, b"").expect("create file");
    fs::create_dir(&directory).expect("create directory");
    fs::write(directory.join("nested"), b"").expect("create nested file");

    assert_eq!(owner_mode(&file).expect("file mode"), 0o600);
    assert_eq!(owner_mode(&directory).expect("directory mode"), 0o700);
    assert_eq!(
        owner_mode(&directory.join("nested")).expect("nested mode"),
        0o600
    );
}

#[cfg(windows)]
#[test]
fn junction_targets_must_be_local_drive_paths() {
    let scratch = Scratch::new("junction");
    let refused = link_directory(Path::new(r"\\server\share\x"), &scratch.path("link"))
        .expect_err("network target refused");
    assert_eq!(refused.kind(), io::ErrorKind::InvalidInput);
    assert!(!scratch.path("link").exists());
}

#[cfg(windows)]
#[test]
fn verbatim_and_drive_targets_record_one_form() {
    let scratch = Scratch::new("verbatim");
    let target = scratch.path("target");
    fs::create_dir(&target).expect("create target");
    let verbatim = fs::canonicalize(&target).expect("verbatim target");
    assert!(verbatim.as_os_str().to_string_lossy().starts_with(r"\\?\"));
    let link = scratch.path("link");

    link_directory(&verbatim, &link).expect("link verbatim target");
    let recorded = read_directory_link(&link).expect("read link");
    assert_eq!(recorded, directory_link_form(&target).expect("drive form"));
    assert_eq!(
        recorded,
        directory_link_form(&verbatim).expect("verbatim form")
    );
}
