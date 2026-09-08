use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::str::FromStr;

use wire::manifest::CANVAS_APP_FILES;
use wire::{Digest, Domain};

use super::{Asset, DATA_MODE, GeneratedImage, InstallError, InstallPlan, Repository};

pub(super) const ASSETS: [(Asset, &str); 8] = [
    (Asset::CanvasAppModule, "app.module"),
    (Asset::CanvasWasm, "app_bg.wasm"),
    (Asset::CanvasHtml, "canvas-app.html"),
    (Asset::CanvasChrome, "chrome.css"),
    (Asset::CanvasComposer, "composer.css"),
    (Asset::CanvasMosaic, "mosaic.css"),
    (Asset::CanvasResponsive, "responsive.css"),
    (Asset::CanvasStyle, "style.css"),
];

pub(super) struct Source {
    generation: Digest,
    files: BTreeMap<Asset, Vec<u8>>,
}

impl Source {
    pub(super) fn resolve(repository: &Repository, binary: &Path) -> Result<Self, InstallError> {
        if let Some(source) = packaged(binary)? {
            return Ok(source);
        }
        let stage = repository.worktree_root().join(canvas::APP_STAGE);
        if cfg!(debug_assertions) && stage.is_dir() {
            return staged(repository.worktree_root());
        }
        Err(InstallError::BinaryContextChanged)
    }

    pub(super) const fn generation(&self) -> &Digest {
        &self.generation
    }

    pub(super) fn bytes(&self, asset: Asset) -> Option<Vec<u8>> {
        if asset == Asset::CanvasCurrent {
            return Some(format!("{}\n", self.generation).into_bytes());
        }
        self.files.get(&asset).cloned()
    }

    pub(super) fn differs_from(&self, plan: &InstallPlan) -> bool {
        if plan.app.as_ref() != Some(&self.generation) {
            return true;
        }
        ASSETS
            .into_iter()
            .map(|(asset, _)| asset)
            .chain([Asset::CanvasCurrent])
            .any(|asset| {
                let path = operation_path(asset, Some(&self.generation))
                    .expect("the source generation is present")
                    .expect("the source inventory contains only Canvas operations");
                let expected = self
                    .bytes(asset)
                    .map(|bytes| GeneratedImage::new(DATA_MODE, &bytes));
                plan.operations
                    .iter()
                    .find(|operation| operation.path == path)
                    .map(|operation| &operation.generated)
                    != expected.as_ref()
            })
    }
}

pub(super) fn file(asset: Asset) -> Option<&'static str> {
    ASSETS
        .iter()
        .find_map(|(candidate, file)| (*candidate == asset).then_some(*file))
}

pub(super) fn operation_generation(asset: Asset, path: &str) -> Option<Digest> {
    let file = file(asset)?;
    let suffix = format!("/{file}");
    let generation = path
        .strip_prefix(".spice-install/app/generations/")?
        .strip_suffix(&suffix)?;
    (!generation.contains('/'))
        .then(|| Digest::from_str(generation).ok())
        .flatten()
}

pub(super) fn operation_path(
    asset: Asset,
    generation: Option<&Digest>,
) -> Result<Option<String>, InstallError> {
    if asset == Asset::CanvasCurrent {
        return Ok(Some(super::APP_CURRENT_PATH.to_owned()));
    }
    let Some(file) = file(asset) else {
        return Ok(None);
    };
    let generation = generation.ok_or_else(|| {
        InstallError::InvalidRecord("Canvas operation has no generation identity".to_owned())
    })?;
    Ok(Some(format!(
        ".spice-install/app/generations/{generation}/{file}"
    )))
}

fn staged(root: &Path) -> Result<Source, InstallError> {
    let source =
        canvas::AppSource::resolve(root).map_err(|_| InstallError::BinaryContextChanged)?;
    let generation = source
        .current_generation()
        .map_err(|_| InstallError::BinaryContextChanged)?;
    let files = ASSETS
        .iter()
        .map(|(asset, file)| {
            let route = canvas::APP
                .asset_file(file)
                .expect("the install inventory matches Canvas");
            generation
                .served(route)
                .map(|served| (*asset, served.into_bytes()))
                .map_err(|_| InstallError::BinaryContextChanged)
        })
        .collect::<Result<_, _>>()?;
    Ok(Source {
        generation: generation.identity().clone(),
        files,
    })
}

fn packaged(binary: &Path) -> Result<Option<Source>, InstallError> {
    let Some((root, target_root)) = package_layout(binary) else {
        return Ok(None);
    };
    let payload_path = target_root.join("payload.json");
    let payload_bytes = fs::read(&payload_path)
        .map_err(|error| InstallError::io("read appliance payload", &payload_path, error))?;
    let admitted = proof::admit(&payload_bytes)
        .map_err(|error| InstallError::InvalidRecord(format!("appliance payload: {error}")))?;
    let payload = admitted.value();
    let target = target_root
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(InstallError::BinaryContextChanged)?;
    if payload.target.as_str() != target {
        return Err(InstallError::BinaryContextChanged);
    }
    let binary_bytes = super::read_binary(binary)?;
    if payload.native != Digest::compute(Domain::DistArtifact, &binary_bytes) {
        return Err(InstallError::BinaryContextChanged);
    }
    let files = packaged_files(root, payload)?;
    Ok(Some(Source {
        generation: payload.app.clone(),
        files,
    }))
}

fn package_layout(binary: &Path) -> Option<(&Path, &Path)> {
    let bin = binary
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "bin"))?;
    let target = bin.parent()?;
    let targets = target
        .parent()
        .filter(|path| path.file_name().is_some_and(|name| name == "targets"))?;
    let root = targets.parent()?;
    root.join("release/package.json")
        .is_file()
        .then_some((root, target))
}

fn packaged_files(
    root: &Path,
    payload: &wire::manifest::TargetPayload,
) -> Result<BTreeMap<Asset, Vec<u8>>, InstallError> {
    let mut files = BTreeMap::new();
    for (asset, file) in ASSETS {
        debug_assert!(CANVAS_APP_FILES.contains(&file));
        let path = root.join("app").join(file);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| InstallError::io("inspect appliance Canvas asset", &path, error))?;
        if !metadata.file_type().is_file() {
            return Err(InstallError::PathShape(path));
        }
        let bytes = fs::read(&path)
            .map_err(|error| InstallError::io("read appliance Canvas asset", &path, error))?;
        let entry = payload
            .inventory
            .iter()
            .find(|entry| entry.path == format!("app/{file}"))
            .ok_or_else(|| InstallError::InvalidRecord(format!("appliance has no app/{file}")))?;
        if entry.mode != 0o644
            || entry.bytes != bytes.len() as u64
            || entry.digest != Digest::compute(Domain::DistArtifact, &bytes)
        {
            return Err(InstallError::BinaryContextChanged);
        }
        files.insert(asset, bytes);
    }
    Ok(files)
}
