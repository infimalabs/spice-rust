use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use clap::ArgMatches;
use serde::Serialize;
use sill::{Acquisition, LockFile, LockName};
use spice::config::{
    self, Builtin, Document, Inputs, PathSegment, PolicyClass, Resolved, ValueKind,
};
use spice::repository::Repository;
use wire::{
    ConfigSource, Digest, Domain, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition,
};

const IGNORED_TRACKED_ROOTS: &[&str] = &["policy", "tasks"];

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct ConfigReport {
    mode: ConfigMode,
    ignored_tracked_roots: &'static [&'static str],
    effective: config::Root,
    provenance: Vec<ProvenanceRow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected: Option<Selection>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mutation: Option<MutationReceipt>,
}

impl ConfigReport {
    pub(crate) fn text(&self) -> Result<Vec<u8>, toml::ser::Error> {
        let mut document = toml::to_string_pretty(self)?.into_bytes();
        if document.last() != Some(&b'\n') {
            document.push(b'\n');
        }
        Ok(document)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ConfigMode {
    PythonScaffoldCoexistence,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct ProvenanceRow {
    path: String,
    source: ConfigSource,
    #[serde(skip_serializing_if = "Option::is_none")]
    builtin: Option<Builtin>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct Selection {
    path: String,
    value: toml::Value,
    source: ConfigSource,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct MutationReceipt {
    path: String,
    destination: PathBuf,
    before: Digest,
    after: Digest,
    changed: bool,
}

pub(crate) fn run(matches: &ArgMatches) -> Outcome<ConfigReport> {
    let current = match crate::working_directory() {
        Ok(current) => current,
        Err(outcome) => return map_outcome(outcome),
    };
    let repository = match Repository::discover(&current) {
        Ok(repository) => repository,
        Err(error) => {
            return map_outcome(environment_fault(
                "config::repository-unresolved",
                &error.to_string(),
            ));
        }
    };
    let key = matches.get_one::<String>("KEY");
    let value = matches.get_one::<String>("VALUE");
    let result = match (key, value) {
        (Some(key), Some(value)) => set(&repository, key, value),
        (key, None) => read(&repository, key.map(String::as_str)),
        (None, Some(_)) => unreachable!("clap cannot supply VALUE without KEY"),
    };
    match result {
        Ok(report) => Outcome::Reply(Reply {
            position: ReplyPosition::Direct {},
            body: report,
        }),
        Err(outcome) => map_outcome(outcome),
    }
}

fn read(repository: &Repository, key: Option<&str>) -> Result<ConfigReport, Outcome<()>> {
    let resolved = config::load_coexisting_repository(repository, &[])
        .map_err(|error| configuration_refusal(&error))?;
    report(resolved, key, None)
}

fn set(repository: &Repository, key: &str, raw: &str) -> Result<ConfigReport, Outcome<()>> {
    let keys = parse_key(key)?;
    let borrowed = borrowed(&keys);
    let descriptor = config::descriptor(&borrowed).ok_or_else(|| {
        refusal(
            "config::key-unknown",
            Some(key.to_owned()),
            "select one concrete path in the generated configuration registry",
        )
    })?;
    if descriptor.policy_class() != PolicyClass::MachineLocal {
        return Err(refusal(
            "config::tracked-policy",
            Some(key.to_owned()),
            "edit tracked spice.toml in an operator-reviewed commit",
        ));
    }
    let value = parse_value(raw, descriptor.kind())?;
    let documents = Documents::read(repository)?;
    let mut local = documents.local_value()?;
    set_value(&mut local, &keys, value)?;
    let candidate = toml::to_string_pretty(&local)
        .map_err(|error| protocol_fault("config::candidate-unencodable", &error.to_string()))?;
    let resolved = documents.resolve(&candidate)?;
    let receipt = publish(&documents, key, candidate.as_bytes())?;
    report(resolved, Some(key), Some(receipt))
}

struct Documents {
    tracked: Option<Vec<u8>>,
    local: Option<Vec<u8>>,
    tracked_path: PathBuf,
    local_path: PathBuf,
}

impl Documents {
    fn read(repository: &Repository) -> Result<Self, Outcome<()>> {
        let locations = config::locations(repository);
        Ok(Self {
            tracked: read_optional(locations.tracked())?,
            local: read_optional(locations.local())?,
            tracked_path: locations.tracked().to_path_buf(),
            local_path: locations.local().to_path_buf(),
        })
    }

    fn local_value(&self) -> Result<toml::Value, Outcome<()>> {
        let Some(bytes) = &self.local else {
            return Ok(toml::Value::Table(toml::Table::new()));
        };
        let text = std::str::from_utf8(bytes).map_err(|error| {
            refusal(
                "config::local-non-utf8",
                Some(self.local_path.display().to_string()),
                &error.to_string(),
            )
        })?;
        toml::from_str(text).map_err(|error| {
            refusal(
                "config::local-malformed",
                Some(self.local_path.display().to_string()),
                &error.to_string(),
            )
        })
    }

    fn resolve(&self, local: &str) -> Result<Resolved, Outcome<()>> {
        let tracked_text = optional_utf8(self.tracked.as_deref(), &self.tracked_path)?;
        let tracked_location = self.tracked_path.display().to_string();
        let local_location = self.local_path.display().to_string();
        config::resolve(Inputs::coexisting(
            tracked_text.map(|text| Document::new(&tracked_location, text)),
            Some(Document::new(&local_location, local)),
            &[],
        ))
        .map_err(|error| resolution_refusal(&error))
    }
}

fn publish(
    documents: &Documents,
    key: &str,
    candidate: &[u8],
) -> Result<MutationReceipt, Outcome<()>> {
    let before = Digest::compute(
        Domain::ConfigLocal,
        documents.local.as_deref().unwrap_or(&[]),
    );
    let after = Digest::compute(Domain::ConfigLocal, candidate);
    if before == after {
        return Ok(receipt(key, &documents.local_path, before, after, false));
    }
    let parent = documents.local_path.parent().ok_or_else(|| {
        environment_fault(
            "config::local-parent-missing",
            &documents.local_path.display().to_string(),
        )
    })?;
    fs::create_dir_all(parent).map_err(|error| {
        environment_fault("config::local-parent-unavailable", &error.to_string())
    })?;
    let lock = LockFile::new(
        LockName::new("config").expect("config is a valid lock name"),
        parent.join("config.lock"),
    );
    let possession = match lock.try_acquire() {
        Ok(Acquisition::Acquired(possession)) => possession,
        Ok(Acquisition::Contended) => {
            return Err(refusal(
                "config::busy",
                Some(lock.path().display().to_string()),
                "retry after the current local configuration writer exits",
            ));
        }
        Err(error) => {
            return Err(environment_fault(
                "config::lock-unavailable",
                &error.to_string(),
            ));
        }
    };
    ensure_unchanged(documents)?;
    let destination = atomic::Destination::new(&documents.local_path).map_err(|error| {
        environment_fault("config::publication-unavailable", &error.to_string())
    })?;
    destination
        .publish(|staged| staged.write_all(candidate))
        .map_err(|error| environment_fault("config::publication-failed", &error.to_string()))?;
    drop(possession);
    Ok(receipt(key, &documents.local_path, before, after, true))
}

fn receipt(
    key: &str,
    destination: &Path,
    before: Digest,
    after: Digest,
    changed: bool,
) -> MutationReceipt {
    MutationReceipt {
        path: key.to_owned(),
        destination: destination.to_path_buf(),
        before,
        after,
        changed,
    }
}

fn ensure_unchanged(documents: &Documents) -> Result<(), Outcome<()>> {
    let tracked = read_optional(&documents.tracked_path)?;
    let local = read_optional(&documents.local_path)?;
    if tracked == documents.tracked && local == documents.local {
        return Ok(());
    }
    Err(refusal(
        "config::source-moved",
        Some(documents.local_path.display().to_string()),
        "reload the changed tracked or local configuration and retry",
    ))
}

fn report(
    resolved: Resolved,
    key: Option<&str>,
    mutation: Option<MutationReceipt>,
) -> Result<ConfigReport, Outcome<()>> {
    let selected = key.map(|key| selection(&resolved, key)).transpose()?;
    let provenance = resolved
        .provenance()
        .iter()
        .map(|row| ProvenanceRow {
            path: row.path().to_string(),
            source: row.source(),
            builtin: row.builtin(),
        })
        .collect();
    Ok(ConfigReport {
        mode: ConfigMode::PythonScaffoldCoexistence,
        ignored_tracked_roots: IGNORED_TRACKED_ROOTS,
        effective: resolved.into_root(),
        provenance,
        selected,
        mutation,
    })
}

fn selection(resolved: &Resolved, key: &str) -> Result<Selection, Outcome<()>> {
    let keys = parse_key(key)?;
    let effective = toml::Value::try_from(resolved.root())
        .map_err(|error| protocol_fault("config::effective-unencodable", &error.to_string()))?;
    let value = lookup(&effective, &keys).cloned().ok_or_else(|| {
        refusal(
            "config::key-absent",
            Some(key.to_owned()),
            "select a concrete effective configuration leaf",
        )
    })?;
    let source = resolved
        .provenance()
        .iter()
        .find(|row| path_matches(row.path(), &keys))
        .map(config::Provenance::source)
        .ok_or_else(|| {
            protocol_fault(
                "config::provenance-missing",
                &format!("effective key {key} has no provenance row"),
            )
        })?;
    Ok(Selection {
        path: key.to_owned(),
        value,
        source,
    })
}

fn parse_key(raw: &str) -> Result<Vec<String>, Outcome<()>> {
    let document = format!("{raw} = 0");
    let mut value: toml::Value = toml::from_str(&document).map_err(|error| {
        refusal(
            "config::key-refused",
            Some(raw.to_owned()),
            &error.to_string(),
        )
    })?;
    let mut keys = Vec::new();
    loop {
        let toml::Value::Table(table) = value else {
            break;
        };
        if table.len() != 1 {
            return Err(refusal(
                "config::key-refused",
                Some(raw.to_owned()),
                "supply one concrete dotted table leaf",
            ));
        }
        let (key, child) = table.into_iter().next().expect("one table entry exists");
        keys.push(key);
        value = child;
    }
    if keys.len() < 2 || value != toml::Value::Integer(0) {
        return Err(refusal(
            "config::key-refused",
            Some(raw.to_owned()),
            "supply one concrete dotted table leaf",
        ));
    }
    Ok(keys)
}

fn parse_value(raw: &str, kind: ValueKind) -> Result<toml::Value, Outcome<()>> {
    let parsed = toml::from_str::<toml::Value>(&format!("value = {raw}"))
        .ok()
        .and_then(|mut value| value.as_table_mut()?.remove("value"));
    if let Some(value) = parsed {
        return Ok(value);
    }
    if matches!(kind, ValueKind::String | ValueKind::Duration) {
        return Ok(toml::Value::String(raw.to_owned()));
    }
    Err(refusal(
        "config::value-refused",
        Some(raw.to_owned()),
        &format!("supply a TOML {} value", kind.as_str()),
    ))
}

fn borrowed(keys: &[String]) -> Vec<PathSegment<'_>> {
    keys.iter()
        .map(|key| PathSegment::Key(key.as_str()))
        .collect()
}

fn set_value(
    root: &mut toml::Value,
    keys: &[String],
    value: toml::Value,
) -> Result<(), Outcome<()>> {
    let mut table = root.as_table_mut().ok_or_else(|| {
        refusal(
            "config::local-malformed",
            None,
            "make local configuration a TOML table",
        )
    })?;
    for key in &keys[..keys.len() - 1] {
        let child = table
            .entry(key.clone())
            .or_insert_with(|| toml::Value::Table(toml::Table::new()));
        table = child.as_table_mut().ok_or_else(|| {
            refusal(
                "config::path-conflict",
                Some(key.clone()),
                "remove the scalar that occupies the requested table path",
            )
        })?;
    }
    table.insert(
        keys.last().expect("a parsed key is nonempty").clone(),
        value,
    );
    Ok(())
}

fn lookup<'a>(root: &'a toml::Value, keys: &[String]) -> Option<&'a toml::Value> {
    keys.iter().try_fold(root, |value, key| value.get(key))
}

fn path_matches(path: &config::ConcretePath, keys: &[String]) -> bool {
    let segments = path.segments().collect::<Vec<_>>();
    segments.len() == keys.len()
        && segments
            .iter()
            .zip(keys)
            .all(|(segment, key)| matches!(segment, PathSegment::Key(found) if *found == key))
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, Outcome<()>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(environment_fault(
                "config::source-unavailable",
                &format!("{}: {error}", path.display()),
            ));
        }
    };
    if !metadata.file_type().is_file() {
        return Err(refusal(
            "config::source-not-regular",
            Some(path.display().to_string()),
            "replace the source with one regular file",
        ));
    }
    fs::read(path).map(Some).map_err(|error| {
        environment_fault(
            "config::source-unreadable",
            &format!("{}: {error}", path.display()),
        )
    })
}

