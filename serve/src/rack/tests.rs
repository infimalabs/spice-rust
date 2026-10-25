use std::io::Read as _;

use wire::manifest::renderer::{self, File, Kind, Member};
use wire::{Digest, Domain};

use super::{MAX_MEMBER_BYTES, Namespace, Rack, RackError, RackInput, SKILL};

const LOCAL_FLAGS: usize = 6;
const LOCAL_METHOD: usize = 8;
const LOCAL_CRC: usize = 14;
const LOCAL_STORED: usize = 18;
const LOCAL_NAME_LENGTH: usize = 26;
const LOCAL_NAME: usize = 30;
const CENTRAL_FLAGS: usize = 8;
const CENTRAL_METHOD: usize = 10;
const CENTRAL_OFFSET: usize = 42;
const CENTRAL_NAME_LENGTH: usize = 28;
const CENTRAL_NAME: usize = 46;
const END_HERE: usize = 8;
const END_COUNT: usize = 10;
const END_OFFSET: usize = 16;
const END_RECORD: usize = 22;
const GZIP_MTIME: usize = 4;
const GZIP_BODY: usize = 10;
const GZIP_SIZE: usize = 4;
const DATA_DESCRIPTOR: u16 = 1 << 3;
const DEFLATE: u16 = 8;

fn generation() -> Digest {
    Digest::compute(Domain::DistArtifact, b"canvas fixture generation")
}

/// One complete app generation, every shell file and one declared renderer
/// module, under the `app` namespace.
fn files(shell_generation: Option<&Digest>) -> Vec<(String, Vec<u8>)> {
    let module = b"export const renderer = 0;".to_vec();
    let declaration = renderer::encode(&[Member {
        file: File::new(Kind::Module, 0).expect("index is in bounds"),
        digest: Digest::compute(Domain::InstallArtifact, &module),
    }])
    .expect("declaration encodes");
    let named = shell_generation
        .map(|generation| format!(" data-app-generation=\"{generation}\""))
        .unwrap_or_default();
    let shell = format!(
        "<main id=\"app\"{named} {}=\"{declaration}\"></main>",
        renderer::ATTRIBUTE
    );
    let mut files = wire::manifest::APP_FILES
        .iter()
        .enumerate()
        .map(|(index, name)| {
            (
                (*name).to_owned(),
                format!("body {index} ").repeat(index + 1).into_bytes(),
            )
        })
        .collect::<Vec<_>>();
    files.push(("renderer-000.mjs".to_owned(), module));
    for (name, bytes) in &mut files {
        if name == wire::manifest::APP_HTML {
            *bytes = shell.clone().into_bytes();
        }
        *name = Namespace::App.member(name);
    }
    files
}

fn archive_of(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    Rack::write(
        files
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice())),
    )
    .expect("fixture archive writes")
}

fn archive() -> Vec<u8> {
    archive_of(&files(Some(&generation())))
}

fn u16_at(bytes: &[u8], at: usize) -> usize {
    usize::from(u16::from_le_bytes([bytes[at], bytes[at + 1]]))
}

fn u32_at(bytes: &[u8], at: usize) -> usize {
    u32::from_le_bytes(
        bytes[at..at + size_of::<u32>()]
            .try_into()
            .expect("four bytes"),
    ) as usize
}

