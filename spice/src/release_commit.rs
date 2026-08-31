//! Deterministic fold and materialization of the release bump commit.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use wire::manifest::ProductVersion;

const VERSION_PATH: &str = "dist/VERSION";
const ACTOR_NAME: &str = "Spice Release";
const ACTOR_EMAIL: &str = "release@spice.local";

#[derive(Clone, Debug, Eq, PartialEq)]
pub(super) struct Fold {
    pub(super) source: String,
    pub(super) tree: String,
    pub(super) commit: String,
    timestamp: String,
}

#[derive(Debug)]
enum Node {
    Directory(BTreeMap<String, Node>),
    Object { mode: String, identity: Vec<u8> },
}

pub(super) fn project(root: &Path, source: &str, version: &ProductVersion) -> Result<Fold, String> {
    let mut tree = read_tree(root, source)?;
    let version_bytes = format!("{version}\n");
    let version_identity = object_identity(root, "blob", version_bytes.as_bytes())?;
    insert(
        &mut tree,
        VERSION_PATH,
        Node::Object {
            mode: "100644".to_owned(),
            identity: decode_identity(&version_identity)?,
        },
    )?;
    let tree = hash_tree(root, &tree)?;
    let (timestamp, timezone) = source_clock(root, source)?;
    let message = message(version);
    let body = commit_body(&tree, source, &timestamp, &timezone, &message);
    let commit = object_identity(root, "commit", &body)?;
    Ok(Fold {
        source: source.to_owned(),
        tree,
        commit,
        timestamp: format!("{timestamp} {timezone}"),
    })
}

pub(super) fn materialize(
    root: &Path,
    fold: &Fold,
    version: &ProductVersion,
) -> Result<String, String> {
    run_git(root, &["add", "--", VERSION_PATH])?;
    let tree = run_git(root, &["write-tree"])?;
    if tree != fold.tree {
        return Err(format!("release tree moved from {} to {tree}", fold.tree));
    }
    let message = message(version);
    let commit = run_git_input_with_identity(
        root,
        &["commit-tree", &tree, "-p", &fold.source, "-F", "-"],
        message.as_bytes(),
        fold,
    )?;
    if commit != fold.commit {
        return Err(format!(
            "release commit moved from {} to {commit}",
            fold.commit
        ));
    }
    run_git(root, &["update-ref", "HEAD", &fold.commit, &fold.source])?;
    run_git(root, &["rev-parse", "--verify", "HEAD^{commit}"])
}

fn read_tree(root: &Path, source: &str) -> Result<Node, String> {
    let output = git_output(root, &["ls-tree", "-rz", "--full-tree", source], None, None)?;
    parse_tree(&output)
}

fn parse_tree(output: &[u8]) -> Result<Node, String> {
    let mut root = Node::Directory(BTreeMap::new());
    for record in output
        .split(|byte| *byte == 0)
        .filter(|record| !record.is_empty())
    {
        let tab = record
            .iter()
            .position(|byte| *byte == b'\t')
            .ok_or_else(|| "Git tree record has no path separator".to_owned())?;
        let header = std::str::from_utf8(&record[..tab])
            .map_err(|error| format!("Git tree header is not UTF-8: {error}"))?;
        let path = std::str::from_utf8(&record[tab + 1..])
            .map_err(|error| format!("Git tree path is not UTF-8: {error}"))?;
        let mut fields = header.split_ascii_whitespace();
        let mode = fields
            .next()
            .ok_or_else(|| "Git tree record has no mode".to_owned())?;
        let _kind = fields
            .next()
            .ok_or_else(|| "Git tree record has no kind".to_owned())?;
        let identity = fields
            .next()
            .ok_or_else(|| "Git tree record has no identity".to_owned())?;
        if fields.next().is_some() {
            return Err("Git tree record has excess fields".to_owned());
        }
        insert(
            &mut root,
            path,
            Node::Object {
                mode: mode.to_owned(),
                identity: decode_identity(identity)?,
            },
        )?;
    }
    Ok(root)
}

