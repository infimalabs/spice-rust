#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};

use gix::object::tree::EntryKind;
use serde::de::DeserializeOwned;
use wire::intent::TrustAnswer;

#[path = "support/fixture_root.rs"]
mod fixture_root;
#[path = "support/git_repository.rs"]
mod git_repository;
#[path = "support/readiness.rs"]
mod readiness;
#[path = "support/staged_app.rs"]
mod staged_app;

use readiness::Readiness;

struct Fixture {
    root: PathBuf,
    main: PathBuf,
    linked: PathBuf,
    canvas: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = fixture_root::claim(&std::env::temp_dir(), "trust-worktree-process");
        let main = root.join("main");
        let linked = root.join("linked");
        fs::create_dir(&main).expect("create main fixture worktree");
        git_repository::link(&main, &linked, "linked");
        staged_app::seed(&main);
        fs::write(main.join("spice-rust.toml"), "# main executable policy\n")
            .expect("write main policy");
        fs::write(
            linked.join("spice-rust.toml"),
            "# linked executable policy\n",
        )
        .expect("write linked policy");
        let main = fs::canonicalize(main).expect("canonicalize main worktree");
        let linked = fs::canonicalize(linked).expect("canonicalize linked worktree");
        let canvas = main.join(".git/.spice-rs/canvas");
        Self {
            root,
            main,
            linked,
            canvas,
        }
    }

    fn invoke(&self, worktree: &Path, arguments: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(arguments)
            .current_dir(worktree)
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
            .env_remove(wire::SpiceVariable::PreCommitSource.as_str())
            .env_remove(spice::install::INSTALLED_PRE_COMMIT_ENVIRONMENT)
            .output()
            .expect("spice trust command launches")
    }

    fn prepare_configured_pre_commit(&self) -> ConfiguredPreCommit {
        let executable = self.root.join("configured-gate");
        let marker = self.root.join("configured-ran");
        fs::write(&executable, "#!/bin/sh\nprintf configured > \"$1\"\n")
            .expect("write configured gate fixture");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755))
            .expect("make configured gate executable");
        let local = self.main.join(".git/.spice-rs/config.toml");
        fs::create_dir_all(local.parent().expect("local policy has a parent"))
            .expect("create local policy parent");
        let tracked_text = configured_policy(&executable, &marker);
        let local_text = "# shared local policy\n".to_owned();
        fs::write(&local, &local_text).expect("write shared local policy");
        for worktree in [&self.main, &self.linked] {
            fs::write(worktree.join("spice-rust.toml"), &tracked_text)
                .expect("write identical tracked policy");
            fs::create_dir_all(worktree.join("src")).expect("create fixture source directory");
            fs::write(worktree.join("src/lib.rs"), "pub fn baseline() {}\n")
                .expect("write baseline source");
        }

        let repository = open_repository(&self.main);
        let tree = snapshot_tree(&repository, &self.linked);
        let commit = commit_tree(&repository, tree);
        fs::write(
            self.main.join(".git/refs/heads/linked"),
            format!("{commit}\n"),
        )
        .expect("publish linked baseline ref");
        write_index(&repository, tree);
        write_index(&open_repository(&self.linked), tree);

        fs::write(self.linked.join("src/lib.rs"), "pub fn staged() {}\n")
            .expect("write sibling staged source");
        stage_all(&self.linked);
        ConfiguredPreCommit {
            marker,
            local,
            local_text,
            tracked_text,
        }
    }
}

struct ConfiguredPreCommit {
    marker: PathBuf,
    local: PathBuf,
    local_text: String,
    tracked_text: String,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove trust fixture");
    }
}

struct Resident {
    child: Option<Child>,
}

impl Resident {
    fn start(fixture: &Fixture) -> Self {
        let child = Command::new(env!("CARGO_BIN_EXE_spice"))
            .args(["--json", "up", "--foreground", "--port", "0"])
            .current_dir(&fixture.main)
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("shared resident starts");
        let mut resident = Self { child: Some(child) };
        let watched = fixture
            .canvas
            .parent()
            .expect("canvas has a runtime parent");
        fs::create_dir_all(watched).expect("runtime directory exists");
        match readiness::wait_until_ready(
            resident.child.as_mut().expect("resident child"),
            watched,
            readiness::ARRIVAL_HORIZON,
            "shared resident discovery",
            || fixture.canvas.is_file(),
        ) {
            Readiness::Ready => resident,
            Readiness::Exited => {
                let output = resident
                    .child
                    .take()
                    .expect("resident child")
                    .wait_with_output()
                    .expect("collect failed resident output");
                panic!(
                    "shared resident exited before readiness: {}",
                    spoken(&output)
                );
            }
        }
    }

