use std::collections::BTreeSet;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};
use wire::Refusal;

use super::{Environment, model::Vendor};

const THREAD_HEX_LEN: usize = 32;
const SEARCH_DEPTH: usize = 8;
const SEARCH_DIRECTORY_LIMIT: usize = 64;
const SEARCH_ENTRY_LIMIT: usize = 256;
const SEARCH_METADATA_LIMIT: usize = SEARCH_ENTRY_LIMIT;
const SEARCH_PATH_BYTES_LIMIT: usize = 64 * 1024;
const SEARCH_MATCH_LIMIT: usize = 64;
const SEARCH_ISSUE_LIMIT: usize = 32;
const SEARCH_DIAGNOSTIC_BYTES_LIMIT: usize = 64 * 1024;
const SEARCH_ELAPSED_LIMIT: std::time::Duration = std::time::Duration::from_secs(1);
const INDEX_ROW_LIMIT: usize = 128;
const INDEX_PATH_BYTES_LIMIT: usize = 64 * 1024;
const INDEX_DIAGNOSTIC_BYTES_LIMIT: usize = 8 * 1024;
const INDEX_ELAPSED_LIMIT: std::time::Duration = std::time::Duration::from_secs(1);
#[derive(Debug, Clone)]
pub(crate) struct ResolvedSource {
    pub(crate) path: PathBuf,
    pub(crate) vendor: Vendor,
    pub(crate) dialect: Option<listen::Dialect>,
    pub(crate) thread: Option<String>,
}

pub(crate) fn resolve_inputs(
    values: &[String],
    environment: &Environment,
) -> Result<Vec<ResolvedSource>, Refusal> {
    let resolved = if values.is_empty() {
        resolve_ambient(environment)?
    } else {
        let mut resolved = Vec::new();
        for value in values {
            resolved.extend(resolve_input(value, environment)?);
        }
        resolved
    };
    deduplicate(resolved)
}

pub(crate) fn resolve_thread(
    value: &str,
    environment: &Environment,
) -> Result<Vec<ResolvedSource>, Refusal> {
    let registry = spice::extension::registry()
        .expect("compiled extension declarations are validated by the test gate");
    resolve_thread_from_registry(value, environment, &registry)
}

fn resolve_thread_from_registry(
    value: &str,
    environment: &Environment,
    registry: &spice::extension::Registry<'_>,
) -> Result<Vec<ResolvedSource>, Refusal> {
    let thread = canonical_thread(value).ok_or_else(|| {
        refusal(
            "session::thread-invalid",
            Some(value.to_owned()),
            "pass one dashed or dashless hexadecimal UUID thread identity",
        )
    })?;
    let preferred = preferred_vendor(environment, registry);
    let mut vendors = registry.session_vendors();
    vendors.sort_by_key(|vendor| vendor.name() != preferred.name());
    let mut failures = Vec::new();
    for vendor in vendors {
        match find_thread(&thread, vendor, environment) {
            Ok(paths) => {
                let admitted = paths
                    .into_iter()
                    .map(|path| admit(path, Some(vendor), Some(thread.clone())))
                    .collect::<Result<Vec<_>, _>>()?;
                return deduplicate(admitted);
            }
            Err(detail) => failures.push(detail),
        }
    }
    Err(refusal(
        "session::thread-unresolved",
        Some(value.to_owned()),
        &format!(
            "restore the owning Codex state index or Claude project transcript, then retry; {}",
            failures.join("; ")
        ),
    ))
}

fn resolve_ambient(environment: &Environment) -> Result<Vec<ResolvedSource>, Refusal> {
    for vendor_environment in &environment.vendors {
        if let Some(raw) = &vendor_environment.identity {
            let value = raw.to_string_lossy();
            let thread = canonical_thread(&value).ok_or_else(|| {
                refusal(
                    "session::ambient-thread-invalid",
                    Some(vendor_environment.vendor.thread_id_environment().to_owned()),
                    "repair the live vendor thread identity or pass an explicit transcript path",
                )
            })?;
            let paths =
                find_thread(&thread, vendor_environment.vendor, environment).map_err(|detail| {
                    refusal(
                        "session::ambient-transcript-unresolved",
                        Some(thread.clone()),
                        &format!("pass the transcript path explicitly; {detail}"),
                    )
                })?;
            let admitted = paths
                .into_iter()
                .map(|path| admit(path, Some(vendor_environment.vendor), Some(thread.clone())))
                .collect::<Result<Vec<_>, _>>()?;
            return deduplicate(admitted);
        }
    }

    let current = std::env::current_dir().map_err(|error| {
        refusal(
            "session::cwd-unavailable",
            Some(error.to_string()),
            "restore the working directory or pass an absolute transcript path",
        )
    })?;
    let mut paths = fs::read_dir(&current)
        .map_err(|error| {
            refusal(
                "session::cwd-unreadable",
                Some(error.to_string()),
                "restore read access or pass an explicit transcript path",
            )
        })?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| transcript_extension(path))
        .collect::<Vec<_>>();
    paths.sort();
    if paths.is_empty() {
        return Err(refusal(
            "session::transcript-unspecified",
            None,
            "pass a .jsonl or .jsonl.gz path, or run inside a live Codex or Claude agent",
        ));
    }
    paths
        .into_iter()
        .map(|path| admit(path, None, None))
        .collect()
}

