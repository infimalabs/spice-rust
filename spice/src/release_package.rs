//! Canonical package admission and deterministic Git tree materialization.

use std::collections::BTreeMap;
use std::fmt;
#[cfg(test)]
use std::fs;
use std::fs::File;
use std::io::{self, Write as _};
use std::path::Path;
use std::process::{Command, Stdio};

use sha1::{Digest as _, Sha1};
use wire::manifest::{
    CANVAS_APP_FILES, InventoryEntry, ProductVersion, ReleaseManifest, ReleaseTarget,
};
use wire::{Digest, Domain};

const MAX_RELEASE_MEMBERS: usize = 32;
const MAX_RELEASE_MEMBER_BYTES: u64 = 512 * 1024 * 1024;
const MAX_RELEASE_BYTES: u64 = 1024 * 1024 * 1024;
const _: () =
    assert!(MAX_RELEASE_MEMBERS > 0 && MAX_RELEASE_MEMBER_BYTES > 0 && MAX_RELEASE_BYTES > 0);

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Preview {
    pub(super) identity: Digest,
    pub(super) git_tree: String,
}

#[derive(Debug)]
pub(super) enum PackageFailure {
    Moved(String),
    Environment(String),
}

impl fmt::Display for PackageFailure {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Moved(detail) | Self::Environment(detail) => detail.fmt(formatter),
        }
    }
}

struct LoadedTarget {
    target: ReleaseTarget,
    executable: proof::PackageMember,
    payload: proof::PackageMember,
    proof: proof::PackageMember,
    inventory: Vec<InventoryEntry>,
}

struct Loaded {
    notes: proof::PackageMember,
    manifest: proof::PackageMember,
    app: Vec<LoadedApp>,
    targets: Vec<LoadedTarget>,
}

struct LoadedApp {
    file: String,
    member: proof::PackageMember,
}

#[derive(Debug)]
pub(super) struct ReleaseIndex {
    preview: Preview,
    tree: proof::PackageTree,
}

impl ReleaseIndex {
    pub(super) fn admit(
        root: &Path,
        version: &ProductVersion,
        notes_identity: Digest,
        notes: &[u8],
        manifest: &[u8],
    ) -> Result<Self, PackageFailure> {
        validate_notes(&notes_identity, notes).map_err(PackageFailure::Moved)?;
        let mut budget = CaptureBudget::default();
        let notes = inline_member(
            "release/notes.md",
            0o644,
            notes,
            proof::PACKAGE_DOCUMENT_LIMIT,
            &mut budget,
        )?;
        let manifest_member = inline_member(
            "release/manifest.json",
            0o644,
            manifest,
            proof::PACKAGE_DOCUMENT_LIMIT,
            &mut budget,
        )?;
        let loaded = load(root, manifest, notes, manifest_member, &mut budget)?;
        let tree = proof::build_package(package_input(&loaded, version, notes_identity))
            .map_err(|error| PackageFailure::Moved(error.to_string()))?;
        budget.admit(
            "release/package.json",
            tree.package().bytes().len() as u64,
            proof::PACKAGE_DOCUMENT_LIMIT,
        )?;
        let preview = Preview {
            identity: tree.package().identity().clone(),
            git_tree: tree_identity(tree.members()).map_err(PackageFailure::Moved)?,
        };
        Ok(Self { preview, tree })
    }

    pub(super) const fn preview(&self) -> &Preview {
        &self.preview
    }

    pub(super) fn materialize(&self, root: &Path) -> Result<Preview, PackageFailure> {
        let git_tree =
            write_tree(root, self.tree.members()).map_err(PackageFailure::Environment)?;
        if git_tree != self.preview.git_tree {
            return Err(PackageFailure::Moved(
                "materialized Git tree changed from its admitted preview".to_owned(),
            ));
        }
        Ok(self.preview.clone())
    }

    pub(super) fn target_executable(
        &self,
        target: ReleaseTarget,
    ) -> Result<proof::PackageMember, PackageFailure> {
        let path = format!("targets/{target}/bin/spice");
        self.tree
            .members()
            .iter()
            .find(|member| member.path() == path)
            .cloned()
            .ok_or_else(|| {
                PackageFailure::Moved(format!("release manifest has no {target} target"))
            })
    }
}

fn validate_notes(identity: &Digest, notes: &[u8]) -> Result<(), String> {
    if identity == &Digest::compute(Domain::DistReleaseNotes, notes) {
        Ok(())
    } else {
        Err("release note bytes disagree with their identity".to_owned())
    }
}

