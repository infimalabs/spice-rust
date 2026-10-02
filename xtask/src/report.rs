//! Frozen, development-scoped evidence acquisition for Making work land.

use std::ffi::OsString;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Read, Write};
use std::path::Path;
use std::process::Command;

use serde::{Deserialize, Serialize};
use sha2::{Digest as _, Sha256};
use spice::repository::Repository;
use wire::intent::{REPORT_FACT_KINDS, REPORT_READ_VERSION, ReportAnswer, ReportPin, ReportRead};
use wire::{Instant, Seq};

type Result<T, E = Box<dyn std::error::Error>> = std::result::Result<T, E>;

const ACQUISITION_PROGRESS_PAGES: usize = 25;
/// Frozen evidence format; version 1 retained lane socket capabilities.
const SNAPSHOT_VERSION: u32 = 2;

#[derive(Deserialize)]
struct PageReply {
    kind: String,
    body: ReportAnswer,
}

#[derive(Serialize, Deserialize)]
pub(crate) struct SnapshotManifest {
    pub version: u32,
    pub upper: Seq,
    pub horizon_at: Instant,
    pub pin: ReportPin,
    pub fact_count: usize,
    pub pages: usize,
    pub facts_sha256: String,
    pub oracle_sha256: String,
    pub oracle_acquisition: String,
    pub consumer_sha256: String,
    pub command_record_sha256: String,
}

mod continuity;
mod episodes;
mod git;
mod metrics;
mod privacy;
mod provenance;
mod publication;
mod reduce;
mod relationships;
mod render;

/// Run one step of the frozen report study: acquire, analyze, or render.
pub fn report(arguments: Vec<OsString>, output: &mut dyn Write) -> Result<()> {
    match arguments.as_slice() {
        [verb, upper, directory, oracle] if verb == "acquire" => {
            let upper: Seq = serde_json::from_str(upper.to_str().ok_or("upper must be UTF-8")?)?;
            acquire(upper, Path::new(directory), Path::new(oracle), output)
        }
        [verb, directory] if verb == "analyze" => reduce::analyze(Path::new(directory), output),
        [verb, directory, public, fonts, license] if verb == "render" => render::render(
            Path::new(directory),
            Path::new(public),
            Path::new(fonts),
            Path::new(license),
            output,
        ),
        _ => Err(concat!(
            "usage: spice dev xtask -- report <acquire UPPER PRIVATE BOARD-ORACLE|",
            "analyze PRIVATE|render PRIVATE PUBLIC FONTS LPPL>"
        )
        .into()),
    }
}

fn acquire(upper: Seq, directory: &Path, oracle: &Path, output: &mut dyn Write) -> Result<()> {
    let repository = Repository::discover(std::env::current_dir()?)?;
    let binary = spice::install::installed_binary(&repository);
    let metadata = fs::symlink_metadata(&binary)?;
    if !metadata.file_type().is_file() {
        return Err("installed appliance must be a regular non-symbolic file".into());
    }
    // Reuse the resident's receipt-owned asset admission; metadata alone is
    // insufficient before executing a credential-bearing native client.
    let _publication = spice::install::installed_canvas_source(&repository, &binary)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt as _, PermissionsExt as _};
        if metadata.permissions().mode() & 0o111 == 0 {
            return Err("installed appliance is not executable".into());
        }
        fs::DirBuilder::new().mode(0o700).create(directory)?;
    }
    #[cfg(not(unix))]
    fs::create_dir(directory)?;
    let oracle_copy = directory.join("independent-board-oracle.json");
    fs::copy(oracle, &oracle_copy)?;
    let mut facts = BufWriter::new(private_file(&directory.join("facts.ndjson"))?);
    let request_path = directory.join("page-request.json");
    let mut request = ReportRead {
        version: REPORT_READ_VERSION,
        after: None,
        upper,
        kinds: REPORT_FACT_KINDS.to_vec(),
        pin: None,
        command_at: None,
    };
    let (count, pages, horizon_at) = acquire_pages(
        &binary,
        repository.worktree_root(),
        &request_path,
        &mut request,
        &mut facts,
        output,
    )?;
    request.after = Some(upper);
    request.command_at = Some(serde_json::from_str("174102")?);
    let record_page = read_page(&binary, repository.worktree_root(), &request_path, &request)?;
    if record_page.pin != *request.pin.as_ref().ok_or("missing report pin")? {
        return Err("command-record read changed report generation".into());
    }
    let record = record_page
        .command_record
        .ok_or("retained correction command record is unavailable")?;
    write_json(&directory.join("correction-command-record.json"), &record)?;
    let manifest = SnapshotManifest {
        version: SNAPSHOT_VERSION,
        upper,
        horizon_at,
        pin: request.pin.ok_or("missing report generation pin")?,
        fact_count: count,
        pages,
        facts_sha256: hash_file(&directory.join("facts.ndjson"))?,
        oracle_sha256: hash_file(&oracle_copy)?,
        oracle_acquisition: "spice board --output FILE; independently captured cursor 797204"
            .to_owned(),
        consumer_sha256: hash_file(&std::env::current_exe()?)?,
        command_record_sha256: hash_file(&directory.join("correction-command-record.json"))?,
    };
    write_json(&directory.join("snapshot-manifest.json"), &manifest)?;
    fs::remove_file(request_path)?;
    writeln!(
        output,
        "frozen report evidence: {count} facts in {pages} pages at {upper}; {}",
        directory.display()
    )?;
    Ok(())
}

