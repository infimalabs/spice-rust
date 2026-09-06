//! Shared Canvas build workspace overlays and deterministic cleanup.

use std::ffi::OsString;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Mutex;

use super::*;

pub(super) struct CanvasBuild {
    _workspace: CanvasWorkspace,
    pub(super) root: PathBuf,
    pub(super) binary: PathBuf,
    shard_inputs: Mutex<Option<Vec<String>>>,
}

impl CanvasBuild {
    pub(super) fn new(workspace: CanvasWorkspace, root: PathBuf, binary: PathBuf) -> Self {
        Self {
            _workspace: workspace,
            root,
            binary,
            shard_inputs: Mutex::new(None),
        }
    }

    pub(super) fn materialize_shard_inputs(
        &self,
        source: &Path,
        inputs: &[String],
    ) -> Result<(), ProveFailure> {
        let mut materialized = self.shard_inputs.lock().map_err(|error| {
            ProveFailure::fault("gate::prove-canvas-overlay-state-poisoned", error)
        })?;
        let mut normalized = inputs.to_vec();
        normalized.sort();
        normalized.dedup();
        if let Some(previous) = materialized.as_ref() {
            if previous == &normalized {
                return Ok(());
            }
            return Err(ProveFailure::refusal(
                "gate::prove-canvas-overlay-input-disagreement",
                "selected Canvas shards did not share one identical input closure",
            ));
        }
        copy_paths(source, &self.root, &normalized)?;
        index_paths(&self.root, &normalized)?;
        *materialized = Some(normalized);
        Ok(())
    }
}

pub(super) struct CanvasWorkspace(pub(super) PathBuf);

impl Drop for CanvasWorkspace {
    fn drop(&mut self) {
        make_workspace_removable(&self.0);
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn make_workspace_removable(path: &Path) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        set_owner_writable(path, &metadata, true);
        if let Ok(entries) = fs::read_dir(path) {
            for entry in entries.flatten() {
                make_workspace_removable(&entry.path());
            }
        }
    } else if metadata.is_file() {
        set_owner_writable(path, &metadata, false);
    }
}

fn set_owner_writable(path: &Path, _metadata: &fs::Metadata, directory: bool) {
    #[cfg(target_family = "unix")]
    {
        use std::os::unix::fs::PermissionsExt as _;

        let mode = if directory { 0o700 } else { 0o600 };
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(mode));
    }
    #[cfg(not(target_family = "unix"))]
    {
        let _ = directory;
        let mut permissions = _metadata.permissions();
        permissions.set_readonly(false);
        let _ = fs::set_permissions(path, permissions);
    }
}

pub(super) fn copy_paths(
    source: &Path,
    destination: &Path,
    paths: &[String],
) -> Result<(), ProveFailure> {
    let sources = paths
        .iter()
        .map(|relative| {
            let components = relative_components(relative)?;
            let (source, metadata) = source_file(source, relative, &components)?;
            Ok((relative.as_str(), components, source, metadata))
        })
        .collect::<Result<Vec<_>, ProveFailure>>()?;
    fs::create_dir_all(destination).map_err(|error| {
        ProveFailure::fault(
            "gate::prove-canvas-workspace-unavailable",
            format!("cannot create {}: {error}", destination.display()),
        )
    })?;
    for (relative, components, source, metadata) in sources {
        let destination = destination_path(destination, relative, &components)?;
        if identical_regular_file(&source, &metadata, &destination, relative)? {
            continue;
        }
        fs::copy(&source, &destination).map_err(|error| {
            ProveFailure::fault(
                "gate::prove-canvas-workspace-unavailable",
                format!(
                    "cannot copy {} to {}: {error}",
                    source.display(),
                    destination.display(),
                ),
            )
        })?;
    }
    Ok(())
}