fn resolve_input(value: &str, environment: &Environment) -> Result<Vec<ResolvedSource>, Refusal> {
    let expanded = expand_home(value, environment);
    if expanded.exists() {
        return Ok(vec![admit(expanded, None, None)?]);
    }
    if canonical_thread(value).is_some() {
        return resolve_thread(value, environment);
    }
    Err(refusal(
        "session::transcript-missing",
        Some(expanded.display().to_string()),
        "restore the transcript or pass an existing .jsonl or .jsonl.gz path",
    ))
}

fn admit(
    path: PathBuf,
    declared: Option<&'static spice::extension::Vendor>,
    thread: Option<String>,
) -> Result<ResolvedSource, Refusal> {
    let metadata = fs::symlink_metadata(&path).map_err(|error| {
        refusal(
            "session::transcript-unavailable",
            Some(format!("{}: {error}", path.display())),
            "restore a readable regular transcript file and retry",
        )
    })?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(refusal(
            "session::transcript-not-regular",
            Some(path.display().to_string()),
            "pass a real regular .jsonl or .jsonl.gz file, not a link, directory, or device",
        ));
    }
    if !transcript_extension(&path) {
        return Err(refusal(
            "session::transcript-extension",
            Some(path.display().to_string()),
            "pass a transcript ending in .jsonl or .jsonl.gz",
        ));
    }
    let canonical = path.canonicalize().map_err(|error| {
        refusal(
            "session::transcript-unavailable",
            Some(format!("{}: {error}", path.display())),
            "restore stable read access to the transcript and retry",
        )
    })?;
    let (vendor, dialect) = declared.map_or((Vendor::UNKNOWN, None), |vendor| {
        (
            Vendor::declared(vendor),
            Some(vendor.session_gate().dialect()),
        )
    });
    Ok(ResolvedSource {
        path: canonical,
        vendor,
        dialect,
        thread,
    })
}

fn deduplicate(values: Vec<ResolvedSource>) -> Result<Vec<ResolvedSource>, Refusal> {
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    for value in values {
        if seen.insert(value.path.clone()) {
            rows.push(value);
        }
    }
    if rows.is_empty() {
        return Err(refusal(
            "session::transcript-unspecified",
            None,
            "pass at least one distinct transcript path",
        ));
    }
    Ok(rows)
}

fn preferred_vendor(
    environment: &Environment,
    registry: &spice::extension::Registry<'_>,
) -> &'static spice::extension::Vendor {
    let configured = environment
        .preferred_vendor
        .as_deref()
        .map(|value| value.to_string_lossy())
        .and_then(|name| registry.vendor(name.as_ref()));
    configured
        .or_else(|| {
            environment
                .vendors
                .iter()
                .find(|row| row.identity.is_some())
                .map(|row| row.vendor)
        })
        .or_else(|| registry.session_vendors().into_iter().next())
        .expect("thread resolution requires at least one declared vendor")
}

