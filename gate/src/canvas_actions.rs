//! Canonical proof actions derived from the Canvas browser shard registry.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use sill::Environment;

mod build;

pub const CANVAS_ACTION_REGISTRY: &str = "app/browser/canvas-shards.json";
pub const CANVAS_BUILD_ACTION: &str = "canvas:build";
pub const CANVAS_ACTION_PREFIX: &str = "canvas:shard:";
pub const CANVAS_ACTION_GROUP: &str = "canvas";
pub const CANVAS_PREDECESSOR_RELEASE: &str = "0.31.0";
const CANVAS_REGISTRY_VERSION: u64 = 1;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
struct CanvasRegistry {
    version: u64,
    shards: Vec<CanvasShard>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanvasShard {
    name: String,
    timeout_ms: u64,
    scenarios: Vec<String>,
}

impl CanvasShard {
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    #[must_use]
    pub const fn timeout_millis(&self) -> u64 {
        self.timeout_ms
    }

    #[must_use]
    pub fn scenarios(&self) -> &[String] {
        &self.scenarios
    }

    #[must_use]
    pub fn action_id(&self) -> String {
        format!("{CANVAS_ACTION_PREFIX}{}", self.name)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanvasActionPlan {
    registry: Vec<u8>,
    build_inputs: Vec<String>,
    build_non_cacheable_reason: Option<String>,
    shard_inputs: Vec<String>,
    shards: Vec<CanvasShard>,
}

pub struct CanvasActionPlanning<'a> {
    pub root: &'a Path,
    pub paths: &'a [String],
    pub changed_paths: &'a [String],
    pub environment: &'a Environment,
    pub cargo: &'a OsStr,
    pub external_source: Option<&'a crate::CargoExternalSourceAuthority>,
}

impl CanvasActionPlan {
    pub fn load(planning: CanvasActionPlanning<'_>) -> Result<Self, CanvasActionError> {
        let CanvasActionPlanning {
            root,
            paths,
            changed_paths,
            environment,
            cargo,
            external_source,
        } = planning;
        let registry_path = root.join(CANVAS_ACTION_REGISTRY);
        let registry = fs::read(&registry_path).map_err(|error| {
            CanvasActionError::new(format!(
                "cannot read Canvas action registry {}: {error}",
                registry_path.display()
            ))
        })?;
        let decoded: CanvasRegistry = serde_json::from_slice(&registry).map_err(|error| {
            CanvasActionError::new(format!("Canvas action registry is invalid: {error}"))
        })?;
        validate_registry(&decoded)?;
        let build = build::authority(root, paths, environment, cargo, external_source);
        let shard_inputs = selected_inputs(paths, changed_paths, shard_input);
        if build.input_paths.is_empty() || shard_inputs.is_empty() {
            return Err(CanvasActionError::new(
                "Canvas proof actions require nonempty build and browser source closures",
            ));
        }
        Ok(Self {
            registry,
            build_inputs: build.input_paths,
            build_non_cacheable_reason: build.non_cacheable_reason,
            shard_inputs,
            shards: decoded.shards,
        })
    }

    #[must_use]
    pub fn registry_bytes(&self) -> &[u8] {
        &self.registry
    }

    #[must_use]
    pub fn build_inputs(&self) -> &[String] {
        &self.build_inputs
    }

    #[must_use]
    pub fn build_non_cacheable_reason(&self) -> Option<&str> {
        self.build_non_cacheable_reason.as_deref()
    }

    #[must_use]
    pub fn shard_inputs(&self) -> &[String] {
        &self.shard_inputs
    }

    #[must_use]
    pub fn shards(&self) -> &[CanvasShard] {
        &self.shards
    }

    #[must_use]
    pub fn action_ids(&self) -> Vec<String> {
        std::iter::once(CANVAS_BUILD_ACTION.to_owned())
            .chain(self.shards.iter().map(CanvasShard::action_id))
            .collect()
    }
}

fn validate_registry(registry: &CanvasRegistry) -> Result<(), CanvasActionError> {
    if registry.version != CANVAS_REGISTRY_VERSION || registry.shards.is_empty() {
        return Err(CanvasActionError::new(
            "Canvas action registry requires version 1 and at least one shard",
        ));
    }
    let mut names = Vec::new();
    let mut scenarios = Vec::new();
    for shard in &registry.shards {
        if !valid_name(&shard.name)
            || shard.timeout_ms == 0
            || shard.scenarios.is_empty()
            || shard.scenarios.iter().any(|name| !valid_name(name))
        {
            return Err(CanvasActionError::new(format!(
                "Canvas shard {:?} has an invalid name, timeout, or scenario inventory",
                shard.name
            )));
        }
        names.push(shard.name.as_str());
        scenarios.extend(shard.scenarios.iter().map(String::as_str));
    }
    names.sort_unstable();
    scenarios.sort_unstable();
    if names.windows(2).any(|pair| pair[0] == pair[1])
        || scenarios.windows(2).any(|pair| pair[0] == pair[1])
    {
        return Err(CanvasActionError::new(
            "Canvas action registry repeats a shard or scenario",
        ));
    }
    Ok(())
}

fn valid_name(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

fn selected_inputs(
    paths: &[String],
    changed_paths: &[String],
    include: fn(&str) -> bool,
) -> Vec<String> {
    let mut selected = paths
        .iter()
        .chain(changed_paths)
        .filter(|path| include(path))
        .cloned()
        .collect::<Vec<_>>();
    selected.sort();
    selected.dedup();
    selected
}

fn shard_input(path: &str) -> bool {
    matches!(path, "package.json" | "package-lock.json")
        || (path.starts_with("app/browser/") && path.ends_with(".mjs"))
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CanvasActionError(String);

impl CanvasActionError {
    fn new(detail: impl Into<String>) -> Self {
        Self(detail.into())
    }
}

impl fmt::Display for CanvasActionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for CanvasActionError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_registry_members_are_refused() {
        let registry = CanvasRegistry {
            version: 1,
            shards: vec![
                CanvasShard {
                    name: "same".to_owned(),
                    timeout_ms: 1,
                    scenarios: vec!["one".to_owned()],
                },
                CanvasShard {
                    name: "same".to_owned(),
                    timeout_ms: 1,
                    scenarios: vec!["two".to_owned()],
                },
            ],
        };
        assert!(validate_registry(&registry).is_err());
    }

    #[test]
    fn membership_and_entry_changes_move_only_the_declared_registry_authority() {
        let first = CanvasShard {
            name: "first".to_owned(),
            timeout_ms: 10,
            scenarios: vec!["one".to_owned()],
        };
        let second = CanvasShard {
            name: "second".to_owned(),
            timeout_ms: 20,
            scenarios: vec!["two".to_owned()],
        };
        let mut changed = first.clone();
        changed.timeout_ms += 1;

        assert_ne!(
            wire::encode(&first).expect("encode first entry"),
            wire::encode(&changed).expect("encode changed entry")
        );
        assert_eq!(
            wire::encode(&second).expect("encode unaffected entry"),
            wire::encode(&second.clone()).expect("encode retained entry")
        );
        let registry = CanvasRegistry {
            version: 1,
            shards: vec![first, second],
        };
        validate_registry(&registry).expect("distinct member is admitted");
        assert_eq!(
            registry
                .shards
                .iter()
                .map(CanvasShard::action_id)
                .collect::<Vec<_>>(),
            ["canvas:shard:first", "canvas:shard:second"]
        );
    }
}