fn relative_components(relative: &str) -> Result<Vec<OsString>, ProveFailure> {
    #[cfg(target_family = "windows")]
    let has_current_component = relative
        .split(['/', '\\'])
        .any(|component| component == ".");
    #[cfg(not(target_family = "windows"))]
    let has_current_component = relative.split('/').any(|component| component == ".");
    let components = Path::new(relative)
        .components()
        .map(|component| match component {
            Component::Normal(value) => Ok(value.to_owned()),
            _ => Err(()),
        })
        .collect::<Result<Vec<_>, _>>();
    match components {
        Ok(components) if !components.is_empty() && !has_current_component => Ok(components),
        _ => Err(ProveFailure::refusal(
            "gate::prove-canvas-source-refused",
            format!("selected Canvas input {relative:?} is not a clean relative path"),
        )),
    }
}

fn source_file(
    root: &Path,
    relative: &str,
    components: &[OsString],
) -> Result<(PathBuf, fs::Metadata), ProveFailure> {
    let mut source = root.to_path_buf();
    for component in &components[..components.len() - 1] {
        source.push(component);
        let metadata = fs::symlink_metadata(&source).map_err(|error| {
            ProveFailure::fault(
                "gate::prove-canvas-source-unreadable",
                format!("cannot inspect {}: {error}", source.display()),
            )
        })?;
        if !metadata.file_type().is_dir() {
            return Err(ProveFailure::refusal(
                "gate::prove-canvas-source-refused",
                format!(
                    "selected Canvas input {relative:?} crosses non-directory source component {}",
                    source.display(),
                ),
            ));
        }
    }
    source.push(components.last().expect("validated path has a leaf"));
    let metadata = fs::symlink_metadata(&source).map_err(|error| {
        ProveFailure::fault(
            "gate::prove-canvas-source-unreadable",
            format!("cannot inspect {}: {error}", source.display()),
        )
    })?;
    if !metadata.file_type().is_file() {
        return Err(ProveFailure::refusal(
            "gate::prove-canvas-source-refused",
            format!(
                "selected Canvas input {} is not a regular file",
                source.display()
            ),
        ));
    }
    Ok((source, metadata))
}