pub(super) fn find_thread(
    thread: &str,
    vendor: &'static spice::extension::Vendor,
    environment: &Environment,
) -> Result<Vec<PathBuf>, String> {
    let lookup = thread.replace('-', "");
    let vendor_environment = environment
        .vendors
        .iter()
        .find(|row| row.vendor == vendor)
        .ok_or_else(|| format!("vendor {} has no captured environment", vendor.name()))?;
    let layout = vendor.transcript_layout();
    let root = environment_root(
        vendor_environment.home.as_deref(),
        environment,
        layout.default_home,
    )?;
    let mut state_failure = None;
    if let Some(vendor::TranscriptIndex::CodexStateV5 { file }) = layout.state_index {
        let state = root.join(file);
        match fs::symlink_metadata(&state) {
            Ok(metadata) if metadata.file_type().is_file() => {
                match codex_state_path(&state, &lookup) {
                    Ok(Some(indexed)) => return Ok(vec![indexed]),
                    Ok(None) => {}
                    Err(StateIndexError::Invalid(error)) => {
                        state_failure = Some(index_issue(format!("{}: {error}", state.display())));
                    }
                    Err(StateIndexError::Refused(error)) => {
                        return Err(index_issue(format!("{}: {error}", state.display())));
                    }
                }
            }
            Ok(_) => {
                state_failure = Some(index_issue(format!(
                    "{} state database {} is not a real regular file",
                    vendor.label(),
                    state.display()
                )));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                state_failure = Some(index_issue(format!(
                    "missing {} state database {}",
                    vendor.label(),
                    state.display()
                )))
            }
            Err(error) => {
                state_failure = Some(index_issue(format!("{}: {error}", state.display())))
            }
        }
    }
    let discovered = match find_named_transcript(&root.join(layout.sessions_directory), &lookup) {
        Ok(matches) => matches,
        Err(error) => {
            return Err(state_failure.map_or_else(
                || error.to_string(),
                |failure| format!("{failure}; {error}"),
            ));
        }
    };
    let paths = discovered.into_iter().collect::<BTreeSet<_>>();
    if !paths.is_empty() {
        return Ok(paths.into_iter().collect());
    }
    Err(state_failure.unwrap_or_else(|| {
        format!(
            "{} thread {thread} is absent beneath {}",
            vendor.name(),
            root.join(layout.sessions_directory).display()
        )
    }))
}

enum StateIndexError {
    Invalid(String),
    Refused(String),
}

fn index_issue(issue: String) -> String {
    if issue.len() <= INDEX_DIAGNOSTIC_BYTES_LIMIT {
        issue
    } else {
        format!("thread index exceeded the {INDEX_DIAGNOSTIC_BYTES_LIMIT}-byte diagnostic limit")
    }
}

fn check_index_elapsed(started: std::time::Instant) -> Result<(), StateIndexError> {
    if started.elapsed() > INDEX_ELAPSED_LIMIT {
        Err(StateIndexError::Refused(format!(
            "thread index exceeded the {INDEX_ELAPSED_LIMIT:?} elapsed-work limit"
        )))
    } else {
        Ok(())
    }
}

fn codex_state_path(state: &Path, thread: &str) -> Result<Option<PathBuf>, StateIndexError> {
    let started = std::time::Instant::now();
    let connection = Connection::open_with_flags(
        state,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|error| StateIndexError::Invalid(error.to_string()))?;
    check_index_elapsed(started)?;
    let mut statement = connection
        .prepare("SELECT rollout_path FROM threads WHERE replace(lower(id), '-', '') = ?1")
        .map_err(|error| StateIndexError::Invalid(error.to_string()))?;
    check_index_elapsed(started)?;
    let mut rows = statement
        .query([thread])
        .map_err(|error| StateIndexError::Invalid(error.to_string()))?;
    check_index_elapsed(started)?;
    let mut found: Option<PathBuf> = None;
    let mut row_count = 0_usize;
    let mut path_bytes = 0_usize;
    while let Some(row) = rows
        .next()
        .map_err(|error| StateIndexError::Invalid(error.to_string()))?
    {
        check_index_elapsed(started)?;
        row_count = row_count.saturating_add(1);
        if row_count > INDEX_ROW_LIMIT {
            return Err(StateIndexError::Refused(format!(
                "thread index exceeded the {INDEX_ROW_LIMIT}-row lookup limit"
            )));
        }
        let spelling = row
            .get::<_, String>(0)
            .map_err(|error| StateIndexError::Invalid(error.to_string()))?;
        path_bytes = path_bytes.saturating_add(spelling.len());
        if path_bytes > INDEX_PATH_BYTES_LIMIT {
            return Err(StateIndexError::Refused(format!(
                "thread index exceeded the {INDEX_PATH_BYTES_LIMIT}-byte path limit"
            )));
        }
        let path = PathBuf::from(&spelling);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            StateIndexError::Invalid(format!(
                "indexed transcript {spelling:?} is unavailable: {error}"
            ))
        })?;
        if !metadata.file_type().is_file() || !transcript_extension(&path) {
            return Err(StateIndexError::Invalid(format!(
                "indexed transcript {spelling:?} is not a real regular JSONL file"
            )));
        }
        let canonical = path.canonicalize().map_err(|error| {
            StateIndexError::Invalid(format!(
                "indexed transcript {spelling:?} cannot be canonicalized: {error}"
            ))
        })?;
        if found.as_ref().is_some_and(|known| known != &canonical) {
            return Err(StateIndexError::Refused(format!(
                "thread index is ambiguous between {} and {}",
                found.as_ref().expect("checked path").display(),
                canonical.display()
            )));
        }
        found = Some(canonical);
    }
    check_index_elapsed(started)?;
    Ok(found)
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
enum TranscriptSearchIssue {
    Directory { path: PathBuf, detail: String },
    Entry { path: PathBuf, detail: String },
    FileType { path: PathBuf, detail: String },
    Depth { path: PathBuf },
    Bound { detail: String },
}