fn put_u16(bytes: &mut [u8], at: usize, value: u16) {
    bytes[at..at + size_of::<u16>()].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(bytes: &mut [u8], at: usize, value: u32) {
    bytes[at..at + size_of::<u32>()].copy_from_slice(&value.to_le_bytes());
}

/// Local header offsets and stored ranges, in archive order.
fn locals(bytes: &[u8]) -> Vec<(usize, std::ops::Range<usize>)> {
    let directory = u32_at(bytes, bytes.len() - END_RECORD + END_OFFSET);
    let mut at = 0;
    let mut locals = Vec::new();
    while at < directory {
        let data = at + LOCAL_NAME + u16_at(bytes, at + LOCAL_NAME_LENGTH);
        let end = data + u32_at(bytes, at + LOCAL_STORED);
        locals.push((at, data..end));
        at = end;
    }
    locals
}

/// Central entry offsets, in directory order.
fn centrals(bytes: &[u8]) -> Vec<usize> {
    let mut at = u32_at(bytes, bytes.len() - END_RECORD + END_OFFSET);
    let mut centrals = Vec::new();
    while at < bytes.len() - END_RECORD {
        centrals.push(at);
        at += CENTRAL_NAME + u16_at(bytes, at + CENTRAL_NAME_LENGTH);
    }
    centrals
}

/// Rename one member in both headers; names must have equal length.
fn rename(bytes: &mut [u8], from: &str, to: &str) {
    let local = locals(bytes)
        .into_iter()
        .find(|(at, _)| bytes[at + LOCAL_NAME..].starts_with(from.as_bytes()))
        .expect("member exists")
        .0;
    let central = centrals(bytes)
        .into_iter()
        .find(|at| bytes[at + CENTRAL_NAME..].starts_with(from.as_bytes()))
        .expect("member exists");
    bytes[local + LOCAL_NAME..][..to.len()].copy_from_slice(to.as_bytes());
    bytes[central + CENTRAL_NAME..][..to.len()].copy_from_slice(to.as_bytes());
}

fn refused_member(result: Result<Rack<'_>, RackError>) -> String {
    match result {
        Err(RackError::Member { path, .. }) => path,
        other => panic!("expected a member refusal, got {other:?}"),
    }
}

#[test]
fn archives_are_a_pure_function_of_their_files() {
    let files = files(Some(&generation()));
    let mut reordered = files.clone();
    reordered.reverse();
    let bytes = archive_of(&files);
    assert_eq!(bytes, archive_of(&reordered));

    let admitted = Rack::admit_generation(&bytes, &generation()).expect("archive admits");
    assert_eq!(admitted.shell(), files[0].1.as_slice());
    let paths = admitted.members().map(|(path, _)| path).collect::<Vec<_>>();
    let mut sorted = paths.clone();
    sorted.sort_unstable();
    assert_eq!(paths, sorted);
    for (name, original) in &files {
        let member = admitted.member(name).expect("every file is a member");
        let mut inflated = Vec::new();
        flate2::read::GzDecoder::new(member.gzip())
            .read_to_end(&mut inflated)
            .expect("member is one gzip stream");
        assert_eq!(&inflated, original, "{name}");
        assert_eq!(member.size() as usize, original.len(), "{name}");
    }
}

#[test]
fn writing_refuses_repeated_unnormalized_and_unnamespaced_paths() {
    for repeated in ["docs/a.md", SKILL] {
        assert!(matches!(
            Rack::write([(repeated, b"a".as_slice()), (repeated, b"b".as_slice())]),
            Err(RackError::Member { path, .. }) if path == repeated
        ));
    }
    for path in [
        "/docs/a.md",
        "docs//b",
        "docs/./a",
        "docs/a/..",
        "docs/a\\b",
        "docs/a\0b",
        "a.md",
        "skill.md",
        "doc/a.md",
        "skill/SKILL.md",
    ] {
        assert!(
            matches!(
                Rack::write([(path, b"x".as_slice())]),
                Err(RackError::Member { .. })
            ),
            "{path:?}"
        );
    }
}

#[test]
fn stored_members_copy_the_stream_their_bytes_would_deflate_to() {
    let mut files = files(Some(&generation()));
    files.push(("docs/guide.md".to_owned(), b"# Guide\n".repeat(32)));
    files.push((SKILL.to_owned(), b"# Skill\n".to_vec()));
    let bytes = archive_of(&files);
    let admitted = Rack::admit(&bytes).expect("rack admits");
    let copied = Rack::write(
        admitted
            .members()
            .map(|(path, member)| (path, RackInput::Stored(member))),
    )
    .expect("stored members write");
    assert_eq!(copied, bytes);
    assert_eq!(
        admitted
            .namespace(Namespace::Docs)
            .map(|(path, _)| path)
            .collect::<Vec<_>>(),
        ["guide.md"]
    );
}

#[test]
fn namespaces_hold_their_declared_member_bounds_and_tool_manifests() {
    let with = |extra: &[(&str, &[u8])]| {
        let mut files = files(Some(&generation()));
        files.extend(
            extra
                .iter()
                .map(|(path, bytes)| ((*path).to_owned(), bytes.to_vec())),
        );
        Rack::write(
            files
                .iter()
                .map(|(path, bytes)| (path.as_str(), bytes.as_slice())),
        )
    };
    let docs = (0..=Namespace::Docs.max_members())
        .map(|index| (format!("docs/{index:03}.md"), b"d".as_slice()))
        .collect::<Vec<_>>();
    let docs = docs
        .iter()
        .map(|(path, bytes)| (path.as_str(), *bytes))
        .collect::<Vec<_>>();
    assert!(matches!(
        with(&docs),
        Err(RackError::Member { path, .. }) if path == "docs/256.md"
    ));
    assert!(matches!(
        with(&[("tool/grep/run.wasm", b"\0asm")]),
        Err(RackError::Member { path, .. }) if path == "tool/grep"
    ));
    assert!(matches!(
        with(&[("tool/run.wasm", b"\0asm")]),
        Err(RackError::Member { path, .. }) if path == "tool/run.wasm"
    ));
    let tool = with(&[
        ("tool/grep/manifest.json", b"{}"),
        ("tool/grep/run.wasm", b"\0asm"),
    ])
    .expect("a tool beside its manifest writes");
    Rack::admit(&tool).expect("a tool beside its manifest admits");
}

#[test]
fn admission_refuses_structure_outside_the_records() {
    let bytes = archive();
    let mut trailing = bytes.clone();
    trailing.push(0);
    let mut displaced = bytes.clone();
    let end = displaced.len() - END_RECORD;
    put_u32(&mut displaced, end + END_OFFSET, u32::MAX);
    for crafted in [
        &bytes[..bytes.len() - 1],
        &bytes[1..],
        &trailing[..],
        &displaced[..],
    ] {
        assert!(matches!(Rack::admit(crafted), Err(RackError::Structure(_))));
    }
    let mut counted = bytes;
    for field in [END_HERE, END_COUNT] {
        put_u16(&mut counted, end + field, u16::MAX);
    }
    assert!(matches!(Rack::admit(&counted), Err(RackError::Bound(_))));
}

#[test]
fn admission_refuses_header_disagreement_and_noncanonical_members() {
    let bytes = archive();
    let (first, data) = locals(&bytes)[0].clone();
    let central = centrals(&bytes);

    let mut disagreeing = bytes.clone();
    disagreeing[first + LOCAL_CRC] ^= 1;
    let mut described = bytes.clone();
    put_u16(&mut described, first + LOCAL_FLAGS, DATA_DESCRIPTOR);
    put_u16(&mut described, central[0] + CENTRAL_FLAGS, DATA_DESCRIPTOR);
    let mut deflated = bytes.clone();
    put_u16(&mut deflated, first + LOCAL_METHOD, DEFLATE);
    put_u16(&mut deflated, central[0] + CENTRAL_METHOD, DEFLATE);
    let mut stamped = bytes.clone();
    stamped[data.start + GZIP_MTIME] = 1;
    let mut oversized = bytes.clone();
    put_u32(&mut oversized, data.end - GZIP_SIZE, MAX_MEMBER_BYTES + 1);
    let mut overlapping = bytes.clone();
    put_u32(&mut overlapping, central[1] + CENTRAL_OFFSET, 0);
    for crafted in [disagreeing, described, deflated, stamped, oversized] {
        assert_eq!(refused_member(Rack::admit(&crafted)), "app/app.html");
    }
    assert_eq!(refused_member(Rack::admit(&overlapping)), "app/app.module");

    let mut corrupt = bytes;
    corrupt[data.start + GZIP_BODY] ^= 0xff;
    assert_eq!(refused_member(Rack::admit(&corrupt)), "app/app.html");
}

#[test]
fn admission_refuses_paths_outside_the_closed_inventory() {
    let bytes = archive();
    for path in [
        "app//ork.css",
        "app/./rk.css",
        "app/../k.css",
        "app/w\\rk.css",
        "app/w\0rk.css",
        "app/wo//.css",
        "app/work.cs2",
        "ark/work.css",
    ] {
        let mut renamed = bytes.clone();
        rename(&mut renamed, "app/work.css", path);
        let refused = refused_member(Rack::admit(&renamed));
        assert_eq!(refused.as_bytes(), path.as_bytes());
    }
    let mut repeated = bytes;
    rename(&mut repeated, "app/work.css", "app/app.html");
    assert_eq!(refused_member(Rack::admit(&repeated)), "app/app.html");

    let mut incomplete = files(Some(&generation()));
    incomplete.retain(|(name, _)| name != "app/renderer-000.mjs");
    assert_eq!(
        refused_member(Rack::admit(&archive_of(&incomplete))),
        "app/renderer-000.mjs"
    );
}

#[test]
fn admission_refuses_member_bounds_and_foreign_generations() {
    let mut inflated = archive();
    for (_, data) in locals(&inflated) {
        put_u32(&mut inflated, data.end - GZIP_SIZE, MAX_MEMBER_BYTES);
    }
    assert!(matches!(Rack::admit(&inflated), Err(RackError::Bound(_))));

    let unnamed = archive_of(&files(None));
    assert!(matches!(Rack::admit(&unnamed), Err(RackError::Shell(_))));

    let other = Digest::compute(Domain::DistArtifact, b"another generation");
    assert!(matches!(
        Rack::admit_generation(&archive(), &other),
        Err(RackError::Generation { found, .. }) if found == generation()
    ));
}
