//! Closed lane app fixture for process tests that start a resident.

use std::fs;
use std::path::Path;

pub(super) fn seed(root: &Path) {
    let stage = root.join(canvas::LANE_STAGE);
    fs::create_dir_all(&stage).expect("fixture app stage directory");
    for asset in canvas::APP.assets() {
        fs::write(
            stage.join(asset.file()),
            format!("fixture lane app {}\n", asset.file()),
        )
        .expect("fixture app asset");
    }
}
