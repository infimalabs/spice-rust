//! Closed atomic app-generation fixture for process tests that start a resident.

use std::fs;
use std::io;
use std::path::Path;

const FIXTURE_GENERATION: &str = "4444444444444444444444444444444444444444444444444444444444444444";

pub(super) fn seed(root: &Path) {
    try_seed(root).expect("fixture app generation");
}

pub(super) fn try_seed(root: &Path) -> io::Result<()> {
    let stage = root.join(canvas::APP_STAGE);
    let generation = stage
        .join(canvas::APP_STAGE_GENERATIONS)
        .join(FIXTURE_GENERATION);
    fs::create_dir_all(&generation)?;
    for asset in canvas::APP.assets() {
        fs::write(
            generation.join(asset.file()),
            format!("fixture lane app {}\n", asset.file()),
        )?;
    }
    fs::write(
        stage.join(canvas::APP_STAGE_CURRENT),
        format!("{FIXTURE_GENERATION}\n"),
    )?;
    Ok(())
}
