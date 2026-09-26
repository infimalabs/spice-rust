use std::borrow::Cow;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use wire::{Digest, Domain};

use super::{Asset, DATA_MODE, GeneratedImage, InstallError, InstallPlan, Repository};

pub(super) const ASSETS: [(Asset, &str); 9] = [
    (Asset::CanvasAppModule, "app.module"),
    (Asset::CanvasWasm, "app_bg.wasm"),
    (Asset::CanvasHtml, "canvas-app.html"),
    (Asset::CanvasChrome, "chrome.css"),
    (Asset::CanvasComposer, "composer.css"),
    (Asset::CanvasMosaic, "mosaic.css"),
    (Asset::CanvasResponsive, "responsive.css"),
    (Asset::CanvasStyle, "style.css"),
    (Asset::CanvasWork, "work.css"),
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

    pub(super) fn assets(&self) -> impl Iterator<Item = Asset> + '_ {
        self.files.keys().copied()
    }

    pub(super) fn bytes(&self, asset: Asset) -> Option<Vec<u8>> {
        if asset == Asset::CanvasCurrent {
            return Some(format!("{}\n", self.generation).into_bytes());
        }
        self.files.get(&asset).cloned()
    }

    pub(super) fn image(&self, asset: Asset) -> Option<GeneratedImage> {
        if asset == Asset::CanvasCurrent {
            return Some(GeneratedImage::new(
                DATA_MODE,
                format!("{}\n", self.generation).as_bytes(),
            ));
        }
        self.files
            .get(&asset)
            .map(|bytes| GeneratedImage::new(DATA_MODE, bytes))
    }

    pub(super) fn differs_from(&self, plan: &InstallPlan) -> bool {
        if plan.app.as_ref() != Some(&self.generation) {
            return true;
        }
        let installed = plan
            .operations
            .iter()
            .map(|operation| (operation.path.as_str(), &operation.generated))
            .collect::<BTreeMap<_, _>>();
        self.assets().chain([Asset::CanvasCurrent]).any(|asset| {
            let path = operation_path(asset, Some(&self.generation))
                .expect("the source generation is present")
                .expect("the source inventory contains only Canvas operations");
            let expected = self.image(asset);
            installed.get(path.as_str()).copied() != expected.as_ref()
        })
    }
}

pub(super) fn file(asset: Asset) -> Option<Cow<'static, str>> {
    if let Asset::CanvasRenderer(file) = asset {
        return Some(Cow::Owned(file.to_string()));
    }
    ASSETS
        .iter()
        .find_map(|(candidate, file)| (*candidate == asset).then_some(Cow::Borrowed(*file)))
}

pub(super) fn asset_file(file: &str) -> Option<Asset> {
    ASSETS
        .iter()
        .find_map(|(asset, candidate)| (*candidate == file).then_some(*asset))
        .or_else(|| wire::manifest::renderer::File::parse_filename(file).map(Asset::CanvasRenderer))
}

/// Common-directory root under which every installed Canvas generation lives.
pub(super) const GENERATIONS_PATH: &str = ".spice-install/app/generations";

pub(super) fn generation_spelling(asset: Asset, path: &str) -> Option<&str> {
    let (generation, file) = split_generation_path(path)?;
    (asset_file(file) == Some(asset)).then_some(generation)
}

/// Generation and file named by any installed Canvas generation file path.
pub(super) fn generation_file(path: &str) -> Option<(&str, &str)> {
    split_generation_path(path).filter(|(_, file)| asset_file(file).is_some())
}

fn split_generation_path(path: &str) -> Option<(&str, &str)> {
    path.strip_prefix(GENERATIONS_PATH)?
        .strip_prefix('/')?
        .split_once('/')
}

pub(super) fn operation_generation(asset: Asset, path: &str) -> Option<Digest> {
    generation_spelling(asset, path)?.parse().ok()
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
    Ok(Some(format!("{GENERATIONS_PATH}/{generation}/{file}")))
}

fn staged(root: &Path) -> Result<Source, InstallError> {
    let source =
        canvas::AppSource::resolve(root).map_err(|_| InstallError::BinaryContextChanged)?;
    let generation = source
        .current_generation()
        .map_err(|_| InstallError::BinaryContextChanged)?;
    let mut files = ASSETS
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
        .collect::<Result<BTreeMap<_, _>, _>>()?;
    files.extend(
        generation
            .renderers()
            .map(|(file, served)| (Asset::CanvasRenderer(file), served.bytes().to_vec())),
    );
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

pub(super) fn package_layout(binary: &Path) -> Option<(&Path, &Path)> {
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
    let mut renderer_bytes = 0_u64;
    for (asset, file, entry) in payload.inventory.iter().filter_map(|entry| {
        let file = entry.path.strip_prefix("app/")?;
        Some((asset_file(file)?, file, entry))
    }) {
        let path = root.join("app").join(file);
        let metadata = fs::symlink_metadata(&path)
            .map_err(|error| InstallError::io("inspect appliance Canvas asset", &path, error))?;
        if !metadata.file_type().is_file() {
            return Err(InstallError::PathShape(path));
        }
        if matches!(asset, Asset::CanvasRenderer(_)) {
            renderer_bytes = renderer_bytes
                .checked_add(metadata.len())
                .filter(|total| *total <= wire::manifest::renderer::MAX_BYTES)
                .ok_or(InstallError::BinaryContextChanged)?;
        }
        if metadata.len() != entry.bytes {
            return Err(InstallError::BinaryContextChanged);
        }
        let bytes = fs::read(&path)
            .map_err(|error| InstallError::io("read appliance Canvas asset", &path, error))?;
        if entry.mode != 0o644
            || entry.bytes != bytes.len() as u64
            || entry.digest != Digest::compute(Domain::DistArtifact, &bytes)
        {
            return Err(InstallError::BinaryContextChanged);
        }
        files.insert(asset, bytes);
    }
    wire::manifest::renderer::validate_bundle(files.iter().map(|(asset, bytes)| {
        (
            file(*asset).expect("only app files entered the source"),
            bytes.as_slice(),
        )
    }))
    .map_err(|error| InstallError::InvalidRecord(error.to_owned()))?;
    Ok(files)
}
