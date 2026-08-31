use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

use gix::bstr::BStr;
use gix::index::entry::{Flags, Mode, Stage, Stat};
use gix::object::tree::EntryKind;

const MAIN_BRANCH: &str = "refs/heads/main";
const FIXTURE_STAMP: &str = "1700000000 +0000";

pub(crate) fn initialize(root: &Path) {
    gix::ThreadSafeRepository::init_opts(
        root,
        gix::create::Kind::WithWorktree,
        gix::create::Options::default(),
        gix::open::Options::isolated(),
    )
    .expect("initialize fixture repository");
    fs::write(root.join(".git/HEAD"), format!("ref: {MAIN_BRANCH}\n"))
        .expect("select fixture main branch");
    writeln!(
        fs::OpenOptions::new()
            .append(true)
            .open(root.join(".git/config"))
            .expect("open fixture repository config"),
        "[core]\n\tlogAllRefUpdates = true"
    )
    .expect("enable fixture reflogs");
}

pub(crate) fn stage(root: &Path, paths: &[&str]) {
    let repository = open(root);
    let mut index = repository
        .index_or_load_from_head_or_empty()
        .expect("load fixture index")
        .into_owned();
    for path in paths {
        index.remove_entries(|_, candidate, _| candidate == path.as_bytes());
        if root.join(path).exists() {
            push_worktree_entry(&repository, &mut index, root, path);
        }
    }
    persist_index(&mut index);
}

pub(crate) fn stage_all(root: &Path) {
    let repository = open(root);
    let mut index = repository
        .index_or_load_from_head_or_empty()
        .expect("load fixture index")
        .into_owned();
    index.remove_entries(|_, _, _| true);
    for path in worktree_files(root) {
        push_worktree_entry(&repository, &mut index, root, &path);
    }
    persist_index(&mut index);
}

pub(crate) fn commit(root: &Path, message: &str) -> String {
    let repository = open(root);
    let index = repository.index().expect("open fixture index");
    let mut editor = repository.empty_tree().edit().expect("edit fixture tree");
    for entry in index.entries() {
        let kind = match entry.mode {
            Mode::FILE => EntryKind::Blob,
            Mode::FILE_EXECUTABLE => EntryKind::BlobExecutable,
            Mode::SYMLINK => EntryKind::Link,
            unsupported => panic!("unsupported fixture index mode {unsupported:?}"),
        };
        editor
            .upsert(entry.path(&index), kind, entry.id)
            .expect("insert fixture tree entry");
    }
    let tree = editor.write().expect("write fixture tree").detach();
    let parent = repository.head_id().ok().map(gix::Id::detach);
    let signature = gix::actor::SignatureRef {
        name: "Gate Fixture".into(),
        email: "gate@example.invalid".into(),
        time: FIXTURE_STAMP,
    };
    let reference = current_reference(&repository);
    let commit = repository
        .commit_as(signature, signature, reference, message, tree, parent)
        .expect("commit fixture tree")
        .detach();
    append_head_reflog(&repository, parent, commit, &format!("commit: {message}"));
    commit.to_string()
}

#[derive(Debug, Eq, PartialEq)]
pub(crate) enum MergeOutcome {
    Clean,
    Conflicted(Vec<String>),
}

pub(crate) fn checkout(root: &Path, branch: &str, create: bool) {
    checkout_from(root, branch, create, None);
}

pub(crate) fn checkout_from(root: &Path, branch: &str, create: bool, start: Option<&str>) {
    let current = start.map_or_else(|| head(root), str::to_owned);
    let reference = format!("refs/heads/{branch}");
    let reference_path = root.join(".git").join(&reference);
    if create {
        if let Some(parent) = reference_path.parent() {
            fs::create_dir_all(parent).expect("create fixture branch directory");
        }
        fs::write(&reference_path, format!("{current}\n")).expect("create fixture branch");
    }
    let target = read_object_id(&reference_path);
    fs::write(root.join(".git/HEAD"), format!("ref: {reference}\n"))
        .expect("select fixture branch");
    materialize_commit(root, target);
}

