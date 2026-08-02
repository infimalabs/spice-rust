//! Closed release-owned JSON decoding for task migration.

use std::collections::HashSet;

use serde_json::{Map, Value};

use super::{
    MigrationDecision, MigrationError, MigrationErrorKind, MigrationProvenance,
    PREDECESSOR_TASK_RELEASE, PredecessorTaskMigration, Source, TASK_MIGRATION_SCHEMA_VERSION,
};

const PREDECESSOR_LINEAGES_KEY: &str = "teams";
const PREDECESSOR_LINEAGE_ID_KEY: &str = "teamId";

pub(super) struct Parsed {
    pub(super) configured_hidden: Vec<String>,
    pub(super) hidden_provenance: Vec<Source>,
    pub(super) lineage_provenance: Vec<MigrationProvenance>,
}

pub(super) fn parse(input: PredecessorTaskMigration<'_>) -> Result<Parsed, MigrationError> {
    let value: Value = serde_json::from_slice(input.bytes).map_err(|error| {
        Source::root(input.source_file, "<document>").error(
            MigrationErrorKind::Syntax,
            None,
            format!("malformed JSON: {error}"),
            "repair the release-0.31.0 JSON export before retrying migration",
        )
    })?;
    let root = object(&value, &Source::root(input.source_file, "<document>"))?;
    unknown_fields(
        root,
        &["version", "release", "tasks", PREDECESSOR_LINEAGES_KEY],
        &Source::root(input.source_file, "<document>"),
    )?;
    exact_version(root, input.source_file)?;
    exact_release(root, input.source_file)?;
    let (configured_hidden, hidden_provenance) = hidden_stems(root, input.source_file)?;
    let lineage_provenance = lineage_overrides(root, input.source_file)?;
    Ok(Parsed {
        configured_hidden,
        hidden_provenance,
        lineage_provenance,
    })
}

fn exact_version(root: &Map<String, Value>, source_file: &str) -> Result<(), MigrationError> {
    let source = Source::root(source_file, "version");
    let Some(value) = root.get("version") else {
        return Err(source.error(
            MigrationErrorKind::Schema,
            None,
            "required schema version is absent",
            "export version 1 from release 0.31.0",
        ));
    };
    if value.as_u64() == Some(TASK_MIGRATION_SCHEMA_VERSION) {
        Ok(())
    } else {
        Err(source.error(
            MigrationErrorKind::Schema,
            None,
            format!("unsupported schema version {value}"),
            "export version 1 from release 0.31.0",
        ))
    }
}

fn exact_release(root: &Map<String, Value>, source_file: &str) -> Result<(), MigrationError> {
    let source = Source::root(source_file, "release");
    let Some(value) = root.get("release") else {
        return Err(source.error(
            MigrationErrorKind::Release,
            None,
            "owning release is absent",
            "export the task input with release set to 0.31.0",
        ));
    };
    if value.as_str() == Some(PREDECESSOR_TASK_RELEASE) {
        Ok(())
    } else {
        Err(source.error(
            MigrationErrorKind::Release,
            None,
            format!("unsupported owning release {value}"),
            "use the migration leaf owned by the export's exact release",
        ))
    }
}

