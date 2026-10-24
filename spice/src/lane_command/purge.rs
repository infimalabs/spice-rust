//! Retire the runtime residue of lanes the resident no longer seats.
//!
//! Removing a lane keeps its tape, supervisor records, RTK history, and media
//! projections because they are forensic. Purge is the operator's explicit
//! decision that they are no longer wanted: it asks the resident which lanes
//! are seated and retires every per-lane path whose lane is not among them.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::ErrorKind;
use std::path::Path;

use spice::ambient::Ambient;
use spice::repository::{Repository, StatePaths};
use wire::{CanonicalUuid, Fault, FaultClass, LanePurge, LanePurgeReport, Outcome, Reply};

use super::{environment_fault, read_lanes};

/// Retire every per-lane runtime path whose lane the resident no longer seats.
pub(crate) fn run(ambient: &Ambient) -> Outcome<LanePurgeReport> {
    let current = match std::env::current_dir() {
        Ok(current) => current,
        Err(error) => return environment_fault("lane::cwd-unavailable", &error.to_string()),
    };
    let repository = match Repository::discover(&current) {
        Ok(repository) => repository,
        Err(error) => {
            return environment_fault(spice::repository::ResolveError::REASON, &error.to_string());
        }
    };
    let state = match repository.admit_state() {
        Ok(state) => state,
        Err(error) => return crate::state_failure(&error).outcome(),
    };
    let lanes = match read_lanes(state.serve(), ambient) {
        Ok(lanes) => lanes,
        Err(outcome) => return outcome,
    };
    let seated = lanes
        .body
        .lanes
        .iter()
        .map(|row| row.lane)
        .collect::<BTreeSet<_>>();
    match retire(&state, &seated) {
        Ok(purged) => Outcome::Reply(Reply {
            position: lanes.position,
            body: LanePurgeReport {
                seated: seated.len() as u64,
                purged,
            },
        }),
        Err(fault) => Outcome::Fault(fault),
    }
}

/// Retire the residue of every lane outside `seated`, one read per family.
///
/// A child that is not named by a canonical lane identity is not residue and
/// stays; an absent family has nothing to retire.
fn retire(state: &StatePaths, seated: &BTreeSet<CanonicalUuid>) -> Result<Vec<LanePurge>, Fault> {
    let mut purged: BTreeMap<CanonicalUuid, Vec<String>> = BTreeMap::new();
    for family in state.lane_families() {
        let entries = match fs::read_dir(family) {
            Ok(entries) => entries,
            Err(error) if error.kind() == ErrorKind::NotFound => continue,
            Err(error) => return Err(store_fault(family, &error)),
        };
        for entry in entries {
            let entry = entry.map_err(|error| store_fault(family, &error))?;
            let Some(lane) = entry
                .file_name()
                .to_str()
                .and_then(|name| name.parse::<CanonicalUuid>().ok())
            else {
                continue;
            };
            if seated.contains(&lane) {
                continue;
            }
            let path = entry.path();
            let kind = entry
                .file_type()
                .map_err(|error| store_fault(&path, &error))?;
            let removed = if kind.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
            removed.map_err(|error| store_fault(&path, &error))?;
            purged.entry(lane).or_default().push(relative(state, &path));
        }
    }
    Ok(purged
        .into_iter()
        .map(|(lane, paths)| LanePurge { lane, paths })
        .collect())
}

fn relative(state: &StatePaths, path: &Path) -> String {
    path.strip_prefix(state.runtime())
        .unwrap_or(path)
        .display()
        .to_string()
}

fn store_fault(path: &Path, error: &std::io::Error) -> Fault {
    Fault {
        class: FaultClass::Store,
        reason: "lane::purge-failed".to_owned(),
        detail: format!("{}: {error}", path.display()),
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use spice::repository::{Repository, StatePaths};
    use wire::CanonicalUuid;

    use super::retire;

    const SEATED: &str = "11111111-1111-4111-8111-111111111111";
    const GONE: &str = "22222222-2222-4222-8222-222222222222";

    fn state() -> (PathBuf, StatePaths) {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-lane-purge-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join(".git")).expect("the fixture repository creates");
        let repository = Repository::discover(&root).expect("the fixture is a repository");
        let state = repository
            .admit_state()
            .expect("an absent state namespace admits");
        (root, state)
    }

    fn plant(family: &Path, name: &str, directory: bool) {
        let path = family.join(name);
        if directory {
            fs::create_dir_all(&path).expect("residue directory plants");
            fs::write(path.join("inner"), b"residue").expect("residue file plants");
        } else {
            fs::create_dir_all(family).expect("family plants");
            fs::write(&path, b"residue").expect("residue file plants");
        }
    }

    #[test]
    fn purge_retires_only_unseated_canonical_residue_across_every_family() {
        let (root, state) = state();
        for family in state.lane_families() {
            plant(family, SEATED, family != state.lane_serve());
            plant(family, GONE, family != state.lane_serve());
            plant(family, "notes", false);
        }
        let seated = BTreeSet::from([SEATED.parse::<CanonicalUuid>().expect("seated lane")]);

        let purged = retire(&state, &seated).expect("residue retires");

        assert_eq!(purged.len(), 1);
        assert_eq!(purged[0].lane.to_string(), GONE);
        assert_eq!(
            purged[0].paths,
            [
                "tape",
                "rtk",
                "supervisors",
                "steering-media",
                "serve-lanes"
            ]
            .map(|family| format!("{family}/{GONE}")),
        );
        for family in state.lane_families() {
            assert!(family.join(SEATED).exists(), "{}", family.display());
            assert!(family.join("notes").exists(), "{}", family.display());
            assert!(!family.join(GONE).exists(), "{}", family.display());
        }
        fs::remove_dir_all(root).expect("fixture cleanup");
    }

    #[test]
    fn purge_reports_nothing_when_no_family_exists_yet() {
        let (root, state) = state();
        let purged = retire(&state, &BTreeSet::new()).expect("absent families retire nothing");
        assert!(purged.is_empty());
        fs::remove_dir_all(root).expect("fixture cleanup");
    }
}