pub(crate) fn set_ref(root: &Path, reference: &str, id: &str) {
    let path = root.join(".git").join(reference);
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create fixture reference directory");
    }
    fs::write(path, format!("{id}\n")).expect("write fixture reference");
}

pub(crate) fn create_branch(root: &Path, branch: &str) {
    set_ref(root, &format!("refs/heads/{branch}"), &head(root));
}

pub(crate) fn set_symbolic_head(root: &Path, reference: &str) {
    fs::write(root.join(".git/HEAD"), format!("ref: {reference}\n"))
        .expect("write fixture symbolic HEAD");
}

pub(crate) fn pack_refs(root: &Path, prune: bool) {
    let heads = root.join(".git/refs/heads");
    let mut references = Vec::new();
    collect_loose_refs(&heads, &heads, &mut references);
    references.sort_by(|left, right| left.0.cmp(&right.0));
    let mut encoded = String::from("# pack-refs with: peeled fully-peeled sorted\n");
    for (name, id) in &references {
        encoded.push_str(&format!("{id} refs/heads/{name}\n"));
    }
    fs::write(root.join(".git/packed-refs"), encoded).expect("write fixture packed refs");
    if prune && heads.exists() {
        fs::remove_dir_all(&heads).expect("prune fixture loose refs");
        fs::create_dir_all(heads).expect("restore fixture heads directory");
    }
}

pub(crate) fn create_linked_worktree(root: &Path, linked: &Path, branch: &str) {
    let id = linked
        .file_name()
        .expect("linked worktree has a name")
        .to_string_lossy();
    let admin = root.join(".git/worktrees").join(id.as_ref());
    fs::create_dir_all(&admin).expect("create linked worktree administration");
    fs::create_dir(linked).expect("create linked fixture worktree");
    let reference = format!("refs/heads/{branch}");
    create_branch(root, branch);
    fs::write(
        linked.join(".git"),
        format!("gitdir: {}\n", admin.display()),
    )
    .expect("write linked worktree gitfile");
    fs::write(admin.join("commondir"), "../..\n").expect("write linked common directory");
    fs::write(
        admin.join("gitdir"),
        format!("{}\n", linked.join(".git").display()),
    )
    .expect("write linked gitfile backlink");
    fs::write(admin.join("HEAD"), format!("ref: {reference}\n"))
        .expect("write linked worktree HEAD");
    fs::copy(root.join(".git/index"), admin.join("index")).expect("copy linked worktree index");

    let repository = open(root);
    let entries = commit_entries(
        &repository,
        repository
            .head_id()
            .expect("fixture HEAD resolves")
            .detach(),
    );
    materialize_entries(linked, &repository, &entries);
}

pub(crate) fn remove_linked_worktree(root: &Path, linked: &Path) {
    let id = linked
        .file_name()
        .expect("linked worktree has a name")
        .to_string_lossy();
    fs::remove_dir_all(linked).expect("remove linked fixture worktree");
    fs::remove_dir_all(root.join(".git/worktrees").join(id.as_ref()))
        .expect("remove linked fixture administration");
}

pub(crate) fn split_index(root: &Path) {
    let index_path = root.join(".git/index");
    let shared = fs::read(&index_path).expect("read fixture index for splitting");
    let checksum = shared
        .get(shared.len().saturating_sub(20)..)
        .expect("fixture index carries a SHA-1 checksum");
    let checksum_id = gix::ObjectId::from_bytes_or_panic(checksum);
    fs::write(
        root.join(".git").join(format!("sharedindex.{checksum_id}")),
        &shared,
    )
    .expect("write fixture shared index");

    let mut split = Vec::new();
    split.extend_from_slice(b"DIRC");
    split.extend_from_slice(&2_u32.to_be_bytes());
    split.extend_from_slice(&0_u32.to_be_bytes());
    split.extend_from_slice(b"link");
    const EMPTY_BITMAP: [u8; 20] = [
        0, 0, 0, 0, // zero represented bits
        0, 0, 0, 1, // one run-length word
        0, 0, 0, 0, 0, 0, 0, 0, // empty run-length word
        0, 0, 0, 0, // run-length word offset
    ];
    split.extend_from_slice(&60_u32.to_be_bytes());
    split.extend_from_slice(checksum);
    split.extend_from_slice(&EMPTY_BITMAP);
    split.extend_from_slice(&EMPTY_BITMAP);
    let mut hasher = gix::hash::hasher(gix::hash::Kind::Sha1);
    hasher.update(&split);
    split.extend_from_slice(
        hasher
            .try_finalize()
            .expect("hash fixture split index")
            .as_slice(),
    );
    fs::write(index_path, split).expect("write fixture split index");
}