fn hidden_stems(
    root: &Map<String, Value>,
    source_file: &str,
) -> Result<(Vec<String>, Vec<Source>), MigrationError> {
    let Some(tasks) = root.get("tasks") else {
        return Ok((Vec::new(), Vec::new()));
    };
    let source = Source::root(source_file, "tasks");
    let tasks = object(tasks, &source)?;
    unknown_fields(tasks, &["hidden_stems"], &source)?;
    let Some(hidden) = tasks.get("hidden_stems") else {
        return Ok((Vec::new(), Vec::new()));
    };
    let source = Source::root(source_file, "tasks.hidden_stems");
    let values = hidden.as_array().ok_or_else(|| {
        source.error(
            MigrationErrorKind::Malformed,
            None,
            format!("expected a string list, observed {}", value_kind(hidden)),
            "export tasks.hidden_stems as a JSON string list",
        )
    })?;
    let mut stems = Vec::with_capacity(values.len());
    let mut sources = Vec::with_capacity(values.len());
    for (index, value) in values.iter().enumerate() {
        let indexed = source.clone().at(index);
        let stem = value.as_str().ok_or_else(|| {
            indexed.error(
                MigrationErrorKind::Malformed,
                None,
                format!("expected a string, observed {}", value_kind(value)),
                "replace this hidden stem with one valid string",
            )
        })?;
        stems.push(stem.to_owned());
        sources.push(indexed);
    }
    Ok((stems, sources))
}

fn lineage_overrides(
    root: &Map<String, Value>,
    source_file: &str,
) -> Result<Vec<MigrationProvenance>, MigrationError> {
    let Some(lineages) = root.get(PREDECESSOR_LINEAGES_KEY) else {
        return Ok(Vec::new());
    };
    let source = Source::root(source_file, PREDECESSOR_LINEAGES_KEY);
    let lineages = lineages.as_array().ok_or_else(|| {
        source.error(
            MigrationErrorKind::Malformed,
            None,
            format!("expected a lineage list, observed {}", value_kind(lineages)),
            format!("export predecessor field {PREDECESSOR_LINEAGES_KEY:?} as a JSON list"),
        )
    })?;
    let mut identities = HashSet::new();
    let mut provenance = Vec::new();
    for (lineage_index, lineage) in lineages.iter().enumerate() {
        let indexed_source = Source::root(
            source_file,
            &format!("{PREDECESSOR_LINEAGES_KEY}[{lineage_index}]"),
        );
        let lineage = object(lineage, &indexed_source)?;
        let lineage_source = lineage
            .get(PREDECESSOR_LINEAGE_ID_KEY)
            .and_then(Value::as_str)
            .filter(|identity| !identity.is_empty())
            .map_or(indexed_source, |identity| {
                Source::lineage(
                    source_file,
                    identity,
                    format!("{PREDECESSOR_LINEAGES_KEY}[{lineage_index}]"),
                )
            });
        unknown_fields(
            lineage,
            &[PREDECESSOR_LINEAGE_ID_KEY, "shellSettings"],
            &lineage_source,
        )?;
        let identity_source = Source::root(
            source_file,
            &format!("{PREDECESSOR_LINEAGES_KEY}[{lineage_index}].{PREDECESSOR_LINEAGE_ID_KEY}"),
        );
        let identity = lineage
            .get(PREDECESSOR_LINEAGE_ID_KEY)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                identity_source.error(
                    MigrationErrorKind::Malformed,
                    None,
                    format!(
                        "required predecessor field {PREDECESSOR_LINEAGE_ID_KEY:?} must be a string"
                    ),
                    format!("export one nonempty {PREDECESSOR_LINEAGE_ID_KEY:?} for every lineage"),
                )
            })?;
        if identity.is_empty() {
            return Err(identity_source.error(
                MigrationErrorKind::Malformed,
                None,
                "lineage identity is empty",
                "export unique nonempty lineage identities",
            ));
        }
        if !identities.insert(identity.to_owned()) {
            return Err(Source::lineage(
                source_file,
                identity,
                format!("{PREDECESSOR_LINEAGES_KEY}[{lineage_index}].{PREDECESSOR_LINEAGE_ID_KEY}"),
            )
            .error(
                MigrationErrorKind::Malformed,
                None,
                format!("lineage identity is duplicated: {identity:?}"),
                "export unique nonempty lineage identities",
            ));
        }
        let Some(settings) = lineage.get("shellSettings") else {
            continue;
        };
        let settings_source = Source::lineage(
            source_file,
            identity,
            format!("{PREDECESSOR_LINEAGES_KEY}[{lineage_index}].shellSettings"),
        );
        let settings = object(settings, &settings_source)?;
        unknown_fields(settings, &["rc", "taskRc"], &settings_source)?;
        let selected = if let Some(value) = settings.get("taskRc") {
            Some(("taskRc", value))
        } else {
            settings.get("rc").map(|value| ("rc", value))
        };
        let Some((field, value)) = selected else {
            continue;
        };
        let selected_source = Source::lineage(
            source_file,
            identity,
            format!("{PREDECESSOR_LINEAGES_KEY}[{lineage_index}].shellSettings.{field}"),
        );
        provenance.extend(override_value(value, selected_source)?);
    }
    Ok(provenance)
}