impl fmt::Display for TranscriptSearchIssue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Directory { path, detail } => {
                write!(
                    formatter,
                    "could not read directory {}: {detail}",
                    path.display()
                )
            }
            Self::Entry { path, detail } => write!(
                formatter,
                "could not read an entry beneath {}: {detail}",
                path.display()
            ),
            Self::FileType { path, detail } => {
                write!(formatter, "could not inspect {}: {detail}", path.display())
            }
            Self::Depth { path } => write!(
                formatter,
                "search depth {SEARCH_DEPTH} was reached at {}",
                path.display()
            ),
            Self::Bound { detail } => formatter.write_str(detail),
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
struct TranscriptSearchError {
    issues: Vec<TranscriptSearchIssue>,
}

impl fmt::Display for TranscriptSearchError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("transcript search was incomplete: ")?;
        for (index, issue) in self.issues.iter().enumerate() {
            if index > 0 {
                formatter.write_str("; ")?;
            }
            write!(formatter, "{issue}")?;
        }
        Ok(())
    }
}

fn find_named_transcript(root: &Path, thread: &str) -> Result<Vec<PathBuf>, TranscriptSearchError> {
    TranscriptSearch::new(root, thread).run()
}

struct TranscriptSearch<'a> {
    thread: &'a str,
    started: std::time::Instant,
    pending: BTreeSet<(usize, PathBuf)>,
    matches: Vec<PathBuf>,
    issues: Vec<TranscriptSearchIssue>,
    directories: usize,
    entries: usize,
    metadata: usize,
    path_bytes: usize,
    diagnostic_bytes: usize,
}

impl<'a> TranscriptSearch<'a> {
    fn new(root: &Path, thread: &'a str) -> Self {
        Self {
            thread,
            started: std::time::Instant::now(),
            pending: BTreeSet::from([(0, root.to_path_buf())]),
            matches: Vec::new(),
            issues: Vec::new(),
            directories: 0,
            entries: 0,
            metadata: 0,
            path_bytes: root.as_os_str().as_encoded_bytes().len(),
            diagnostic_bytes: 0,
        }
    }

    fn run(mut self) -> Result<Vec<PathBuf>, TranscriptSearchError> {
        while let Some((depth, directory)) = self.pending.pop_first() {
            self.check_elapsed()?;
            self.directories = self.directories.saturating_add(1);
            if self.directories > SEARCH_DIRECTORY_LIMIT {
                return Err(self.bound(format!(
                    "search exceeded the {SEARCH_DIRECTORY_LIMIT}-directory limit"
                )));
            }
            self.visit_directory(directory, depth)?;
        }
        self.issues.sort();
        if !self.issues.is_empty() {
            return Err(TranscriptSearchError {
                issues: self.issues,
            });
        }
        self.matches.sort();
        self.matches.dedup();
        self.check_elapsed()?;
        Ok(self.matches)
    }

    fn visit_directory(
        &mut self,
        directory: PathBuf,
        depth: usize,
    ) -> Result<(), TranscriptSearchError> {
        let entries = fs::read_dir(&directory);
        self.check_elapsed()?;
        let entries = match entries {
            Ok(entries) => entries,
            Err(error) => {
                return self.retain_issue(TranscriptSearchIssue::Directory {
                    path: directory,
                    detail: error.to_string(),
                });
            }
        };
        let mut readable = self.collect_entries(&directory, entries)?;
        readable.sort_by(|left, right| left.0.cmp(&right.0));
        for (_, entry, path) in readable {
            self.visit_entry(entry, path, depth)?;
        }
        Ok(())
    }

    fn collect_entries(
        &mut self,
        directory: &Path,
        entries: fs::ReadDir,
    ) -> Result<Vec<(std::ffi::OsString, fs::DirEntry, PathBuf)>, TranscriptSearchError> {
        let mut readable = Vec::new();
        for entry in entries {
            self.check_elapsed()?;
            self.entries = self.entries.saturating_add(1);
            if self.entries > SEARCH_ENTRY_LIMIT {
                return Err(self.bound(format!(
                    "search exceeded the {SEARCH_ENTRY_LIMIT}-entry limit"
                )));
            }
            match entry {
                Ok(entry) => {
                    let path = entry.path();
                    self.retain_path(&path)?;
                    readable.push((entry.file_name(), entry, path));
                }
                Err(error) => self.retain_issue(TranscriptSearchIssue::Entry {
                    path: directory.to_path_buf(),
                    detail: error.to_string(),
                })?,
            }
        }
        Ok(readable)
    }