    fn stop(mut self, fixture: &Fixture) {
        let down = fixture.invoke(&fixture.main, &["--json", "down"]);
        assert!(down.status.success(), "down failed: {}", spoken(&down));
        let output = self
            .child
            .take()
            .expect("resident child")
            .wait_with_output()
            .expect("resident exits");
        assert!(
            output.status.success(),
            "resident failed: {}",
            spoken(&output)
        );
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn linked_worktree_rulings_use_the_producer_across_resident_restart() {
    let fixture = Fixture::new();
    let resident = Resident::start(&fixture);

    let pended: TrustAnswer = reply(&fixture.invoke(&fixture.linked, &["trust", "pend"]));
    assert_eq!(pended.standing, wire::TrustState::Pended);
    let digest = pended.digest.to_string();
    let granted: TrustAnswer =
        reply(&fixture.invoke(&fixture.linked, &["trust", "grant", &digest]));
    assert_eq!(granted.digest, pended.digest);
    assert_eq!(granted.standing, wire::TrustState::Granted);

    resident.stop(&fixture);
    let restarted = Resident::start(&fixture);
    let revoked: TrustAnswer =
        reply(&fixture.invoke(&fixture.linked, &["trust", "revoke", &digest]));
    assert_eq!(revoked.digest, pended.digest);
    assert_eq!(revoked.standing, wire::TrustState::Revoked);
    let report: serde_json::Value = reply(&fixture.invoke(&fixture.linked, &["trust"]));
    assert_eq!(report["digest"], digest);
    assert_eq!(report["standing"], "revoked");
    restarted.stop(&fixture);
}

#[test]
fn linked_sibling_pre_commit_consumes_only_the_exact_shared_grant() {
    let fixture = Fixture::new();
    let resident = Resident::start(&fixture);
    let configured = fixture.prepare_configured_pre_commit();

    let pended: TrustAnswer = reply(&fixture.invoke(&fixture.main, &["trust", "pend"]));
    let digest = pended.digest.to_string();
    let granted: TrustAnswer = reply(&fixture.invoke(&fixture.main, &["trust", "grant", &digest]));
    assert_eq!(granted.standing, wire::TrustState::Granted);
    let repository = spice::repository::Repository::discover(&fixture.main)
        .expect("discover shared trust repository");
    let journal = repository
        .admit_state()
        .expect("admit shared trust state")
        .journal()
        .to_path_buf();
    let granted_bytes = fs::read(&journal).expect("read granted trust authority");

    let admitted = fixture.invoke(&fixture.linked, &["gate", "pre-commit"]);
    assert!(admitted.status.success(), "{}", spoken(&admitted));
    assert_eq!(
        fs::read(&configured.marker).expect("configured gate marker reads"),
        b"configured",
    );
    assert_eq!(
        fs::read(&journal).expect("reread trust after sibling gate"),
        granted_bytes,
        "configured pre-commit must not mutate shared trust",
    );
    let head = journal::JournalReader::open(&journal)
        .expect("open shared trust authority")
        .read(|read| read.trust_head(&pended.digest))
        .expect("read granted trust head")
        .expect("granted trust head exists");
    assert_eq!(
        head.body().worktree.as_deref(),
        Some(
            fixture
                .main
                .to_str()
                .expect("producer worktree path is UTF-8"),
        ),
        "the ruling retains producer provenance after sibling consumption",
    );

    fs::remove_file(&configured.marker).expect("clear configured gate marker");
    fs::write(
        fixture.linked.join("spice-rust.toml"),
        format!("{}# sibling tracked drift\n", configured.tracked_text),
    )
    .expect("write sibling tracked drift");
    stage_all(&fixture.linked);
    assert_untrusted_without_execution(&fixture, &configured, &journal, &granted_bytes, None);

    fs::write(
        fixture.linked.join("spice-rust.toml"),
        &configured.tracked_text,
    )
    .expect("restore sibling tracked policy");
    stage_all(&fixture.linked);
    fs::write(&configured.local, "# changed shared local policy\n")
        .expect("write changed local policy bytes");
    assert_untrusted_without_execution(&fixture, &configured, &journal, &granted_bytes, None);

    fs::write(&configured.local, &configured.local_text).expect("restore shared local policy");
    fs::write(
        fixture.linked.join("spice-rust.toml"),
        format!(
            "{}[policy]\npre_commit_builtins = false\npre_commit = [{{ label = 'changed', run = ['/bin/false'] }}]\n",
            gate::NARROWEST_FILE_CLASSES,
        ),
    )
    .expect("write changed tracked command words");
    stage_all(&fixture.linked);
    assert_untrusted_without_execution(
        &fixture,
        &configured,
        &journal,
        &granted_bytes,
        Some("/bin/false"),
    );

    fs::write(
        fixture.linked.join("spice-rust.toml"),
        &configured.tracked_text,
    )
    .expect("restore sibling tracked commands");
    fs::write(&configured.local, &configured.local_text).expect("restore shared local policy");
    resident.stop(&fixture);
}

fn assert_untrusted_without_execution(
    fixture: &Fixture,
    configured: &ConfiguredPreCommit,
    journal: &Path,
    granted_bytes: &[u8],
    command: Option<&str>,
) {
    let refused = fixture.invoke(&fixture.linked, &["gate", "pre-commit"]);
    assert!(!refused.status.success(), "{}", spoken(&refused));
    let rendered = spoken(&refused);
    assert!(rendered.contains("standing=absent"), "{rendered}");
    if let Some(command) = command {
        assert!(rendered.contains(command), "{rendered}");
    }
    assert!(
        !configured.marker.exists(),
        "untrusted policy executed its configured command",
    );
    assert_eq!(
        fs::read(journal).expect("reread trust after refusal"),
        granted_bytes,
        "an untrusted sibling gate mutated shared trust",
    );
}

fn configured_policy(executable: &Path, marker: &Path) -> String {
    format!(
        "{}[policy]\npre_commit_builtins = false\npre_commit = [{{ label = 'linked', run = [{}, {}] }}]\n",
        gate::NARROWEST_FILE_CLASSES,
        serde_json::to_string(&executable.display().to_string()).expect("encode executable path"),
        serde_json::to_string(&marker.display().to_string()).expect("encode marker path"),
    )
}

fn commit_tree(repository: &gix::Repository, tree: gix::ObjectId) -> gix::ObjectId {
    let identity = gix::actor::SignatureRef {
        name: "Spice Test".into(),
        email: "spice@example.invalid".into(),
        time: "0 +0000",
    };
    repository
        .commit_as(
            identity,
            identity,
            "refs/heads/main",
            "fixture baseline",
            tree,
            gix::commit::NO_PARENT_IDS,
        )
        .expect("commit trust fixture baseline")
        .detach()
}

fn stage_all(root: &Path) {
    let repository = open_repository(root);
    let tree = snapshot_tree(&repository, root);
    write_index(&repository, tree);
}

fn snapshot_tree(repository: &gix::Repository, root: &Path) -> gix::ObjectId {
    write_tree(repository, root, root)
}

fn write_tree(repository: &gix::Repository, root: &Path, directory: &Path) -> gix::ObjectId {
    let mut entries = fs::read_dir(directory)
        .expect("read trust fixture directory")
        .collect::<Result<Vec<_>, _>>()
        .expect("read trust fixture entries");
    entries.sort_by_key(fs::DirEntry::file_name);
    let mut tree_entries = entries
        .into_iter()
        .filter(|entry| directory != root || entry.file_name() != ".git")
        .map(|entry| {
            let path = entry.path();
            let (kind, oid) = if entry
                .file_type()
                .expect("inspect trust fixture entry")
                .is_dir()
            {
                (EntryKind::Tree, write_tree(repository, root, &path))
            } else {
                let blob = repository
                    .write_blob(fs::read(&path).expect("read trust fixture file"))
                    .expect("write trust fixture blob");
                (EntryKind::Blob, blob.detach())
            };
            gix::objs::tree::Entry {
                mode: kind.into(),
                filename: entry
                    .file_name()
                    .to_str()
                    .expect("trust fixture paths are UTF-8")
                    .into(),
                oid,
            }
        })
        .collect::<Vec<_>>();
    tree_entries.sort();
    repository
        .write_object(gix::objs::Tree {
            entries: tree_entries,
        })
        .expect("write trust fixture tree")
        .detach()
}

fn write_index(repository: &gix::Repository, tree: gix::ObjectId) {
    let mut index = repository
        .index_from_tree(&tree)
        .expect("derive trust fixture index from tree");
    index.set_path(repository.index_path());
    index
        .write(Default::default())
        .expect("write trust fixture index");
}

fn open_repository(root: &Path) -> gix::Repository {
    gix::open_opts(root, gix::open::Options::isolated()).expect("open trust fixture repository")
}

fn reply<T: DeserializeOwned>(output: &Output) -> T {
    assert!(
        output.status.success(),
        "command failed: {}",
        spoken(output)
    );
    wire::decode(&output.stdout).expect("command reply is typed")
}

fn spoken(output: &Output) -> String {
    format!(
        "status={:?} stdout={} stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    )
}
