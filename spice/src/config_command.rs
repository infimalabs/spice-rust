use clap::ArgMatches;
use serde::Serialize;
use spice::config::{
    Builtin, ConcretePath, LoadError, PathSegment, Provenance, Resolved, descriptor,
};
use wire::{ConfigSource, Fault, FaultClass, Outcome, Refusal, Reply, ReplyPosition};

#[derive(Clone, Debug, PartialEq, Serialize)]
pub(crate) struct ConfigReport {
    entries: Vec<ConfigEntry>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
struct ConfigEntry {
    path: String,
    value: Option<toml::Value>,
    source: ConfigSource,
    template: &'static str,
    kind: &'static str,
    class: &'static str,
    layers: Vec<ConfigSource>,
    #[serde(skip_serializing_if = "Option::is_none")]
    builtin: Option<BuiltinValue>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "kebab-case")]
enum BuiltinValue {
    Absent,
    String(String),
    StringList(Vec<String>),
    UnsignedInteger(u64),
    Float(f64),
}

pub(crate) fn run(matches: &ArgMatches) -> Outcome<ConfigReport> {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => {
            return fault(
                FaultClass::Environment,
                "config::working-directory-unavailable",
                error.to_string(),
            );
        }
    };
    let resolved = match spice::config::load(&current, &[]) {
        Ok(resolved) => resolved,
        Err(error) => return load_failure(&error),
    };
    let mut entries = match entries(&resolved) {
        Ok(entries) => entries,
        Err(error) => return fault(FaultClass::Protocol, "config::render-failed", error),
    };

    if let Some(key) = matches.get_one::<String>("KEY") {
        entries.retain(|entry| entry.path == *key);
        if entries.is_empty() {
            return Outcome::Refusal(Refusal {
                reason: "config::key-unknown".to_owned(),
                witness: None,
                expected: Some("one canonical concrete key from config output".to_owned()),
                observed: Some(key.clone()),
                remedy: "run spice config and select one exact path from its entries".to_owned(),
            });
        }
        if matches.get_one::<String>("VALUE").is_some() {
            return Outcome::Refusal(Refusal {
                reason: "config::mutation-unavailable".to_owned(),
                witness: None,
                expected: Some("the read-only config [KEY] contract".to_owned()),
                observed: Some(key.clone()),
                remedy: concat!(
                    "remove VALUE; this command has no digest-bound mutation ",
                    "contract in release one"
                )
                .to_owned(),
            });
        }
    }

    Outcome::Reply(Reply {
        position: ReplyPosition::Direct {},
        body: ConfigReport { entries },
    })
}

fn entries(resolved: &Resolved) -> Result<Vec<ConfigEntry>, String> {
    let root = toml::Value::try_from(resolved.root()).map_err(|error| error.to_string())?;
    resolved
        .provenance()
        .iter()
        .map(|provenance| entry(&root, provenance))
        .collect()
}

fn entry(root: &toml::Value, provenance: &Provenance) -> Result<ConfigEntry, String> {
    let segments = provenance.path().segments().collect::<Vec<_>>();
    let schema = descriptor(&segments).ok_or_else(|| {
        format!(
            "effective configuration path has no registry descriptor: {}",
            provenance.path()
        )
    })?;
    Ok(ConfigEntry {
        path: provenance.path().to_string(),
        value: value_at(root, provenance.path()).cloned(),
        source: provenance.source(),
        template: schema.template().as_str(),
        kind: schema.kind().as_str(),
        class: schema.policy_class().as_str(),
        layers: schema.layers().to_vec(),
        builtin: schema.builtin().map(BuiltinValue::from),
    })
}

fn value_at<'a>(root: &'a toml::Value, path: &ConcretePath) -> Option<&'a toml::Value> {
    let mut value = root;
    for segment in path.segments() {
        value = match segment {
            PathSegment::Key(key) => value.as_table()?.get(key)?,
            PathSegment::Index(index) => value.as_array()?.get(index)?,
        };
    }
    Some(value)
}

impl From<Builtin> for BuiltinValue {
    fn from(value: Builtin) -> Self {
        match value {
            Builtin::Absent => Self::Absent,
            Builtin::String(value) => Self::String(value.to_owned()),
            Builtin::Strings(values) => {
                Self::StringList(values.iter().map(|value| (*value).to_owned()).collect())
            }
            Builtin::UnsignedInteger(value) => Self::UnsignedInteger(value),
            Builtin::Float(value) => Self::Float(value),
        }
    }
}

fn load_failure(error: &LoadError) -> Outcome<ConfigReport> {
    if let Some(repository) = error.repository() {
        return fault(
            FaultClass::Environment,
            "config::repository-unresolved",
            repository.to_string(),
        );
    }
    let diagnostics = diagnostics(error);
    if error
        .acquisition()
        .iter()
        .any(|issue| issue.io_error().is_some())
    {
        return fault(
            FaultClass::Environment,
            "config::input-unreadable",
            diagnostics,
        );
    }
    Outcome::Refusal(Refusal {
        reason: "config::configuration-refused".to_owned(),
        witness: None,
        expected: Some("tracked and local values admitted by the typed registry".to_owned()),
        observed: Some(diagnostics),
        remedy: "correct the named configuration inputs and rerun config".to_owned(),
    })
}

fn diagnostics(error: &LoadError) -> String {
    let mut diagnostics = error
        .acquisition()
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if let Some(resolution) = error.resolution() {
        diagnostics.extend(resolution.issues().iter().map(ToString::to_string));
    }
    if diagnostics.is_empty() {
        error.to_string()
    } else {
        diagnostics.join(" | ")
    }
}

fn fault(class: FaultClass, reason: &str, detail: String) -> Outcome<ConfigReport> {
    Outcome::Fault(Fault {
        class,
        reason: reason.to_owned(),
        detail,
    })
}

#[cfg(test)]
mod tests {
    use spice::config::{Document, Inputs, resolve};

    use super::{BuiltinValue, entries};

    #[test]
    fn absent_and_shadowed_builtins_remain_explicit() {
        let resolved = resolve(Inputs::new(
            Some(Document::new("spice.toml", "[lane]\nlife = \"Steer\"\n")),
            None,
            &[],
        ))
        .expect("fixture configuration resolves");
        let entries = entries(&resolved).expect("fixture entries render");

        let life = entries
            .iter()
            .find(|entry| entry.path == "lane.life")
            .expect("lane life entry exists");
        assert_eq!(life.builtin, Some(BuiltinValue::String("Drive".to_owned())));
        let container = entries
            .iter()
            .find(|entry| entry.path == "proof.container")
            .expect("absent container entry exists");
        assert_eq!(container.value, None);
        assert_eq!(container.builtin, Some(BuiltinValue::Absent));
    }
}