pub(crate) fn merge(root: &Path, branch: &str) -> MergeOutcome {
    let repository = open(root);
    let ours = repository
        .head_id()
        .expect("fixture HEAD resolves")
        .detach();
    let theirs_path = root.join(".git/refs/heads").join(branch);
    let theirs = read_object_id(&theirs_path);
    let base = merge_base(&repository, ours, theirs);
    let base_entries = commit_entries(&repository, base);
    let ours_entries = commit_entries(&repository, ours);
    let theirs_entries = commit_entries(&repository, theirs);
    let paths = base_entries
        .keys()
        .chain(ours_entries.keys())
        .chain(theirs_entries.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    let mut merged = BTreeMap::new();
    let mut conflicts = Vec::new();
    for path in paths {
        let base_entry = base_entries.get(&path);
        let ours_entry = ours_entries.get(&path);
        let theirs_entry = theirs_entries.get(&path);
        if ours_entry == theirs_entry {
            if let Some(entry) = ours_entry {
                merged.insert(path, entry.clone());
            }
            continue;
        }
        if ours_entry == base_entry {
            if let Some(entry) = theirs_entry {
                merged.insert(path, entry.clone());
            }
            continue;
        }
        if theirs_entry == base_entry {
            if let Some(entry) = ours_entry {
                merged.insert(path, entry.clone());
            }
            continue;
        }
        match merge_disjoint_text(&repository, base_entry, ours_entry, theirs_entry) {
            Some(entry) => {
                merged.insert(path, entry);
            }
            None => conflicts.push(path),
        }
    }

    let ours_tree = repository
        .find_commit(ours)
        .expect("find fixture HEAD commit")
        .tree_id()
        .expect("find fixture HEAD tree");
    let mut index = repository
        .index_from_tree(&ours_tree)
        .expect("expand fixture HEAD tree");
    index.remove_entries(|_, _, _| true);
    for (path, entry) in &merged {
        push_index_entry(&mut index, path, entry, Stage::Unconflicted);
    }
    for path in &conflicts {
        if let Some(entry) = base_entries.get(path) {
            push_index_entry(&mut index, path, entry, Stage::Base);
        }
        if let Some(entry) = ours_entries.get(path) {
            push_index_entry(&mut index, path, entry, Stage::Ours);
        }
        if let Some(entry) = theirs_entries.get(path) {
            push_index_entry(&mut index, path, entry, Stage::Theirs);
        }
    }
    persist_index(&mut index);

    let mut worktree = merged;
    for path in &conflicts {
        if let Some(entry) = ours_entries.get(path) {
            worktree.insert(path.clone(), entry.clone());
        }
    }
    materialize_entries(root, &repository, &worktree);
    fs::write(root.join(".git/MERGE_HEAD"), format!("{theirs}\n"))
        .expect("write fixture MERGE_HEAD");

    if conflicts.is_empty() {
        MergeOutcome::Clean
    } else {
        MergeOutcome::Conflicted(conflicts)
    }
}

pub(crate) fn head(root: &Path) -> String {
    open(root)
        .head_id()
        .expect("fixture HEAD resolves")
        .detach()
        .to_string()
}

pub(crate) fn head_tree(root: &Path) -> String {
    open(root)
        .head_commit()
        .expect("fixture HEAD commit resolves")
        .tree_id()
        .expect("fixture HEAD tree resolves")
        .detach()
        .to_string()
}

pub(crate) fn remove_from_index(root: &Path, paths: &[&str]) {
    let repository = open(root);
    let mut index = repository
        .index_or_load_from_head_or_empty()
        .expect("load fixture index")
        .into_owned();
    index.remove_entries(|_, candidate, _| paths.iter().any(|path| candidate == path.as_bytes()));
    persist_index(&mut index);
}

pub(crate) fn remove(root: &Path, paths: &[&str]) {
    for path in paths {
        let target = root.join(path);
        if target.is_dir() {
            fs::remove_dir_all(target).expect("remove fixture directory");
        } else if target.exists() {
            fs::remove_file(target).expect("remove fixture file");
        }
    }
    remove_from_index(root, paths);
}

pub(crate) fn move_path(root: &Path, source: &str, destination: &str) {
    let destination_path = root.join(destination);
    if let Some(parent) = destination_path.parent() {
        fs::create_dir_all(parent).expect("create fixture rename parent");
    }
    fs::rename(root.join(source), destination_path).expect("rename fixture path");
    stage(root, &[source, destination]);
}

pub(crate) fn staged_changes(root: &Path) -> String {
    let repository = open(root);
    let head = repository
        .head_commit()
        .expect("fixture HEAD commit resolves")
        .tree_id()
        .expect("fixture HEAD tree resolves");
    let before = repository
        .index_from_tree(&head)
        .expect("expand fixture HEAD tree");
    let after = repository.index().expect("open fixture index");
    changed_paths(&before, &after).join("\n")
}

pub(crate) fn worktree_changes(root: &Path) -> String {
    let repository = open(root);
    let index = repository.index().expect("open fixture index");
    let mut changed = Vec::new();
    for entry in index.entries() {
        let relative = entry.path(&index).to_string();
        let path = root.join(&relative);
        let differs = match fs::read(path) {
            Ok(bytes) => {
                repository
                    .write_blob(bytes)
                    .expect("hash fixture worktree blob")
                    .detach()
                    != entry.id
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
            Err(error) => panic!("read fixture worktree path {relative}: {error}"),
        };
        if differs {
            changed.push(relative);
        }
    }
    changed.join("\n")
}

fn open(root: &Path) -> gix::Repository {
    gix::open_opts(root, gix::open::Options::isolated()).expect("open fixture repository")
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct IndexEntry {
    id: gix::ObjectId,
    mode: Mode,
}

fn current_reference(repository: &gix::Repository) -> String {
    fs::read_to_string(repository.git_dir().join("HEAD"))
        .expect("read fixture HEAD")
        .strip_prefix("ref: ")
        .expect("fixture HEAD is symbolic")
        .trim()
        .to_owned()
}

fn append_head_reflog(
    repository: &gix::Repository,
    previous: Option<gix::ObjectId>,
    current: gix::ObjectId,
    message: &str,
) {
    let path = repository.git_dir().join("logs/HEAD");
    fs::create_dir_all(path.parent().expect("HEAD reflog has a parent"))
        .expect("create fixture HEAD reflog directory");
    let previous = previous.map_or_else(|| "0".repeat(40), |id| id.to_string());
    writeln!(
        fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .expect("open fixture HEAD reflog"),
        "{previous} {current} Gate Fixture <gate@example.invalid> {FIXTURE_STAMP}\t{message}"
    )
    .expect("append fixture HEAD reflog");
}

fn collect_loose_refs(root: &Path, directory: &Path, references: &mut Vec<(String, String)>) {
    if !directory.exists() {
        return;
    }
    for entry in fs::read_dir(directory).expect("read fixture loose refs") {
        let entry = entry.expect("read fixture loose ref");
        if entry
            .file_type()
            .expect("read fixture loose ref kind")
            .is_dir()
        {
            collect_loose_refs(root, &entry.path(), references);
        } else {
            let name = entry
                .path()
                .strip_prefix(root)
                .expect("fixture ref is below heads")
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "/");
            let id = fs::read_to_string(entry.path())
                .expect("read fixture loose ref value")
                .trim()
                .to_owned();
            references.push((name, id));
        }
    }
}

fn read_object_id(path: &Path) -> gix::ObjectId {
    let encoded = fs::read_to_string(path).expect("read fixture reference");
    gix::ObjectId::from_hex(encoded.trim().as_bytes()).expect("parse fixture object id")
}

fn merge_base(
    repository: &gix::Repository,
    ours: gix::ObjectId,
    theirs: gix::ObjectId,
) -> gix::ObjectId {
    let mut ours_ancestors = BTreeSet::new();
    let mut cursor = Some(ours);
    while let Some(id) = cursor {
        ours_ancestors.insert(id);
        cursor = first_parent(repository, id);
    }
    let mut cursor = Some(theirs);
    while let Some(id) = cursor {
        if ours_ancestors.contains(&id) {
            return id;
        }
        cursor = first_parent(repository, id);
    }
    panic!("fixture branches have no common ancestor")
}

fn first_parent(repository: &gix::Repository, id: gix::ObjectId) -> Option<gix::ObjectId> {
    repository
        .find_commit(id)
        .expect("find fixture commit")
        .parent_ids()
        .next()
        .map(gix::Id::detach)
}

fn commit_entries(
    repository: &gix::Repository,
    commit: gix::ObjectId,
) -> BTreeMap<String, IndexEntry> {
    let tree = repository
        .find_commit(commit)
        .expect("find fixture commit")
        .tree_id()
        .expect("find fixture commit tree");
    let index = repository
        .index_from_tree(&tree)
        .expect("expand fixture commit tree");
    index
        .entries()
        .iter()
        .map(|entry| {
            (
                entry.path(&index).to_string(),
                IndexEntry {
                    id: entry.id,
                    mode: entry.mode,
                },
            )
        })
        .collect()
}

fn merge_disjoint_text(
    repository: &gix::Repository,
    base: Option<&IndexEntry>,
    ours: Option<&IndexEntry>,
    theirs: Option<&IndexEntry>,
) -> Option<IndexEntry> {
    let (Some(base), Some(ours), Some(theirs)) = (base, ours, theirs) else {
        return None;
    };
    if base.mode != ours.mode || base.mode != theirs.mode {
        return None;
    }
    let base_bytes = blob(repository, base.id);
    let ours_bytes = blob(repository, ours.id);
    let theirs_bytes = blob(repository, theirs.id);
    let bytes = if let (Some(ours_prefix), Some(theirs_suffix)) = (
        ours_bytes.strip_suffix(base_bytes.as_slice()),
        theirs_bytes.strip_prefix(base_bytes.as_slice()),
    ) {
        [ours_prefix, &base_bytes, theirs_suffix].concat()
    } else if let (Some(theirs_prefix), Some(ours_suffix)) = (
        theirs_bytes.strip_suffix(base_bytes.as_slice()),
        ours_bytes.strip_prefix(base_bytes.as_slice()),
    ) {
        [theirs_prefix, &base_bytes, ours_suffix].concat()
    } else {
        return None;
    };
    Some(IndexEntry {
        id: repository
            .write_blob(bytes)
            .expect("write fixture merged blob")
            .detach(),
        mode: base.mode,
    })
}

fn blob(repository: &gix::Repository, id: gix::ObjectId) -> Vec<u8> {
    repository
        .find_blob(id)
        .expect("find fixture blob")
        .data
        .to_vec()
}

fn push_index_entry(index: &mut gix::index::File, path: &str, entry: &IndexEntry, stage: Stage) {
    index.dangerously_push_entry(
        Stat::default(),
        entry.id,
        Flags::from_stage(stage),
        entry.mode,
        BStr::new(path.as_bytes()),
    );
}

fn materialize_commit(root: &Path, commit: gix::ObjectId) {
    let repository = open(root);
    let entries = commit_entries(&repository, commit);
    let tree = repository
        .find_commit(commit)
        .expect("find fixture checkout commit")
        .tree_id()
        .expect("find fixture checkout tree");
    let mut index = repository
        .index_from_tree(&tree)
        .expect("expand fixture checkout tree");
    persist_index(&mut index);
    materialize_entries(root, &repository, &entries);
}

fn materialize_entries(
    root: &Path,
    repository: &gix::Repository,
    entries: &BTreeMap<String, IndexEntry>,
) {
    for entry in fs::read_dir(root).expect("read fixture root") {
        let entry = entry.expect("read fixture root entry");
        if entry.file_name() == ".git" {
            continue;
        }
        if entry.file_type().expect("read fixture entry kind").is_dir() {
            fs::remove_dir_all(entry.path()).expect("clear fixture directory");
        } else {
            fs::remove_file(entry.path()).expect("clear fixture file");
        }
    }
    for (relative, entry) in entries {
        let target = root.join(relative);
        if let Some(parent) = target.parent() {
            fs::create_dir_all(parent).expect("create fixture checkout parent");
        }
        fs::write(&target, blob(repository, entry.id)).expect("write fixture checkout path");
        set_executable(&target, entry.mode == Mode::FILE_EXECUTABLE);
    }
}

#[cfg(unix)]
fn set_executable(path: &Path, executable: bool) {
    use std::os::unix::fs::PermissionsExt as _;

    let mode = if executable { 0o755 } else { 0o644 };
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .expect("set fixture checkout permissions");
}

#[cfg(not(unix))]
fn set_executable(_path: &Path, _executable: bool) {}

fn persist_index(index: &mut gix::index::File) {
    index.sort_entries();
    index.remove_tree();
    index
        .write(gix::index::write::Options::default())
        .expect("write fixture index");
}

fn changed_paths(before: &gix::index::File, after: &gix::index::File) -> Vec<String> {
    let before = index_entries(before);
    let after = index_entries(after);
    before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter(|path| before.get(path) != after.get(path))
        .collect()
}

fn index_entries(index: &gix::index::File) -> BTreeMap<String, (gix::ObjectId, Mode)> {
    index
        .entries()
        .iter()
        .map(|entry| (entry.path(index).to_string(), (entry.id, entry.mode)))
        .collect()
}

fn push_worktree_entry(
    repository: &gix::Repository,
    index: &mut gix::index::File,
    root: &Path,
    relative: &str,
) {
    let path = root.join(relative);
    let metadata = fs::symlink_metadata(&path).expect("read fixture path metadata");
    let (bytes, mode) = if metadata.file_type().is_symlink() {
        (
            fs::read_link(&path)
                .expect("read fixture symlink")
                .to_string_lossy()
                .into_owned()
                .into_bytes(),
            Mode::SYMLINK,
        )
    } else {
        let mode = executable_mode(&metadata);
        (fs::read(&path).expect("read fixture file"), mode)
    };
    let id = repository
        .write_blob(&bytes)
        .expect("write fixture blob")
        .detach();
    let stat = Stat {
        size: u32::try_from(bytes.len()).unwrap_or(u32::MAX),
        ..Stat::default()
    };
    index.dangerously_push_entry(
        stat,
        id,
        Flags::empty(),
        mode,
        BStr::new(relative.as_bytes()),
    );
}

#[cfg(unix)]
fn executable_mode(metadata: &fs::Metadata) -> Mode {
    use std::os::unix::fs::PermissionsExt as _;

    if metadata.permissions().mode() & 0o111 == 0 {
        Mode::FILE
    } else {
        Mode::FILE_EXECUTABLE
    }
}

#[cfg(not(unix))]
fn executable_mode(_metadata: &fs::Metadata) -> Mode {
    Mode::FILE
}

fn worktree_files(root: &Path) -> Vec<String> {
    let mut pending = vec![PathBuf::new()];
    let mut files = Vec::new();
    while let Some(relative) = pending.pop() {
        for entry in fs::read_dir(root.join(&relative)).expect("read fixture worktree") {
            let entry = entry.expect("read fixture worktree entry");
            let name = entry.file_name();
            if relative.as_os_str().is_empty() && name == ".git" {
                continue;
            }
            let path = relative.join(name);
            if entry.file_type().expect("read fixture entry kind").is_dir() {
                pending.push(path);
            } else {
                files.push(
                    path.to_string_lossy()
                        .replace(std::path::MAIN_SEPARATOR, "/"),
                );
            }
        }
    }
    files.sort();
    files
}