fn override_value(
    value: &Value,
    source: Source,
) -> Result<Vec<MigrationProvenance>, MigrationError> {
    if let Some(token) = value.as_str() {
        validate_token(token, &source)?;
        return Ok(vec![
            source.provenance(MigrationDecision::SelectedEmpty, None),
        ]);
    }
    let values = value.as_array().ok_or_else(|| {
        source.error(
            MigrationErrorKind::Malformed,
            None,
            format!(
                "expected a string or string list, observed {}",
                value_kind(value)
            ),
            "export this shell setting as one string or an ordered string list",
        )
    })?;
    let mut provenance = Vec::new();
    if values.is_empty() {
        provenance.push(source.provenance(MigrationDecision::SelectedEmpty, None));
    }
    for (index, value) in values.iter().enumerate() {
        let indexed = source.clone().at(index);
        let token = value.as_str().ok_or_else(|| {
            indexed.error(
                MigrationErrorKind::Malformed,
                None,
                format!("expected a string, observed {}", value_kind(value)),
                "replace this override member with one string",
            )
        })?;
        validate_token(token, &indexed)?;
        provenance.push(indexed.provenance(MigrationDecision::SelectedEmpty, None));
    }
    Ok(provenance)
}

fn validate_token(token: &str, source: &Source) -> Result<(), MigrationError> {
    let normalized = token.trim();
    if normalized.is_empty() {
        return Ok(());
    }
    let (kind, message, recovery) = if normalized.starts_with("rc.") {
        (
            MigrationErrorKind::UnsupportedOverride,
            format!("raw Taskwarrior override {token:?} has no current typed equivalent"),
            concat!(
                "remove the override or export its intended ranking/filter behavior ",
                "for a future named Rust task field"
            ),
        )
    } else {
        (
            MigrationErrorKind::MalformedOverride,
            format!("override token {token:?} does not begin with rc."),
            "remove the malformed override and export only release-owned rc.* tokens",
        )
    };
    Err(source.error(kind, Some(token.to_owned()), message, recovery))
}

fn object<'a>(value: &'a Value, source: &Source) -> Result<&'a Map<String, Value>, MigrationError> {
    value.as_object().ok_or_else(|| {
        source.error(
            MigrationErrorKind::Malformed,
            None,
            format!("expected an object, observed {}", value_kind(value)),
            "export the documented version-1 object shape",
        )
    })
}

fn unknown_fields(
    object: &Map<String, Value>,
    allowed: &[&str],
    source: &Source,
) -> Result<(), MigrationError> {
    if let Some(field) = object
        .keys()
        .find(|field| !allowed.contains(&field.as_str()))
    {
        let mut field_source = source.clone();
        field_source.field = match source.field.as_str() {
            "<document>" => field.clone(),
            _ => format!("{}.{}", source.field, field),
        };
        return Err(field_source.error(
            MigrationErrorKind::Schema,
            None,
            format!("unsupported export field {field:?}"),
            "remove the field or use the release-specific migration schema that owns it",
        ));
    }
    Ok(())
}

fn value_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "boolean",
        Value::Number(_) => "number",
        Value::String(_) => "string",
        Value::Array(_) => "list",
        Value::Object(_) => "object",
    }
}