fn destination_path(
    root: &Path,
    relative: &str,
    components: &[OsString],
) -> Result<PathBuf, ProveFailure> {
    let mut destination = root.to_path_buf();
    for component in &components[..components.len() - 1] {
        destination.push(component);
        match fs::symlink_metadata(&destination) {
            Ok(metadata) if metadata.file_type().is_dir() => {}
            Ok(_) => {
                return Err(ProveFailure::refusal(
                    "gate::prove-canvas-input-collision",
                    format!(
                        "selected Canvas input {relative:?} crosses non-directory destination component {}",
                        destination.display(),
                    ),
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir(&destination).map_err(|error| {
                    ProveFailure::fault(
                        "gate::prove-canvas-workspace-unavailable",
                        format!("cannot create {}: {error}", destination.display()),
                    )
                })?;
            }
            Err(error) => {
                return Err(ProveFailure::fault(
                    "gate::prove-canvas-workspace-unavailable",
                    format!("cannot inspect {}: {error}", destination.display()),
                ));
            }
        }
    }
    destination.push(components.last().expect("validated path has a leaf"));
    Ok(destination)
}

fn identical_regular_file(
    source: &Path,
    source_metadata: &fs::Metadata,
    destination: &Path,
    relative: &str,
) -> Result<bool, ProveFailure> {
    let destination_metadata = match fs::symlink_metadata(destination) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
        Ok(metadata) => metadata,
        Err(error) => {
            return Err(ProveFailure::fault(
                "gate::prove-canvas-workspace-unavailable",
                format!("cannot inspect {}: {error}", destination.display()),
            ));
        }
    };
    if !destination_metadata.file_type().is_file() {
        return Err(ProveFailure::refusal(
            "gate::prove-canvas-input-collision",
            format!("selected Canvas input {relative:?} collides with a non-regular destination"),
        ));
    }
    let source_bytes = fs::read(source).map_err(|error| {
        ProveFailure::fault(
            "gate::prove-canvas-source-unreadable",
            format!("cannot read {}: {error}", source.display()),
        )
    })?;
    let destination_bytes = fs::read(destination).map_err(|error| {
        ProveFailure::fault(
            "gate::prove-canvas-workspace-unavailable",
            format!("cannot read {}: {error}", destination.display()),
        )
    })?;
    if source_bytes != destination_bytes
        || executable(source_metadata) != executable(&destination_metadata)
    {
        return Err(ProveFailure::refusal(
            "gate::prove-canvas-input-collision",
            format!(
                "selected Canvas input {relative:?} collides with different bytes or executable mode"
            ),
        ));
    }
    Ok(true)
}

#[cfg(target_family = "unix")]
fn executable(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;

    metadata.permissions().mode() & 0o111 != 0
}

#[cfg(not(target_family = "unix"))]
fn executable(metadata: &fs::Metadata) -> bool {
    !metadata.permissions().readonly()
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::{PermissionsExt as _, symlink};

    use super::*;

    fn success<T>(result: Result<T, ProveFailure>) -> T {
        match result {
            Ok(value) => value,
            Err(ProveFailure::Refusal { reason, detail })
            | Err(ProveFailure::Fault { reason, detail }) => {
                panic!("unexpected failure {reason}: {detail}")
            }
        }
    }

    fn failure(result: Result<(), ProveFailure>) -> (String, String) {
        match result.expect_err("fixture must refuse") {
            ProveFailure::Refusal { reason, detail } | ProveFailure::Fault { reason, detail } => {
                (reason, detail)
            }
        }
    }

    #[test]
    fn overlay_rejects_non_relative_spelling_before_creating_destination() {
        let source = CanvasWorkspace(success(exclusive_workspace("canvas-path-source")));
        fs::create_dir(&source.0).expect("create source workspace");
        fs::write(source.0.join("selected.mjs"), "selected\n").expect("write selected input");
        let absolute = source.0.join("selected.mjs").to_string_lossy().into_owned();
        let root = std::path::MAIN_SEPARATOR.to_string();

        for relative in [
            "",
            ".",
            "./selected.mjs",
            "nested/./selected.mjs",
            "../outside.mjs",
            "nested/../../outside.mjs",
            root.as_str(),
            absolute.as_str(),
        ] {
            let destination =
                CanvasWorkspace(success(exclusive_workspace("canvas-invalid-destination")));
            let (reason, detail) = failure(copy_paths(
                &source.0,
                &destination.0,
                &[relative.to_owned()],
            ));
            assert_eq!(reason, "gate::prove-canvas-source-refused");
            assert!(detail.contains(&format!("{relative:?}")), "{detail}");
            assert!(
                !destination.0.exists(),
                "{relative:?} created a destination"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn overlay_refuses_source_intermediate_symlink_without_reading_outside() {
        let source = CanvasWorkspace(success(exclusive_workspace("canvas-link-source")));
        let outside = CanvasWorkspace(success(exclusive_workspace("canvas-link-outside")));
        let destination = CanvasWorkspace(success(exclusive_workspace("canvas-link-destination")));
        fs::create_dir(&source.0).expect("create source workspace");
        fs::create_dir(&outside.0).expect("create outside workspace");
        fs::write(outside.0.join("sentinel.mjs"), "outside\n").expect("write outside sentinel");
        symlink(&outside.0, source.0.join("redirect")).expect("link source outside root");

        let (reason, detail) = failure(copy_paths(
            &source.0,
            &destination.0,
            &["redirect/sentinel.mjs".to_owned()],
        ));
        assert_eq!(reason, "gate::prove-canvas-source-refused");
        assert!(detail.contains("redirect/sentinel.mjs"), "{detail}");
        assert_eq!(
            fs::read_to_string(outside.0.join("sentinel.mjs")).expect("read outside sentinel"),
            "outside\n",
        );
        assert!(!destination.0.exists(), "refusal created a destination");
    }

    #[cfg(unix)]
    #[test]
    fn overlay_refuses_destination_intermediate_symlink_without_writing_outside() {
        let source = CanvasWorkspace(success(exclusive_workspace("canvas-link-source")));
        let outside = CanvasWorkspace(success(exclusive_workspace("canvas-link-outside")));
        let destination = CanvasWorkspace(success(exclusive_workspace("canvas-link-destination")));
        fs::create_dir_all(source.0.join("redirect")).expect("create source input parent");
        fs::write(source.0.join("redirect/selected.mjs"), "selected\n")
            .expect("write selected input");
        fs::create_dir(&outside.0).expect("create outside workspace");
        fs::write(outside.0.join("sentinel.mjs"), "outside\n").expect("write outside sentinel");
        fs::create_dir(&destination.0).expect("create destination workspace");
        symlink(&outside.0, destination.0.join("redirect")).expect("link destination outside root");

        let (reason, detail) = failure(copy_paths(
            &source.0,
            &destination.0,
            &["redirect/selected.mjs".to_owned()],
        ));
        assert_eq!(reason, "gate::prove-canvas-input-collision");
        assert!(detail.contains("redirect/selected.mjs"), "{detail}");
        assert_eq!(
            fs::read_to_string(outside.0.join("sentinel.mjs")).expect("read outside sentinel"),
            "outside\n",
        );
        assert!(!outside.0.join("selected.mjs").exists());
        assert!(
            fs::symlink_metadata(destination.0.join("redirect"))
                .expect("inspect redirect")
                .file_type()
                .is_symlink()
        );
    }

    #[cfg(unix)]
    #[test]
    fn repeated_read_only_overlay_accepts_only_identical_bytes_and_mode() {
        let source = CanvasWorkspace(success(exclusive_workspace("canvas-overlay-source")));
        let destination =
            CanvasWorkspace(success(exclusive_workspace("canvas-overlay-destination")));
        fs::create_dir(&source.0).expect("create source workspace");
        let selected = source.0.join("shared.mjs");
        fs::write(&selected, "selected\n").expect("write selected input");
        fs::set_permissions(&selected, fs::Permissions::from_mode(0o400))
            .expect("seal selected input");

        let paths = ["shared.mjs".to_owned()];
        success(copy_paths(&source.0, &destination.0, &paths));
        success(copy_paths(&source.0, &destination.0, &paths));

        let copied = destination.0.join("shared.mjs");
        fs::set_permissions(&copied, fs::Permissions::from_mode(0o600))
            .expect("open copied input for mutation");
        fs::write(&copied, "changed\n").expect("mutate copied input");
        fs::set_permissions(&copied, fs::Permissions::from_mode(0o400))
            .expect("reseal changed input");
        let (reason, detail) = failure(copy_paths(&source.0, &destination.0, &paths));
        assert_eq!(reason, "gate::prove-canvas-input-collision");
        assert!(detail.contains("shared.mjs"), "{detail}");
        assert!(
            detail.contains("different bytes or executable mode"),
            "{detail}"
        );

        fs::set_permissions(&copied, fs::Permissions::from_mode(0o600))
            .expect("open copied input for mode mutation");
        fs::write(&copied, "selected\n").expect("restore selected bytes");
        fs::set_permissions(&copied, fs::Permissions::from_mode(0o500))
            .expect("change copied executable mode");
        let (reason, detail) = failure(copy_paths(&source.0, &destination.0, &paths));
        assert_eq!(reason, "gate::prove-canvas-input-collision");
        assert!(detail.contains("shared.mjs"), "{detail}");

        let other = source.0.join("other.mjs");
        fs::write(&other, "other\n").expect("write second selected input");
        fs::create_dir(destination.0.join("other.mjs"))
            .expect("create non-file destination collision");
        let (reason, detail) = failure(copy_paths(
            &source.0,
            &destination.0,
            &["other.mjs".to_owned()],
        ));
        assert_eq!(reason, "gate::prove-canvas-input-collision");
        assert!(detail.contains("non-regular destination"), "{detail}");
    }

    #[cfg(unix)]
    #[test]
    fn shared_build_serializes_overlapping_shard_overlays() {
        let source = CanvasWorkspace(success(exclusive_workspace("canvas-shard-source")));
        let destination = success(exclusive_workspace("canvas-shared-build"));
        fs::create_dir(&source.0).expect("create source workspace");
        fs::create_dir(&destination).expect("create shared build workspace");
        success(initialize_git_index(&destination));
        for path in ["shared.mjs", "first.mjs", "second.mjs"] {
            let selected = source.0.join(path);
            fs::write(&selected, format!("{path}\n")).expect("write selected shard input");
            fs::set_permissions(&selected, fs::Permissions::from_mode(0o400))
                .expect("seal selected shard input");
        }
        let build = Arc::new(CanvasBuild::new(
            CanvasWorkspace(destination.clone()),
            destination,
            PathBuf::from("unused-spice"),
        ));
        let source_root = source.0.clone();
        std::thread::scope(|scope| {
            for _ in 0..2 {
                let build = Arc::clone(&build);
                let source_root = source_root.clone();
                scope.spawn(move || {
                    success(build.materialize_shard_inputs(
                        &source_root,
                        &[
                            "shared.mjs".to_owned(),
                            "first.mjs".to_owned(),
                            "second.mjs".to_owned(),
                        ],
                    ));
                });
            }
        });

        assert_eq!(
            build.shard_inputs.lock().expect("overlay state").as_deref(),
            Some(
                [
                    "first.mjs".to_owned(),
                    "second.mjs".to_owned(),
                    "shared.mjs".to_owned(),
                ]
                .as_slice(),
            ),
        );
        for path in ["shared.mjs", "first.mjs", "second.mjs"] {
            assert_eq!(
                fs::read_to_string(build.root.join(path)).expect("read materialized shard input"),
                format!("{path}\n"),
            );
        }
        let (reason, detail) =
            failure(build.materialize_shard_inputs(&source.0, &["shared.mjs".to_owned()]));
        assert_eq!(reason, "gate::prove-canvas-overlay-input-disagreement");
        assert!(detail.contains("identical input closure"), "{detail}");
    }

    #[cfg(unix)]
    #[test]
    fn shard_overlay_enters_the_private_git_index_before_execution() {
        let source = CanvasWorkspace(success(exclusive_workspace("canvas-index-source")));
        let destination = success(exclusive_workspace("canvas-index-build"));
        fs::create_dir_all(source.0.join("app/browser")).expect("create selected browser path");
        fs::create_dir(&destination).expect("create build workspace");
        success(initialize_git_index(&destination));
        let launch = source.0.join("app/browser/headless-launch.mjs");
        fs::write(&launch, "const launchBoundary = true;\n").expect("write shard-only source");
        fs::set_permissions(&launch, fs::Permissions::from_mode(0o400))
            .expect("seal shard-only source");
        let build = CanvasBuild::new(
            CanvasWorkspace(destination.clone()),
            destination,
            PathBuf::from("unused-spice"),
        );

        success(
            build.materialize_shard_inputs(
                &source.0,
                &["app/browser/headless-launch.mjs".to_owned()],
            ),
        );
        let output = Command::new("/usr/bin/git")
            .args([
                "grep",
                "-F",
                "launchBoundary",
                "--",
                "app/browser/headless-launch.mjs",
            ])
            .current_dir(&build.root)
            .output()
            .expect("run private index grep");
        assert!(output.status.success(), "{}", process_failure(&output));
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            "app/browser/headless-launch.mjs:const launchBoundary = true;\n",
        );
    }

    #[cfg(unix)]
    #[test]
    fn canvas_workspace_drop_removes_immutable_descendants() {
        let root = success(exclusive_workspace("canvas-immutable-cleanup"));
        {
            let _workspace = CanvasWorkspace(root.clone());
            let nested = root.join("sealed");
            fs::create_dir_all(&nested).expect("create cleanup fixture");
            let file = nested.join("input.rs");
            fs::write(&file, "selected\n").expect("write cleanup input");
            fs::set_permissions(&file, fs::Permissions::from_mode(0o400))
                .expect("seal cleanup input");
            fs::set_permissions(&nested, fs::Permissions::from_mode(0o500))
                .expect("seal cleanup directory");
            fs::set_permissions(&root, fs::Permissions::from_mode(0o500))
                .expect("seal cleanup root");
        }
        assert!(!root.exists(), "immutable Canvas workspace survived drop");
    }
}