    fn visit_entry(
        &mut self,
        entry: fs::DirEntry,
        path: PathBuf,
        depth: usize,
    ) -> Result<(), TranscriptSearchError> {
        self.check_elapsed()?;
        self.metadata = self.metadata.saturating_add(1);
        if self.metadata > SEARCH_METADATA_LIMIT {
            return Err(self.bound(format!(
                "search exceeded the {SEARCH_METADATA_LIMIT}-metadata limit"
            )));
        }
        let kind = entry.file_type();
        self.check_elapsed()?;
        let kind = match kind {
            Ok(kind) => kind,
            Err(error) => {
                return self.retain_issue(TranscriptSearchIssue::FileType {
                    path,
                    detail: error.to_string(),
                });
            }
        };
        if kind.is_symlink() {
            return Ok(());
        }
        if kind.is_dir() {
            return self.visit_subdirectory(path, depth);
        }
        let name = entry.file_name().to_string_lossy().replace('-', "");
        if kind.is_file() && transcript_extension(&path) && name.contains(self.thread) {
            self.retain_match(path)?;
        }
        Ok(())
    }

    fn visit_subdirectory(
        &mut self,
        path: PathBuf,
        depth: usize,
    ) -> Result<(), TranscriptSearchError> {
        if depth == SEARCH_DEPTH {
            self.retain_issue(TranscriptSearchIssue::Depth { path })
        } else {
            self.pending.insert((depth + 1, path));
            Ok(())
        }
    }

    fn retain_path(&mut self, path: &Path) -> Result<(), TranscriptSearchError> {
        self.path_bytes = self
            .path_bytes
            .saturating_add(path.as_os_str().as_encoded_bytes().len());
        if self.path_bytes > SEARCH_PATH_BYTES_LIMIT {
            Err(self.bound(format!(
                "search exceeded the {SEARCH_PATH_BYTES_LIMIT}-byte retained-path limit"
            )))
        } else {
            Ok(())
        }
    }

    fn retain_match(&mut self, path: PathBuf) -> Result<(), TranscriptSearchError> {
        if self.matches.len() == SEARCH_MATCH_LIMIT {
            return Err(self.bound(format!(
                "search exceeded the {SEARCH_MATCH_LIMIT}-match limit"
            )));
        }
        self.matches.push(path);
        Ok(())
    }

    fn retain_issue(&mut self, issue: TranscriptSearchIssue) -> Result<(), TranscriptSearchError> {
        let bytes = issue.to_string().len();
        if self.issues.len() == SEARCH_ISSUE_LIMIT
            || self.diagnostic_bytes.saturating_add(bytes) > SEARCH_DIAGNOSTIC_BYTES_LIMIT
        {
            return Err(self.bound(format!(
                "search exceeded its {SEARCH_ISSUE_LIMIT}-issue or \
                 {SEARCH_DIAGNOSTIC_BYTES_LIMIT}-byte diagnostic limit"
            )));
        }
        self.diagnostic_bytes += bytes;
        self.issues.push(issue);
        Ok(())
    }

    fn check_elapsed(&self) -> Result<(), TranscriptSearchError> {
        if self.started.elapsed() <= SEARCH_ELAPSED_LIMIT {
            Ok(())
        } else {
            Err(self.bound(format!(
                "search exceeded the {SEARCH_ELAPSED_LIMIT:?} elapsed-work limit"
            )))
        }
    }

    fn bound(&self, detail: String) -> TranscriptSearchError {
        let mut issues = self.issues.clone();
        issues.push(TranscriptSearchIssue::Bound { detail });
        issues.sort();
        issues.dedup();
        TranscriptSearchError { issues }
    }
}

fn environment_root(
    configured: Option<&Path>,
    environment: &Environment,
    fallback: &str,
) -> Result<PathBuf, String> {
    if let Some(value) = configured {
        return Ok(value.to_path_buf());
    }
    environment
        .home
        .as_ref()
        .map(|home| home.join(fallback))
        .ok_or_else(|| format!("the vendor home and HOME are absent for {fallback}"))
}

fn expand_home(value: &str, environment: &Environment) -> PathBuf {
    let Some(rest) = value.strip_prefix("~/") else {
        return PathBuf::from(value);
    };
    environment
        .home
        .as_ref()
        .map_or_else(|| PathBuf::from(value), |home| home.join(rest))
}

fn transcript_extension(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text.ends_with(".jsonl") || text.ends_with(".jsonl.gz")
}