fn insert(node: &mut Node, path: &str, value: Node) -> Result<(), String> {
    let (name, tail) = path
        .split_once('/')
        .map_or((path, None), |(name, tail)| (name, Some(tail)));
    let Node::Directory(children) = node else {
        return Err(format!("release tree path collides with a file: {path}"));
    };
    if name.is_empty() {
        return Err(format!("release tree path is empty: {path:?}"));
    }
    if let Some(tail) = tail {
        let child = children
            .entry(name.to_owned())
            .or_insert_with(|| Node::Directory(BTreeMap::new()));
        insert(child, tail, value)
    } else {
        children.insert(name.to_owned(), value);
        Ok(())
    }
}

fn hash_tree(root: &Path, node: &Node) -> Result<String, String> {
    let Node::Directory(children) = node else {
        return Err("release tree root is not a directory".to_owned());
    };
    let mut entries = children.iter().collect::<Vec<_>>();
    entries.sort_by(|(left_name, left), (right_name, right)| {
        sort_name(left_name, left).cmp(&sort_name(right_name, right))
    });
    let mut body = Vec::new();
    for (name, child) in entries {
        let (mode, identity) = match child {
            Node::Directory(_) => ("40000", decode_identity(&hash_tree(root, child)?)?),
            Node::Object { mode, identity } => (mode.as_str(), identity.clone()),
        };
        body.extend_from_slice(mode.as_bytes());
        body.push(b' ');
        body.extend_from_slice(name.as_bytes());
        body.push(0);
        body.extend_from_slice(&identity);
    }
    object_identity(root, "tree", &body)
}

fn sort_name(name: &str, node: &Node) -> Vec<u8> {
    let mut key = name.as_bytes().to_vec();
    if matches!(node, Node::Directory(_)) {
        key.push(b'/');
    }
    key
}

fn source_clock(root: &Path, source: &str) -> Result<(String, String), String> {
    let clock = run_git(root, &["show", "-s", "--format=%ct %ai", source])?;
    parse_source_clock(&clock)
}

fn parse_source_clock(clock: &str) -> Result<(String, String), String> {
    let mut fields = clock.split_ascii_whitespace();
    let timestamp = fields
        .next()
        .ok_or_else(|| "source commit has no timestamp".to_owned())?;
    let timezone = fields
        .last()
        .ok_or_else(|| "source commit has no timezone".to_owned())?;
    if !timestamp.bytes().all(|byte| byte.is_ascii_digit())
        || timezone.len() != 5
        || !matches!(timezone.as_bytes()[0], b'+' | b'-')
        || !timezone.as_bytes()[1..].iter().all(u8::is_ascii_digit)
    {
        return Err(format!("source commit has a noncanonical clock: {clock}"));
    }
    Ok((timestamp.to_owned(), timezone.to_owned()))
}

fn message(version: &ProductVersion) -> String {
    format!("release: bump to {version}\n")
}

fn commit_body(
    tree: &str,
    source: &str,
    timestamp: &str,
    timezone: &str,
    message: &str,
) -> Vec<u8> {
    format!(
        "tree {tree}\nparent {source}\nauthor {ACTOR_NAME} <{ACTOR_EMAIL}> {timestamp} {timezone}\ncommitter {ACTOR_NAME} <{ACTOR_EMAIL}> {timestamp} {timezone}\n\n{message}"
    )
    .into_bytes()
}

fn object_identity(root: &Path, kind: &str, bytes: &[u8]) -> Result<String, String> {
    let output = git_output(
        root,
        &["hash-object", "-t", kind, "--stdin"],
        Some(bytes),
        None,
    )?;
    String::from_utf8(output)
        .map_err(|error| format!("Git object identity is not UTF-8: {error}"))
        .map(|identity| identity.trim().to_owned())
}

fn decode_identity(identity: &str) -> Result<Vec<u8>, String> {
    if !identity.len().is_multiple_of(2) || identity.is_empty() {
        return Err(format!("Git object identity is not canonical: {identity}"));
    }
    identity
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).expect("ASCII hex pair is UTF-8");
            u8::from_str_radix(text, 16)
                .map_err(|_| format!("Git object identity is not canonical: {identity}"))
        })
        .collect()
}

