use super::*;

pub(super) fn seeded_policy(path: PathBuf) -> Result<Vec<u8>, Error> {
    let existing = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => return Err(io_fault("read tracked configuration", &path, source)),
    };
    let text = String::from_utf8(existing).map_err(|_| {
        refusal(
            "install::tracked-config-encoding",
            Some("UTF-8 TOML".to_owned()),
            Some(path.display().to_string()),
            "repair spice.toml before installing",
        )
    })?;
    if !text.is_empty() {
        toml::from_str::<toml::Value>(&text).map_err(|source| {
            refusal(
                "install::tracked-config-invalid",
                Some("valid TOML".to_owned()),
                Some(source.to_string()),
                "repair spice.toml before installing",
            )
        })?;
    }
    seed_tastes(text)
}

fn seed_tastes(mut text: String) -> Result<Vec<u8>, Error> {
    let parsed = if text.is_empty() {
        toml::Value::Table(toml::Table::new())
    } else {
        toml::from_str::<toml::Value>(&text)
            .map_err(|source| fault("install::tracked-config-invalid", source.to_string()))?
    };
    let present = parsed.get("taste").and_then(toml::Value::as_table);
    for name in TASTE_NAMES {
        if present.is_some_and(|table| table.contains_key(name)) {
            continue;
        }
        if !text.is_empty() && !text.ends_with('\n') {
            text.push('\n');
        }
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(taste_block(name)?);
    }
    toml::from_str::<toml::Value>(&text).map_err(|source| {
        refusal(
            "install::tracked-config-shape",
            Some("taste must be a table that accepts generated seed rows".to_owned()),
            Some(source.to_string()),
            "repair the taste configuration before installing",
        )
    })?;
    Ok(text.into_bytes())
}

fn taste_block(name: &str) -> Result<&'static str, Error> {
    let heading = format!("[taste.{name}]");
    let start = TRACKED_POLICY.find(&heading).ok_or_else(|| {
        fault(
            "install::seed-missing",
            format!("compiled policy omitted {heading}"),
        )
    })?;
    let rest = &TRACKED_POLICY[start..];
    let end = rest[heading.len()..]
        .find("\n[taste.")
        .map_or(rest.len(), |offset| heading.len() + offset + 1);
    Ok(rest[..end].trim_end())
}

pub(super) fn seeded_baseline(path: PathBuf) -> Result<Vec<u8>, Error> {
    match fs::read(&path) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes.clone()).map_err(|_| {
                refusal(
                    "install::baseline-encoding",
                    Some("UTF-8 TOML".to_owned()),
                    Some(path.display().to_string()),
                    "repair baseline.toml before installing",
                )
            })?;
            toml::from_str::<toml::Value>(&text).map_err(|source| {
                refusal(
                    "install::baseline-invalid",
                    Some("valid TOML".to_owned()),
                    Some(source.to_string()),
                    "repair baseline.toml before installing",
                )
            })?;
            Ok(bytes)
        }
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(BASELINE.to_vec()),
        Err(source) => Err(io_fault("read baseline", &path, source)),
    }
}

pub(super) fn pre_commit(executable: &Path) -> String {
    format!(
        "#!/bin/sh\nset -eu\nexec {} gate\n",
        shell_word(&executable.display().to_string())
    )
}

pub(super) fn systemd_unit(executable: &Path, root: &Path) -> String {
    format!(
        concat!(
            "[Unit]\nDescription=Spice repository resident\n\n",
            "[Service]\nType=simple\nExecStart={} up\nWorkingDirectory={}\n",
            "Restart=on-failure\n\n[Install]\nWantedBy=default.target\n",
        ),
        systemd_word(&executable.display().to_string()),
        systemd_word(&root.display().to_string()),
    )
}

