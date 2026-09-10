//! Canonical package admission and deterministic Git tree materialization.

use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use sha1::{Digest as _, Sha1};
use wire::manifest::{CANVAS_APP_FILES, ProductVersion, ReleaseManifest, ReleaseTarget};
use wire::{Digest, Domain};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Preview {
    pub(super) identity: Digest,
    pub(super) git_tree: String,
}

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
    executable: Vec<u8>,
    payload: Vec<u8>,
    proof: Vec<u8>,
}

struct Loaded {
    manifest: Vec<u8>,
    app: Vec<LoadedApp>,
    targets: Vec<LoadedTarget>,
}

struct LoadedApp {
    file: String,
    bytes: Vec<u8>,
}

pub(super) fn preview(
    root: &Path,
    version: &ProductVersion,
    notes_identity: Digest,
    notes: &[u8],
    manifest: &[u8],
) -> Result<Preview, PackageFailure> {
    validate_notes(&notes_identity, notes).map_err(PackageFailure::Moved)?;
    let loaded = load(root, manifest)?;
    let package =
        proof::build_package_index(package_input(&loaded, version, notes_identity, notes))
            .map_err(|error| PackageFailure::Moved(error.to_string()))?;
    let members = member_refs(&loaded, notes, package.bytes());
    Ok(Preview {
        identity: package.identity().clone(),
        git_tree: tree_identity(&members).map_err(PackageFailure::Moved)?,
    })
}

pub(super) fn materialize(
    root: &Path,
    version: &ProductVersion,
    notes_identity: Digest,
    notes: &[u8],
    manifest: &[u8],
) -> Result<Preview, PackageFailure> {
    validate_notes(&notes_identity, notes).map_err(PackageFailure::Moved)?;
    let loaded = load(root, manifest)?;
    let tree = proof::build_package(package_input(&loaded, version, notes_identity, notes))
        .map_err(|error| PackageFailure::Moved(error.to_string()))?;
    let members = tree
        .members()
        .iter()
        .map(|member| Member {
            path: member.path().to_owned(),
            mode: member.mode(),
            bytes: member.bytes(),
        })
        .collect::<Vec<_>>();
    let git_tree = write_tree(root, &members).map_err(PackageFailure::Environment)?;
    Ok(Preview {
        identity: tree.package().identity().clone(),
        git_tree,
    })
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
    notes: &'a [u8],
) -> proof::PackageInput<'a> {
    let app = loaded
        .app
        .iter()
        .map(|asset| proof::PackageAppInput::new(&asset.file, &asset.bytes))
        .collect();
    let targets = loaded
        .targets
        .iter()
        .map(|target| {
            proof::PackageTargetInput::new(
                target.target,
                &target.executable,
                &target.payload,
                &target.proof,
            )
        })
        .collect();
    proof::PackageInput::new(
        version,
        notes_identity,
        notes,
        &loaded.manifest,
        app,
        targets,
    )
}

fn load(root: &Path, manifest: &[u8]) -> Result<Loaded, PackageFailure> {
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
        let executable_path = stage
            .join("targets")
            .join(target.target.as_str())
            .join("bin/spice");
        let executable = read_bound(
            &executable_path,
            &format!("staged {} executable", target.target),
        )?;
        let payload_path = stage.join(format!("objects/payload-{}.json", target.payload));
        let payload = read_bound(&payload_path, &format!("staged {} payload", target.target))?;
        let proof_path = stage.join(format!("objects/receipt-{}.json", target.receipt));
        let proof = read_bound(&proof_path, &format!("staged {} proof", target.target))?;
        targets.push(LoadedTarget {
            target: target.target,
            executable,
            payload,
            proof,
        });
    }
    let payload: wire::manifest::TargetPayload = wire::decode(
        &targets
            .first()
            .expect("the current release manifest carries two targets")
            .payload,
    )
    .map_err(|error| PackageFailure::Moved(error.to_string()))?;
    let mut files = CANVAS_APP_FILES.to_vec();
    files.sort_unstable();
    let app = files
        .into_iter()
        .map(|file| {
            let path = format!("app/{file}");
            let entry = payload
                .inventory
                .iter()
                .find(|entry| entry.path == path)
                .ok_or_else(|| {
                    PackageFailure::Moved(format!(
                        "staged target payload has no Canvas asset {path}"
                    ))
                })?;
            let object = stage.join(format!("objects/{}-{:04o}", entry.digest, entry.mode));
            Ok(LoadedApp {
                file: file.to_owned(),
                bytes: read_bound(&object, &format!("staged Canvas asset {file}"))?,
            })
        })
        .collect::<Result<Vec<_>, PackageFailure>>()?;
    Ok(Loaded {
        manifest: manifest.to_vec(),
        app,
        targets,
    })
}