fn run_git(root: &Path, arguments: &[&str]) -> Result<String, String> {
    let output = git_output(root, arguments, None, None)?;
    String::from_utf8(output)
        .map_err(|error| format!("Git output is not UTF-8: {error}"))
        .map(|text| text.trim().to_owned())
}

fn run_git_input_with_identity(
    root: &Path,
    arguments: &[&str],
    input: &[u8],
    fold: &Fold,
) -> Result<String, String> {
    let environment = [
        ("GIT_AUTHOR_NAME", ACTOR_NAME),
        ("GIT_AUTHOR_EMAIL", ACTOR_EMAIL),
        ("GIT_AUTHOR_DATE", fold.timestamp.as_str()),
        ("GIT_COMMITTER_NAME", ACTOR_NAME),
        ("GIT_COMMITTER_EMAIL", ACTOR_EMAIL),
        ("GIT_COMMITTER_DATE", fold.timestamp.as_str()),
    ];
    let output = git_output(root, arguments, Some(input), Some(&environment))?;
    String::from_utf8(output)
        .map_err(|error| format!("Git output is not UTF-8: {error}"))
        .map(|text| text.trim().to_owned())
}

fn git_output(
    root: &Path,
    arguments: &[&str],
    input: Option<&[u8]>,
    environment: Option<&[(&str, &str)]>,
) -> Result<Vec<u8>, String> {
    let mut command = Command::new("git");
    command
        .args(arguments)
        .current_dir(root)
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(environment) = environment {
        command.envs(environment.iter().copied());
    }
    if input.is_some() {
        command.stdin(Stdio::piped());
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("cannot launch git: {error}"))?;
    if let Some(input) = input {
        child
            .stdin
            .take()
            .expect("piped Git stdin exists")
            .write_all(input)
            .map_err(|error| format!("cannot write Git stdin: {error}"))?;
    }
    let output = child
        .wait_with_output()
        .map_err(|error| format!("cannot wait for git: {error}"))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        let detail = String::from_utf8_lossy(&output.stderr);
        Err(format!(
            "git {} exited {:?}: {}",
            arguments.join(" "),
            output.status.code(),
            detail.trim()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tree_parser_preserves_prefix_siblings_and_nested_paths() {
        let output = concat!(
            "100644 blob 1111111111111111111111111111111111111111\ta.c\0",
            "100644 blob 2222222222222222222222222222222222222222\ta/file\0",
        );
        let tree = parse_tree(output.as_bytes()).expect("canonical tree records parse");
        let Node::Directory(root) = tree else {
            panic!("tree root is a directory");
        };
        assert!(matches!(root.get("a.c"), Some(Node::Object { .. })));
        let Some(Node::Directory(nested)) = root.get("a") else {
            panic!("nested directory is retained");
        };
        assert!(matches!(nested.get("file"), Some(Node::Object { .. })));
    }

    #[test]
    fn tree_parser_refuses_malformed_records() {
        assert_eq!(
            parse_tree(b"100644 blob deadbeef missing-tab\0").unwrap_err(),
            "Git tree record has no path separator"
        );
        assert!(decode_identity("xyz").is_err());
    }

    #[test]
    fn source_clock_parser_admits_only_canonical_git_clocks() {
        assert_eq!(
            parse_source_clock("1700000000 2023-11-14 16:13:20 -0600").unwrap(),
            ("1700000000".to_owned(), "-0600".to_owned())
        );
        assert!(parse_source_clock("not-a-clock").is_err());
        assert!(parse_source_clock("1700000000 2023-11-14 16:13:20 UTC").is_err());
    }

    #[test]
    fn commit_body_binds_release_identity_and_source_clock() {
        let clock = "1700000000 +0000";
        let body = String::from_utf8(commit_body(
            "tree-id",
            "source-id",
            "1700000000",
            "+0000",
            "release: bump to 1.2.3\n",
        ))
        .expect("commit body is UTF-8");
        assert_eq!(
            body,
            format!(
                "tree tree-id\nparent source-id\nauthor {ACTOR_NAME} <{ACTOR_EMAIL}> {clock}\n\
                 committer {ACTOR_NAME} <{ACTOR_EMAIL}> {clock}\n\nrelease: bump to 1.2.3\n",
            )
        );
    }
}