pub(crate) fn canonical_thread(value: &str) -> Option<String> {
    let compact = value.trim().replace('-', "").to_ascii_lowercase();
    if compact.len() != THREAD_HEX_LEN || !compact.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    uuid::Uuid::parse_str(&compact)
        .ok()
        .map(wire::CanonicalUuid::new)
        .map(|identity| identity.to_string())
}

fn refusal(reason: &str, observed: Option<String>, detail: &str) -> Refusal {
    Refusal::bare(
        wire::RefusalDiagnostic::new(
            reason.to_owned(),
            Some("session-transcript".to_owned()),
            Some("immutable transcript evidence readable as JSONL".to_owned()),
            observed,
        )
        .with_detail(detail),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt as _;

    use super::{
        Environment, SEARCH_DEPTH, SEARCH_ENTRY_LIMIT, canonical_thread, find_named_transcript,
        find_thread, resolve_inputs, resolve_thread_from_registry,
    };
    use crate::session_command::VendorEnvironment;

    const THREAD: &str = "0123456789abcdef0123456789abcdef";
    const THREAD_DASHED: &str = "01234567-89ab-cdef-0123-456789abcdef";
    const THREAD_UPPER: &str = "01234567-89AB-CDEF-0123-456789ABCDEF";
    const FIXTURE_DIALECT: vendor::Vendor = vendor::Vendor::named("fixture", &vendor::CODEX);
    const FIXTURE_VENDOR: spice::extension::Vendor =
        spice::extension::Vendor::new(&FIXTURE_DIALECT, fixture_configuration, 2);
    const FIXTURE_DECLARATIONS: &[spice::extension::Declaration] =
        &[spice::extension::Declaration {
            namespace: spice::extension::Namespace::Vendor,
            name: "fixture",
            provider: "fixture",
            value: spice::extension::Value::Vendor(&FIXTURE_VENDOR),
        }];
    static FIXTURE: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn dashed_and_compact_thread_inputs_share_one_canonical_presentation() {
        let expected = Some(THREAD_DASHED);
        assert_eq!(canonical_thread(THREAD).as_deref(), expected);
        assert_eq!(canonical_thread(THREAD_DASHED).as_deref(), expected);
        assert_eq!(canonical_thread(THREAD_UPPER).as_deref(), expected);
    }

    fn fixture_configuration(root: &spice::config::Root) -> vendor::LaunchValues<'_> {
        let configured = root.vendor().codex();
        vendor::LaunchValues {
            binary: configured.binary().as_str(),
            model: configured.model(),
            effort: configured.effort(),
            personality: configured.personality(),
            fast_mode: Some(configured.fast_mode()),
            model_context_window: Some(configured.model_context_window()),
            auto_compact_window_tokens: None,
        }
    }

    struct Scratch(PathBuf);

    impl Scratch {
        fn new(label: &str) -> Self {
            let serial = FIXTURE.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "spice-session-source-{}-{serial}-{label}",
                std::process::id()
            ));
            fs::create_dir_all(&path).expect("session source fixture root");
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).expect("remove session source fixture");
        }
    }

    fn claude_environment(home: &Path) -> Environment {
        Environment {
            vendors: vec![VendorEnvironment {
                vendor: &spice::extension::CLAUDE,
                identity: None,
                home: Some(home.to_path_buf()),
            }],
            preferred_vendor: None,
            home: None,
        }
    }

    fn codex_environment(home: &Path) -> Environment {
        Environment {
            vendors: vec![VendorEnvironment {
                vendor: &spice::extension::CODEX,
                identity: None,
                home: Some(home.to_path_buf()),
            }],
            preferred_vendor: None,
            home: None,
        }
    }

    fn write_codex_state(home: &Path, paths: &[&Path]) {
        let state = rusqlite::Connection::open(home.join("state_5.sqlite"))
            .expect("Codex state fixture opens");
        state
            .execute("CREATE TABLE threads (id TEXT, rollout_path TEXT)", [])
            .expect("Codex state fixture schema");
        for path in paths {
            state
                .execute(
                    "INSERT INTO threads (id, rollout_path) VALUES (?1, ?2)",
                    (THREAD, path.to_string_lossy().as_ref()),
                )
                .expect("Codex state fixture row");
        }
    }

    #[test]
    fn indexed_codex_paths_survive_a_depth_limited_sessions_tree() {
        let scratch = Scratch::new("indexed-depth");
        let indexed = scratch.path().join("indexed.jsonl");
        fs::write(&indexed, b"\n").expect("indexed transcript exists");
        write_codex_state(scratch.path(), &[&indexed]);
        let mut directory = scratch.path().join("sessions");
        for depth in 0..=SEARCH_DEPTH {
            directory = directory.join(format!("level-{depth}"));
        }
        fs::create_dir_all(&directory).expect("deep Codex session branch exists");

        assert_eq!(
            find_thread(
                THREAD,
                &spice::extension::CODEX,
                &codex_environment(scratch.path()),
            )
            .expect("incomplete discovery cannot erase indexed authority"),
            [indexed.canonicalize().expect("indexed path canonicalizes")],
        );
    }

    #[test]
    fn ambiguous_codex_index_refuses_without_walking_the_complete_sessions_tree() {
        let scratch = Scratch::new("indexed-ambiguous");
        let sessions = scratch.path().join("sessions");
        fs::create_dir_all(&sessions).expect("Codex sessions directory exists");
        let first = sessions.join("first.jsonl");
        let second = sessions.join("second.jsonl");
        for path in [&first, &second] {
            fs::write(path, b"\n").expect("Codex transcript exists");
        }
        write_codex_state(scratch.path(), &[&first, &second]);

        let error = find_thread(
            THREAD,
            &spice::extension::CODEX,
            &codex_environment(scratch.path()),
        )
        .expect_err("a second canonical index path is bounded ambiguity");
        assert!(error.contains("ambiguous"), "{error}");
        assert!(!error.contains(" absent "), "{error}");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_directory_is_not_reported_as_an_absent_claude_thread() {
        let scratch = Scratch::new("unreadable");
        let projects = scratch.path().join("projects");
        let denied = projects.join("denied");
        fs::create_dir_all(&denied).expect("denied branch exists");
        let original = fs::metadata(&denied)
            .expect("denied branch metadata")
            .permissions();
        let mut blocked = original.clone();
        blocked.set_mode(0o0);
        fs::set_permissions(&denied, blocked).expect("deny fixture branch reads");

        let read_result = fs::read_dir(&denied);
        let lookup = find_thread(
            THREAD,
            &spice::extension::CLAUDE,
            &claude_environment(scratch.path()),
        );
        fs::set_permissions(&denied, original).expect("restore fixture permissions");
        let read_error = read_result
            .expect_err("fixture must force a real read_dir failure")
            .to_string();
        let error = lookup.expect_err("an incomplete walk cannot claim absence");
        assert!(error.contains(&denied.display().to_string()), "{error}");
        assert!(error.contains(&read_error), "{error}");
        assert!(!error.contains(" absent "), "{error}");
    }

    #[test]
    fn directory_beyond_the_search_bound_is_not_reported_as_absence() {
        let scratch = Scratch::new("depth");
        let projects = scratch.path().join("projects");
        let mut directory = projects.clone();
        for depth in 0..=SEARCH_DEPTH {
            directory = directory.join(format!("level-{depth}"));
        }
        fs::create_dir_all(&directory).expect("deep transcript branch exists");
        fs::write(directory.join(format!("thread-{THREAD}.jsonl")), b"\n")
            .expect("deep transcript exists");

        let error = find_thread(
            THREAD,
            &spice::extension::CLAUDE,
            &claude_environment(scratch.path()),
        )
        .expect_err("a bounded-out hit cannot become absence");

        assert!(
            error.contains(&format!("search depth {SEARCH_DEPTH} was reached")),
            "{error}"
        );
        assert!(error.contains(&directory.display().to_string()), "{error}");
        assert!(!error.contains(" absent "), "{error}");
    }

    #[test]
    fn codex_keeps_state_failure_beside_the_incomplete_session_path() {
        let scratch = Scratch::new("codex-depth");
        let sessions = scratch.path().join("sessions");
        let mut directory = sessions.clone();
        for depth in 0..=SEARCH_DEPTH {
            directory = directory.join(format!("level-{depth}"));
        }
        fs::create_dir_all(&directory).expect("deep Codex session branch exists");

        let error = find_thread(
            THREAD,
            &spice::extension::CODEX,
            &codex_environment(scratch.path()),
        )
        .expect_err("Codex cannot erase either failed lookup path");

        assert!(
            error.contains(&scratch.path().join("state_5.sqlite").display().to_string()),
            "{error}"
        );
        assert!(error.contains(&directory.display().to_string()), "{error}");
        assert!(!error.contains(" absent "), "{error}");
    }

    #[test]
    fn complete_walk_returns_every_matching_transcript_in_order() {
        let scratch = Scratch::new("complete");
        let root = scratch.path().join("sessions");
        fs::create_dir_all(&root).expect("session root exists");
        let older = root.join(format!("a-{THREAD}.jsonl"));
        let newest = root.join(format!("z-{THREAD}.jsonl.gz"));
        fs::write(&older, b"\n").expect("older transcript exists");
        fs::write(&newest, b"\n").expect("newest transcript exists");
        for index in 2..SEARCH_ENTRY_LIMIT {
            fs::write(root.join(format!("unrelated-{index}")), b"")
                .expect("bounded unrelated entry exists");
        }
        let exact = find_named_transcript(&root, THREAD).expect("exact entry bound succeeds");
        assert_eq!(exact, vec![older, newest]);
        fs::write(root.join("one-over"), b"").expect("one-over entry exists");
        let error =
            find_named_transcript(&root, THREAD).expect_err("one entry beyond the bound refuses");
        let expected = format!("{SEARCH_ENTRY_LIMIT}-entry limit");
        assert!(error.to_string().contains(&expected));
    }

    #[test]
    fn a_third_declaration_reaches_session_discovery_without_another_mode_arm() {
        let scratch = Scratch::new("third-declaration");
        let sessions = scratch.path().join("sessions");
        fs::create_dir_all(&sessions).expect("fixture sessions exist");
        let transcript = sessions.join(format!("rollout-{THREAD}.jsonl"));
        fs::write(
            &transcript,
            concat!(
                r#"{"timestamp":"2026-08-09T01:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-fixture"}}"#,
                "\n",
            ),
        )
        .expect("fixture transcript exists");
        let registry = spice::extension::Registry::discover(FIXTURE_DECLARATIONS)
            .expect("third declaration is coherent");
        let fixture = registry.vendor("fixture").expect("fixture value resolves");
        let environment = Environment::new(
            vec![VendorEnvironment::new(
                fixture,
                Some(std::ffi::OsString::from(THREAD)),
                Some(scratch.path().as_os_str().to_owned()),
            )],
            Some(std::ffi::OsString::from("fixture")),
            None,
        );

        let resolved = resolve_thread_from_registry(THREAD, &environment, &registry)
            .expect("third declaration resolves its transcript");
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].vendor, super::Vendor::declared(fixture));
        assert_eq!(resolved[0].dialect, Some(listen::Dialect::Codex));
        assert_eq!(resolved[0].thread.as_deref(), Some(THREAD_DASHED));
        assert_eq!(
            resolved[0].path,
            transcript.canonicalize().expect("fixture path")
        );

        let matches = crate::cli::command()
            .try_get_matches_from(["spice", "session", "briefing", "--deadline-seconds", "5"])
            .expect("briefing syntax parses");
        let leaf = crate::cli::selected_matches(&matches);
        let wire::Outcome::Reply(reply) =
            crate::session_command::run(leaf, &["session", "briefing"], &environment)
        else {
            panic!("third declaration reaches briefing");
        };
        let crate::session_command::model::Report::Recall {
            sources, window, ..
        } = reply.body
        else {
            panic!("briefing returns recall");
        };
        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].vendor, super::Vendor::declared(fixture));
        assert_eq!(window.turns.len(), 1);
        assert_eq!(window.turns[0].vendor, super::Vendor::declared(fixture));
    }

    #[test]
    fn dual_ambient_identities_follow_declared_codex_first_precedence() {
        let scratch = Scratch::new("ambient-precedence");
        let codex = scratch
            .path()
            .join("sessions")
            .join(format!("codex-{THREAD}.jsonl"));
        let claude = scratch
            .path()
            .join("projects")
            .join(format!("claude-{THREAD}.jsonl"));
        fs::create_dir_all(codex.parent().expect("Codex parent")).expect("Codex root exists");
        fs::create_dir_all(claude.parent().expect("Claude parent")).expect("Claude root exists");
        fs::write(&codex, b"\n").expect("Codex transcript exists");
        fs::write(&claude, b"\n").expect("Claude transcript exists");
        let identity = Some(std::ffi::OsString::from(THREAD));
        let home = Some(scratch.path().as_os_str().to_owned());
        let environment = Environment::new(
            vec![
                VendorEnvironment::new(&spice::extension::CLAUDE, identity.clone(), home.clone()),
                VendorEnvironment::new(&spice::extension::CODEX, identity, home),
            ],
            None,
            None,
        );

        let resolved = resolve_inputs(&[], &environment)
            .expect("both valid ambient identities resolve one declared winner");
        assert_eq!(resolved.len(), 1);
        assert_eq!(
            resolved[0].vendor,
            super::Vendor::declared(&spice::extension::CODEX)
        );
        assert_eq!(resolved[0].dialect, Some(listen::Dialect::Codex));
        assert_eq!(
            resolved[0].path,
            codex.canonicalize().expect("Codex transcript path")
        );
    }
}