fn optional_utf8<'a>(bytes: Option<&'a [u8]>, path: &Path) -> Result<Option<&'a str>, Outcome<()>> {
    bytes
        .map(|bytes| {
            std::str::from_utf8(bytes).map_err(|error| {
                refusal(
                    "config::source-non-utf8",
                    Some(path.display().to_string()),
                    &error.to_string(),
                )
            })
        })
        .transpose()
}

fn configuration_refusal(error: &config::LoadError) -> Outcome<()> {
    let mut details = Vec::new();
    if let Some(repository) = error.repository() {
        details.push(repository.to_string());
    }
    details.extend(error.acquisition().iter().map(ToString::to_string));
    if let Some(resolution) = error.resolution() {
        details.extend(resolution.issues().iter().map(ToString::to_string));
    }
    refusal(
        "config::load-refused",
        (!details.is_empty()).then(|| details.join(" | ")),
        "repair the named tracked or local configuration issue and retry",
    )
}

fn resolution_refusal(error: &config::ResolveError) -> Outcome<()> {
    refusal(
        "config::candidate-refused",
        Some(
            error
                .issues()
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(" | "),
        ),
        "supply a value admitted by the named configuration leaf",
    )
}

fn map_outcome<B>(outcome: Outcome<()>) -> Outcome<B> {
    match outcome {
        Outcome::Refusal(refusal) => Outcome::Refusal(refusal),
        Outcome::Fault(fault) => Outcome::Fault(fault),
        Outcome::Reply(_) => unreachable!("error paths cannot carry replies"),
    }
}

fn refusal(reason: &str, observed: Option<String>, remedy: &str) -> Outcome<()> {
    Outcome::Refusal(Refusal {
        reason: reason.to_owned(),
        witness: None,
        expected: None,
        observed,
        remedy: remedy.to_owned(),
    })
}

fn environment_fault(reason: &str, detail: &str) -> Outcome<()> {
    Outcome::Fault(Fault {
        class: FaultClass::Environment,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}

fn protocol_fault(reason: &str, detail: &str) -> Outcome<()> {
    Outcome::Fault(Fault {
        class: FaultClass::Protocol,
        reason: reason.to_owned(),
        detail: detail.to_owned(),
    })
}