fn read_bound(path: &Path, label: &str) -> Result<Vec<u8>, PackageFailure> {
    fs::read(path).map_err(|error| {
        let detail = format!("cannot read {label}: {error}");
        if error.kind() == std::io::ErrorKind::NotFound {
            PackageFailure::Moved(detail)
        } else {
            PackageFailure::Environment(detail)
        }
    })
}

#[derive(Clone)]
struct Member<'a> {
    path: String,
    mode: u32,
    bytes: &'a [u8],
}

fn member_refs<'a>(loaded: &'a Loaded, notes: &'a [u8], package: &'a [u8]) -> Vec<Member<'a>> {
    let mut members = vec![
        Member {
            path: "release/manifest.json".to_owned(),
            mode: 0o644,
            bytes: &loaded.manifest,
        },
        Member {
            path: "release/notes.md".to_owned(),
            mode: 0o644,
            bytes: notes,
        },
        Member {
            path: "release/package.json".to_owned(),
            mode: 0o644,
            bytes: package,
        },
    ];
    members.extend(loaded.app.iter().map(|asset| Member {
        path: format!("app/{}", asset.file),
        mode: 0o644,
        bytes: &asset.bytes,
    }));
    for target in &loaded.targets {
        members.extend([
            Member {
                path: format!("targets/{}/bin/spice", target.target),
                mode: 0o755,
                bytes: &target.executable,
            },
            Member {
                path: format!("targets/{}/payload.json", target.target),
                mode: 0o644,
                bytes: &target.payload,
            },
            Member {
                path: format!("targets/{}/proof.json", target.target),
                mode: 0o644,
                bytes: &target.proof,
            },
        ]);
    }
    members
}

enum Node<'a> {
    Directory(BTreeMap<String, Node<'a>>),
    File { mode: u32, bytes: &'a [u8] },
}

fn tree_identity(members: &[Member<'_>]) -> Result<String, String> {
    let root = tree(members)?;
    Ok(hex(&hash_node(&root)?))
}

fn write_tree(root: &Path, members: &[Member<'_>]) -> Result<String, String> {
    let tree = tree(members)?;
    let mut write = |kind: &str, bytes: &[u8]| write_object(root, kind, bytes);
    hash_or_write_node(&tree, &mut write).map(|object| hex(&object))
}

fn tree<'a>(members: &[Member<'a>]) -> Result<Node<'a>, String> {
    let mut root = Node::Directory(BTreeMap::new());
    for member in members {
        insert(&mut root, &member.path, member.mode, member.bytes)?;
    }
    Ok(root)
}

fn insert<'a>(node: &mut Node<'a>, path: &str, mode: u32, bytes: &'a [u8]) -> Result<(), String> {
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
        insert(child, tail, mode, bytes)
    } else if children
        .insert(name.to_owned(), Node::File { mode, bytes })
        .is_some()
    {
        Err(format!("package path occurs twice: {path}"))
    } else {
        Ok(())
    }
}

fn hash_node(node: &Node<'_>) -> Result<[u8; 20], String> {
    hash_or_write_node(node, &mut |kind, bytes| Ok(hash(kind, bytes)))
}

fn hash_or_write_node(
    node: &Node<'_>,
    write: &mut impl FnMut(&str, &[u8]) -> Result<[u8; 20], String>,
) -> Result<[u8; 20], String> {
    match node {
        Node::File { bytes, .. } => write("blob", bytes),
        Node::Directory(children) => {
            let mut body = Vec::new();
            for (name, child) in children {
                let (mode, object) = match child {
                    Node::Directory(_) => ("40000", hash_or_write_node(child, write)?),
                    Node::File { mode: 0o644, .. } => ("100644", hash_or_write_node(child, write)?),
                    Node::File { mode: 0o755, .. } => ("100755", hash_or_write_node(child, write)?),
                    Node::File { mode, .. } => {
                        return Err(format!("package member {name} has mode {mode:o}"));
                    }
                };
                body.extend_from_slice(mode.as_bytes());
                body.push(b' ');
                body.extend_from_slice(name.as_bytes());
                body.push(0);
                body.extend_from_slice(&object);
            }
            write("tree", &body)
        }
    }
}

fn hash(kind: &str, bytes: &[u8]) -> [u8; 20] {
    let mut hash = Sha1::new();
    hash.update(format!("{kind} {}\0", bytes.len()).as_bytes());
    hash.update(bytes);
    hash.finalize().into()
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