pub(super) fn launchd_unit(executable: &Path, root: &Path, suffix: &str) -> String {
    format!(
        concat!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n",
            "<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" ",
            "\"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n",
            "<plist version=\"1.0\"><dict>\n",
            "<key>Label</key><string>dev.spice.{}</string>\n",
            "<key>ProgramArguments</key><array><string>{}</string><string>up</string></array>\n",
            "<key>WorkingDirectory</key><string>{}</string>\n",
            "<key>KeepAlive</key><true/>\n</dict></plist>\n",
        ),
        suffix,
        xml(&executable.display().to_string()),
        xml(&root.display().to_string()),
    )
}

fn shell_word(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

fn systemd_word(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', "\\\\").replace('"', "\\\""))
}

fn xml(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[derive(Clone, Debug)]
pub(super) struct ParsedHook {
    pub(super) value: Option<String>,
    line: Option<usize>,
    core_end: Option<usize>,
}

pub(super) fn parse_hook(text: &str) -> Result<ParsedHook, Error> {
    let mut section = String::new();
    let mut value = None;
    let mut line_index = None;
    let mut core_end = None;
    for (index, line) in text.split('\n').enumerate() {
        let trimmed = line.trim();
        if let Some(name) = section_name(trimmed) {
            if section == "core" && core_end.is_none() {
                core_end = Some(index);
            }
            section = name;
            continue;
        }
        if section != "core" || trimmed.is_empty() || trimmed.starts_with(['#', ';']) {
            continue;
        }
        let Some(found) = config_value(trimmed, "hookspath")? else {
            continue;
        };
        if value.is_some() {
            return Err(refusal(
                "install::hook-setting-ambiguous",
                Some("at most one core.hooksPath".to_owned()),
                Some("multiple values".to_owned()),
                "reduce core.hooksPath to one value before installing",
            ));
        }
        value = Some(found);
        line_index = Some(index);
    }
    if section == "core" && core_end.is_none() {
        core_end = Some(text.split('\n').count());
    }
    Ok(ParsedHook {
        value,
        line: line_index,
        core_end,
    })
}

fn section_name(line: &str) -> Option<String> {
    let body = line.strip_prefix('[')?.strip_suffix(']')?;
    Some(body.split_whitespace().next()?.to_ascii_lowercase())
}

fn config_value(line: &str, expected: &str) -> Result<Option<String>, Error> {
    let (key, raw) = match line.split_once('=') {
        Some((key, raw)) => (key.trim(), raw.trim()),
        None => {
            let mut parts = line.splitn(2, char::is_whitespace);
            (
                parts.next().unwrap_or_default(),
                parts.next().unwrap_or_default().trim(),
            )
        }
    };
    if !key.eq_ignore_ascii_case(expected) {
        return Ok(None);
    }
    decode_config_value(raw).map(Some)
}

fn decode_config_value(raw: &str) -> Result<String, Error> {
    if !raw.starts_with('"') {
        return Ok(raw.to_owned());
    }
    if !raw.ends_with('"') || raw.len() < 2 {
        return Err(config_shape_refusal(raw));
    }
    let mut decoded = String::new();
    let mut escaped = false;
    for character in raw[1..raw.len() - 1].chars() {
        if escaped {
            match character {
                '\\' | '"' => decoded.push(character),
                'n' => decoded.push('\n'),
                't' => decoded.push('\t'),
                'b' => decoded.push('\u{0008}'),
                _ => return Err(config_shape_refusal(raw)),
            }
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else {
            decoded.push(character);
        }
    }
    if escaped {
        return Err(config_shape_refusal(raw));
    }
    Ok(decoded)
}

pub(super) fn set_hook(text: &str, value: Option<&str>) -> Result<String, Error> {
    let parsed = parse_hook(text)?;
    let mut lines = text.split('\n').map(str::to_owned).collect::<Vec<_>>();
    let replacement = value.map(|value| format!("\thooksPath = {}", encode_config_value(value)));
    match (parsed.line, replacement) {
        (Some(index), Some(line)) => lines[index] = line,
        (Some(index), None) => {
            lines.remove(index);
        }
        (None, Some(line)) => insert_hook(&mut lines, parsed.core_end, line),
        (None, None) => {}
    }
    Ok(lines.join("\n"))
}

fn insert_hook(lines: &mut Vec<String>, core_end: Option<usize>, line: String) {
    match core_end {
        Some(index) => lines.insert(index.min(lines.len()), line),
        None => {
            if lines.last().is_some_and(|line| !line.is_empty()) {
                lines.push(String::new());
            }
            lines.push("[core]".to_owned());
            lines.push(line);
        }
    }
}

fn encode_config_value(value: &str) -> String {
    format!(
        "\"{}\"",
        value
            .replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
            .replace('\t', "\\t")
    )
}

pub(super) fn snapshot_mode(snapshot: &Snapshot) -> Option<u32> {
    match snapshot {
        Snapshot::Absent => None,
        Snapshot::File { mode, .. } => Some(*mode),
    }
}

pub(super) fn snapshot_generation(snapshot: &Snapshot) -> Option<Generation> {
    match snapshot {
        Snapshot::Absent => None,
        Snapshot::File { generation, .. } => Some(*generation),
    }
}

pub(super) fn snapshot_bytes(snapshot: &Snapshot) -> Result<String, Error> {
    match snapshot {
        Snapshot::Absent => Ok(String::new()),
        Snapshot::File { bytes, .. } => String::from_utf8(bytes.clone())
            .map_err(|_| record_shape_refusal("recorded Git configuration prior is not UTF-8")),
    }
}

pub(super) fn validate_parent_observation(
    path: &Path,
    parent: &ParentObservation,
    root: &Path,
) -> Result<(), Error> {
    let expected_parent = path.parent().ok_or_else(|| {
        record_shape_refusal("recorded destination does not have one file parent")
    })?;
    if !path.starts_with(root)
        || !parent.ancestor.starts_with(root)
        || parent.ancestor.join(&parent.missing) != expected_parent
    {
        return Err(record_shape_refusal(
            "recorded parent binding escapes or disagrees with its destination",
        ));
    }
    let canonical = fs::canonicalize(&parent.ancestor).map_err(|source| {
        io_fault(
            "resolve recorded install destination ancestor",
            &parent.ancestor,
            source,
        )
    })?;
    let metadata = fs::metadata(&canonical).map_err(|source| {
        io_fault(
            "inspect recorded install destination ancestor",
            &canonical,
            source,
        )
    })?;
    if canonical != parent.ancestor
        || !metadata.is_dir()
        || generation(&metadata) != parent.generation
    {
        return Err(record_shape_refusal(
            "recorded destination ancestor is no longer its bound generation",
        ));
    }
    Ok(())
}

pub(super) fn snapshot_signature(
    metadata: &fs::Metadata,
) -> (Generation, u64, u32, Option<std::time::SystemTime>) {
    (
        generation(metadata),
        metadata.len(),
        file_mode(metadata),
        metadata.modified().ok(),
    )
}

pub(super) fn canonical_file(path: &Path, reason: &str) -> Result<PathBuf, Error> {
    let canonical =
        fs::canonicalize(path).map_err(|source| io_fault("resolve executable", path, source))?;
    let metadata = fs::metadata(&canonical)
        .map_err(|source| io_fault("inspect executable", &canonical, source))?;
    if !metadata.is_file() {
        return Err(refusal(
            reason,
            Some("regular file".to_owned()),
            Some(canonical.display().to_string()),
            "run the installed Rust spice executable directly",
        ));
    }
    Ok(canonical)
}

pub(super) fn canonical_directory(path: &Path, reason: &str) -> Result<PathBuf, Error> {
    let canonical =
        fs::canonicalize(path).map_err(|source| io_fault("resolve home", path, source))?;
    let metadata =
        fs::metadata(&canonical).map_err(|source| io_fault("inspect home", &canonical, source))?;
    if !metadata.is_dir() {
        return Err(refusal(
            reason,
            Some("directory".to_owned()),
            Some(canonical.display().to_string()),
            "select an existing home directory",
        ));
    }
    Ok(canonical)
}

pub(super) fn repository_error(error: crate::repository::ResolveError) -> Error {
    refusal(
        "install::repository",
        Some("one containing Git repository".to_owned()),
        Some(error.to_string()),
        "run spice install from inside the repository to configure",
    )
}

pub(super) fn drift_refusal(path: &Path) -> Error {
    refusal(
        "install::drift",
        Some("the bytes and mode recorded at apply".to_owned()),
        Some(path.display().to_string()),
        "restore the installed bytes or reconcile the path before unapplying",
    )
}

pub(super) fn destination_drift_refusal(path: &Path) -> Error {
    refusal(
        "install::destination-drift",
        Some("the exact destination and generation bound by the plan".to_owned()),
        Some(path.display().to_string()),
        "preview the installation again after destination state settles",
    )
}

pub(super) fn destination_root_refusal(path: &Path, root: &Path) -> Error {
    refusal(
        "install::destination-root",
        Some(format!("a destination confined beneath {}", root.display())),
        Some(path.display().to_string()),
        "move destination symlinks beneath the permitted install root",
    )
}

pub(super) fn record_shape_refusal(observed: &str) -> Error {
    refusal(
        "install::record-shape",
        Some("the canonical record for this repository install plan".to_owned()),
        Some(observed.to_owned()),
        "restore the exact install record or reconcile the installation manually",
    )
}

pub(super) fn record_version_refusal(observed: u16) -> Error {
    refusal(
        "install::record-version",
        Some(RECORD_VERSION.to_string()),
        Some(observed.to_string()),
        "use the Spice release that wrote this install record",
    )
}

fn config_shape_refusal(observed: &str) -> Error {
    refusal(
        "install::git-config-shape",
        Some("one ordinary core.hooksPath value".to_owned()),
        Some(observed.to_owned()),
        "normalize core.hooksPath before installing",
    )
}

pub(super) fn refusal(
    reason: &str,
    expected: Option<String>,
    observed: Option<String>,
    remedy: &str,
) -> Error {
    Error::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: None,
        expected,
        observed,
        remedy: remedy.to_owned(),
    })
}