fn package_input<'a>(
    loaded: &'a Loaded,
    version: &'a ProductVersion,
    notes_identity: Digest,
) -> proof::PackageInput<'a> {
    let app = loaded
        .app
        .iter()
        .map(|asset| proof::PackageAppInput::new(&asset.file, asset.member.clone()))
        .collect();
    let targets = loaded
        .targets
        .iter()
        .map(|target| {
            proof::PackageTargetInput::new(
                target.target,
                target.executable.clone(),
                target.payload.clone(),
                target.proof.clone(),
            )
        })
        .collect();
    proof::PackageInput::new(
        version,
        notes_identity,
        loaded.notes.clone(),
        loaded.manifest.clone(),
        app,
        targets,
    )
}

fn load(
    root: &Path,
    manifest: &[u8],
    notes: proof::PackageMember,
    manifest_member: proof::PackageMember,
    budget: &mut CaptureBudget,
) -> Result<Loaded, PackageFailure> {
    let document: ReleaseManifest =
        wire::decode(manifest).map_err(|error| PackageFailure::Moved(error.to_string()))?;
    let canonical =
        wire::encode(&document).map_err(|error| PackageFailure::Moved(error.to_string()))?;
    if canonical != manifest {
        return Err(PackageFailure::Moved(
            "release manifest is not canonical".to_owned(),
        ));
    }
    let stage = root.join("dist/stage");
    let mut targets = Vec::with_capacity(document.targets.len());
    for target in document.targets {
        let payload_path = stage.join(format!("objects/payload-{}.json", target.payload));
        let payload = file_member(
            &payload_path,
            &format!("targets/{}/payload.json", target.target),
            0o644,
            proof::PACKAGE_DOCUMENT_LIMIT,
            &format!("staged {} payload", target.target),
            budget,
        )?;
        let payload_bytes = payload
            .read_bounded(proof::PACKAGE_DOCUMENT_LIMIT)
            .map_err(|error| PackageFailure::Moved(error.to_string()))?;
        let admitted = proof::admit(&payload_bytes)
            .map_err(|error| PackageFailure::Moved(error.to_string()))?;
        if admitted.digest() != &target.payload || admitted.value().target != target.target {
            return Err(PackageFailure::Moved(format!(
                "staged {} payload disagrees with the release manifest",
                target.target
            )));
        }
        let executable = read_artifact(
            &stage,
            &admitted.value().inventory,
            "bin/spice",
            &format!("targets/{}/bin/spice", target.target),
            &format!("staged {} executable", target.target),
            budget,
        )?;
        let proof_path = stage.join(format!("objects/receipt-{}.json", target.receipt));
        let proof = file_member(
            &proof_path,
            &format!("targets/{}/proof.json", target.target),
            0o644,
            proof::PACKAGE_DOCUMENT_LIMIT,
            &format!("staged {} proof", target.target),
            budget,
        )?;
        targets.push(LoadedTarget {
            target: target.target,
            executable,
            payload,
            proof,
            inventory: admitted.value().inventory.clone(),
        });
    }
    let inventory = &targets
        .first()
        .expect("the current release manifest carries two targets")
        .inventory;
    let mut files = CANVAS_APP_FILES.to_vec();
    files.sort_unstable();
    let app = files
        .into_iter()
        .map(|file| {
            let path = format!("app/{file}");
            Ok(LoadedApp {
                file: file.to_owned(),
                member: read_artifact(
                    &stage,
                    inventory,
                    &path,
                    &path,
                    &format!("staged Canvas asset {file}"),
                    budget,
                )?,
            })
        })
        .collect::<Result<Vec<_>, PackageFailure>>()?;
    Ok(Loaded {
        notes,
        manifest: manifest_member,
        app,
        targets,
    })
}

fn read_artifact(
    stage: &Path,
    inventory: &[InventoryEntry],
    path: &str,
    member_path: &str,
    label: &str,
    budget: &mut CaptureBudget,
) -> Result<proof::PackageMember, PackageFailure> {
    let entry = inventory
        .iter()
        .find(|entry| entry.path == path)
        .ok_or_else(|| PackageFailure::Moved(format!("staged target payload has no {path}")))?;
    if entry.bytes > MAX_RELEASE_MEMBER_BYTES {
        return Err(PackageFailure::Moved(format!(
            "staged target payload member {path:?} declares {} bytes; maximum is {MAX_RELEASE_MEMBER_BYTES}",
            entry.bytes
        )));
    }
    let object = stage.join(format!("objects/{}-{:04o}", entry.digest, entry.mode));
    let member = file_member(
        &object,
        member_path,
        entry.mode,
        MAX_RELEASE_MEMBER_BYTES,
        label,
        budget,
    )?;
    if member.length() != entry.bytes || member.digest() != &entry.digest {
        return Err(PackageFailure::Moved(format!(
            "{label} object disagrees with its payload inventory"
        )));
    }
    Ok(member)
}