fn acquire_pages(
    binary: &Path,
    worktree: &Path,
    request_path: &Path,
    request: &mut ReportRead,
    facts: &mut BufWriter<File>,
    output: &mut dyn Write,
) -> Result<(usize, usize, Instant)> {
    let upper = request.upper;
    let mut count = 0;
    let mut pages = 0;
    let mut last_seen = None;
    let mut horizon_at = None;
    loop {
        let page = read_page(binary, worktree, request_path, request)?;
        if page.upper != upper || request.pin.as_ref().is_some_and(|pin| pin != &page.pin) {
            return Err("report page changed its horizon or decoder/schema/epoch pin".into());
        }
        if horizon_at.is_some_and(|at| at != page.horizon_at) {
            return Err("report horizon instant changed during acquisition".into());
        }
        for fact in &page.rows {
            if fact.seq() > upper
                || request.after.is_some_and(|after| fact.seq() <= after)
                || last_seen.is_some_and(|previous| fact.seq() <= previous)
                || !REPORT_FACT_KINDS.contains(&fact.kind())
            {
                return Err(
                    "report returned an unordered, duplicate, out-of-prefix or undeclared fact"
                        .into(),
                );
            }
            serde_json::to_writer(&mut *facts, fact)?;
            facts.write_all(b"\n")?;
            last_seen = Some(fact.seq());
            count += 1;
        }
        request.pin = Some(page.pin);
        horizon_at = Some(page.horizon_at);
        pages += 1;
        if pages % ACQUISITION_PROGRESS_PAGES == 0 {
            writeln!(
                output,
                "report acquisition: {pages} pages, {count} admitted facts"
            )?;
            output.flush()?;
        }
        match page.continuation {
            Some(next) => {
                if next >= upper
                    || request.after.is_some_and(|after| next <= after)
                    || last_seen.is_some_and(|last| next < last)
                {
                    return Err(
                        "report continuation failed to advance within the fixed prefix".into(),
                    );
                }
                request.after = Some(next);
            }
            None => break,
        }
    }
    facts.flush()?;
    facts.get_ref().sync_all()?;
    Ok((
        count,
        pages,
        horizon_at.ok_or("missing report horizon instant")?,
    ))
}

fn read_page(
    binary: &Path,
    worktree: &Path,
    path: &Path,
    request: &ReportRead,
) -> Result<ReportAnswer> {
    let mut file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(path)?;
    serde_json::to_writer(&mut file, request)?;
    file.sync_all()?;
    let result = Command::new(binary)
        .current_dir(worktree)
        .args(["dev", "report-page"])
        .arg(path)
        .arg("--json")
        .output()?;
    if !result.status.success() {
        return Err(format!(
            "native report read refused ({}): {} {}",
            result.status,
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        )
        .into());
    }
    let reply: PageReply = serde_json::from_slice(&result.stdout)?;
    if reply.kind != "reply" || reply.body.version != REPORT_READ_VERSION {
        return Err("native report response has an unsupported kind or version".into());
    }
    Ok(reply.body)
}

pub(crate) fn private_file(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    options.open(path)
}

pub(crate) fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = BufWriter::new(private_file(path)?);
    serde_json::to_writer_pretty(&mut file, value)?;
    file.write_all(b"\n")?;
    file.flush()?;
    file.get_ref().sync_all()?;
    Ok(())
}

pub(crate) fn hash_file(path: &Path) -> Result<String> {
    let mut file = File::open(path)?;
    let mut digest = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let count = file.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(format!("{:x}", digest.finalize()))
}