pub(super) fn fault(reason: &str, detail: impl Into<String>) -> Error {
    Error::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.into(),
    })
}

pub(super) fn io_fault(operation: &str, path: &Path, source: std::io::Error) -> Error {
    fault(
        "install::filesystem",
        format!("{operation} at {}: {source}", path.display()),
    )
}

pub(super) fn encode_fault(subject: &str, source: impl fmt::Display) -> Error {
    fault("install::encoding", format!("encode {subject}: {source}"))
}

#[cfg(unix)]
pub(super) fn file_mode(metadata: &fs::Metadata) -> u32 {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o777
}

#[cfg(unix)]
pub(super) fn generation(metadata: &fs::Metadata) -> Generation {
    use std::os::unix::fs::MetadataExt;

    Generation {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

#[cfg(not(unix))]
pub(super) fn generation(_metadata: &fs::Metadata) -> Generation {
    Generation {
        device: 0,
        inode: 0,
    }
}

#[cfg(not(unix))]
pub(super) fn file_mode(_metadata: &fs::Metadata) -> u32 {
    DATA_MODE
}

#[cfg(unix)]
pub(super) fn set_mode(path: &Path, mode: u32) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
pub(super) fn set_mode(_path: &Path, _mode: u32) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
pub(super) fn effective_mode(mode: u32) -> u32 {
    mode
}

#[cfg(not(unix))]
pub(super) fn effective_mode(_mode: u32) -> u32 {
    DATA_MODE
}