fn file_member(
    path: &Path,
    member_path: &str,
    mode: u32,
    limit: u64,
    label: &str,
    budget: &mut CaptureBudget,
) -> Result<proof::PackageMember, PackageFailure> {
    let file = File::open(path).map_err(|error| {
        let detail = format!("cannot read {label}: {error}");
        if error.kind() == std::io::ErrorKind::NotFound {
            PackageFailure::Moved(detail)
        } else {
            PackageFailure::Environment(detail)
        }
    })?;
    let length = file
        .metadata()
        .map_err(|error| PackageFailure::Environment(format!("cannot inspect {label}: {error}")))?
        .len();
    budget.admit(member_path, length, limit)?;
    proof::PackageMember::admit_file(member_path, mode, file, limit)
        .map_err(|error| PackageFailure::Moved(error.to_string()))
}

fn inline_member(
    path: &str,
    mode: u32,
    bytes: &[u8],
    limit: u64,
    budget: &mut CaptureBudget,
) -> Result<proof::PackageMember, PackageFailure> {
    budget.admit(path, bytes.len() as u64, limit)?;
    proof::PackageMember::admit_inline(path, mode, bytes, limit)
        .map_err(|error| PackageFailure::Moved(error.to_string()))
}

#[derive(Default)]
struct CaptureBudget {
    members: usize,
    bytes: u64,
}

impl CaptureBudget {
    fn admit(&mut self, path: &str, bytes: u64, role_limit: u64) -> Result<(), PackageFailure> {
        if role_limit == 0 || role_limit > MAX_RELEASE_MEMBER_BYTES {
            return Err(PackageFailure::Moved(format!(
                "release member {path:?} has invalid {role_limit}-byte role ceiling"
            )));
        }
        if self.members >= MAX_RELEASE_MEMBERS {
            return Err(PackageFailure::Moved(format!(
                "release index exceeds {MAX_RELEASE_MEMBERS} members"
            )));
        }
        if bytes > role_limit || bytes > MAX_RELEASE_MEMBER_BYTES {
            return Err(PackageFailure::Moved(format!(
                "release member {path:?} is {bytes} bytes; maximum is {}",
                role_limit.min(MAX_RELEASE_MEMBER_BYTES)
            )));
        }
        self.bytes = self
            .bytes
            .checked_add(bytes)
            .filter(|total| *total <= MAX_RELEASE_BYTES)
            .ok_or_else(|| {
                PackageFailure::Moved(format!(
                    "release index exceeds {MAX_RELEASE_BYTES} aggregate bytes"
                ))
            })?;
        self.members += 1;
        Ok(())
    }
}

enum Node<'a> {
    Directory(BTreeMap<String, Node<'a>>),
    File(&'a proof::PackageMember),
}

fn tree_identity(members: &[proof::PackageMember]) -> Result<String, String> {
    let root = tree(members)?;
    Ok(hex(&hash_node(&root)?))
}

fn write_tree(root: &Path, members: &[proof::PackageMember]) -> Result<String, String> {
    let tree = tree(members)?;
    write_node(root, &tree).map(|object| hex(&object))
}

fn tree(members: &[proof::PackageMember]) -> Result<Node<'_>, String> {
    let mut root = Node::Directory(BTreeMap::new());
    for member in members {
        insert(&mut root, member.path(), member)?;
    }
    Ok(root)
}

fn insert<'a>(
    node: &mut Node<'a>,
    path: &str,
    member: &'a proof::PackageMember,
) -> Result<(), String> {
    let (name, tail) = path
        .split_once('/')
        .map_or((path, None), |(name, tail)| (name, Some(tail)));
    let Node::Directory(children) = node else {
        return Err(format!("package path collides with a file: {path}"));
    };
    if let Some(tail) = tail {
        let child = children
            .entry(name.to_owned())
            .or_insert_with(|| Node::Directory(BTreeMap::new()));
        insert(child, tail, member)
    } else if children
        .insert(name.to_owned(), Node::File(member))
        .is_some()
    {
        Err(format!("package path occurs twice: {path}"))
    } else {
        Ok(())
    }
}

fn hash_node(node: &Node<'_>) -> Result<[u8; 20], String> {
    match node {
        Node::File(member) => hash_member(member),
        Node::Directory(children) => tree_body(children, hash_node).map(|body| hash("tree", &body)),
    }
}

fn write_node(root: &Path, node: &Node<'_>) -> Result<[u8; 20], String> {
    match node {
        Node::File(member) => write_member_object(root, member),
        Node::Directory(children) => {
            let body = tree_body(children, |child| write_node(root, child))?;
            write_object(root, "tree", &body)
        }
    }
}

fn tree_body(
    children: &BTreeMap<String, Node<'_>>,
    mut identity: impl FnMut(&Node<'_>) -> Result<[u8; 20], String>,
) -> Result<Vec<u8>, String> {
    let mut body = Vec::new();
    for (name, child) in children {
        let (mode, object) = match child {
            Node::Directory(_) => ("40000", identity(child)?),
            Node::File(member) if member.mode() == 0o644 => ("100644", identity(child)?),
            Node::File(member) if member.mode() == 0o755 => ("100755", identity(child)?),
            Node::File(member) => {
                return Err(format!(
                    "package member {name} has mode {:o}",
                    member.mode()
                ));
            }
        };
        body.extend_from_slice(mode.as_bytes());
        body.push(b' ');
        body.extend_from_slice(name.as_bytes());
        body.push(0);
        body.extend_from_slice(&object);
    }
    Ok(body)
}

fn hash_member(member: &proof::PackageMember) -> Result<[u8; 20], String> {
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", member.length()).as_bytes());
    member
        .copy_to(&mut HashWriter(&mut hash))
        .map_err(|error| error.to_string())?;
    Ok(hash.finalize().into())
}

struct HashWriter<'a>(&'a mut Sha1);

impl io::Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn hash(kind: &str, bytes: &[u8]) -> [u8; 20] {
    let mut hash = Sha1::new();
    hash.update(format!("{kind} {}\0", bytes.len()).as_bytes());
    hash.update(bytes);
    hash.finalize().into()
}

fn write_member_object(root: &Path, member: &proof::PackageMember) -> Result<[u8; 20], String> {
    let mut child = Command::new("git")
        .args(["hash-object", "-w", "-t", "blob", "--stdin"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot launch git hash-object: {error}"))?;
    let mut stdin = child.stdin.take().expect("piped Git stdin exists");
    let mut hash = Sha1::new();
    hash.update(format!("blob {}\0", member.length()).as_bytes());
    member
        .copy_to(&mut CopyHashWriter {
            writer: &mut stdin,
            hash: &mut hash,
        })
        .map_err(|error| error.to_string())?;
    drop(stdin);
    let output = child
        .wait_with_output()
        .map_err(|error| format!("cannot wait for git hash-object: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git hash-object refused: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let observed = String::from_utf8(output.stdout)
        .map_err(|error| format!("Git object id is not UTF-8: {error}"))?;
    let expected: [u8; 20] = hash.finalize().into();
    if observed.trim() != hex(&expected) {
        return Err("Git blob object identity disagrees with preview".to_owned());
    }
    Ok(expected)
}

struct CopyHashWriter<'a> {
    writer: &'a mut dyn io::Write,
    hash: &'a mut Sha1,
}

impl io::Write for CopyHashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        let written = self.writer.write(bytes)?;
        self.hash.update(&bytes[..written]);
        Ok(written)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

fn write_object(root: &Path, kind: &str, bytes: &[u8]) -> Result<[u8; 20], String> {
    let mut child = Command::new("git")
        .args(["hash-object", "-w", "-t", kind, "--stdin"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("cannot launch git hash-object: {error}"))?;
    child
        .stdin
        .take()
        .expect("piped Git stdin exists")
        .write_all(bytes)
        .map_err(|error| format!("cannot write Git {kind} object: {error}"))?;
    let output = child
        .wait_with_output()
        .map_err(|error| format!("cannot wait for git hash-object: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "git hash-object refused: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let observed = String::from_utf8(output.stdout)
        .map_err(|error| format!("Git object id is not UTF-8: {error}"))?;
    let expected = hash(kind, bytes);
    if observed.trim() != hex(&expected) {
        return Err(format!("Git {kind} object identity disagrees with preview"));
    }
    Ok(expected)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

#[cfg(test)]
mod tests;
