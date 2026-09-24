use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::install::{self, Asset};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-install-canvas-history-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).expect("exclusive fixture root");
        let root = fs::canonicalize(root).expect("canonical fixture root");
        gix::init(&root).expect("fixture repository");
        let fixture = Self(root);
        fixture.stage(b"before work.css");
        fs::write(fixture.binary(), b"#!/bin/sh\nexit 0\n").expect("fixture executable");
        fixture
    }

    fn repository(&self) -> Repository {
        Repository::discover(&self.0).expect("fixture repository resolves")
    }

    fn binary(&self) -> PathBuf {
        self.0.join("spice")
    }

    fn stage(&self, label: &[u8]) -> Digest {
        let identity = Digest::compute(wire::Domain::DistInventory, label);
        let stage = self.0.join(canvas::APP_STAGE);
        let generation = stage.join("generations").join(identity.as_str());
        fs::create_dir_all(&generation).expect("fixture generation");
        for asset in canvas::APP.assets() {
            fs::write(generation.join(asset.file()), asset.file()).expect("fixture app asset");
        }
        fs::write(stage.join("current"), format!("{identity}\n")).expect("fixture pointer");
        identity
    }

    fn legacy_record(&self) -> LoadedRecord {
        let repository = self.repository();
        let plan = install::plan(&repository, Some(&self.binary())).expect("initial plan");
        install::apply(&repository, Some(&self.binary()), &plan.digest().unwrap())
            .expect("initial install");
        let mut loaded = read_record(&repository).unwrap().unwrap();
        let generation = loaded.record.plan.app.clone();
        loaded
            .record
            .remove_test_operations(&repository, &[Asset::CanvasWork], generation.as_ref())
            .expect("recreate the receipt written before work.css existed");
        // Write the historical shape exactly as the previous build did, without
        // asking the current build to authorize it as a new installation.
        loaded.generation =
            write_record_bytes(&repository, &loaded.record, Some(loaded.generation))
                .expect("publish historical fixture receipt");
        loaded
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("fixture cleanup");
    }
}

#[test]
fn canvas_history_before_work_css_refreshes_without_rewriting_old_generations() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let loaded = fixture.legacy_record();
    let previous = loaded.record.plan.app.as_ref().unwrap();
    let previous_assets = loaded
        .record
        .plan
        .operations
        .iter()
        .filter(|operation| install::app::file(operation.asset).is_some())
        .map(|operation| (operation.path.clone(), operation.generated.clone()))
        .collect::<Vec<_>>();
    install::inspect(&repository).expect("the intact historical receipt remains readable");

    let current = fixture.stage(b"with work.css");
    let plan = install::plan(&repository, Some(&fixture.binary()))
        .expect("a pre-work.css receipt can plan a current refresh");
    assert_eq!(plan.app_generation(), Some(&current));
    install::apply(
        &repository,
        Some(&fixture.binary()),
        &plan.digest().unwrap(),
    )
    .expect("refresh retains the old generation and installs the new one");
    install::inspect(&repository).expect("refreshed receipt verifies");
    for (path, generated) in previous_assets {
        let operation = plan.operations.iter().find(|op| op.path == path).unwrap();
        assert_eq!(operation.generated, generated);
        let bytes = fs::read(repository.common_dir().join(path)).unwrap();
        assert_eq!(
            generated,
            install::GeneratedImage::new(install::DATA_MODE, &bytes)
        );
    }
    assert!(
        !repository
            .common_dir()
            .join(format!(
                ".spice-install/app/generations/{previous}/work.css"
            ))
            .exists()
    );
    let source =
        install::installed_canvas_source(&repository, &install::installed_binary(&repository))
            .expect("new installed app is complete");
    let old_route = format!("{}/{previous}/style.css", canvas::APP_GENERATION_ROUTE);
    assert_eq!(
        source
            .served(&old_route)
            .expect("old pages retain their assets")
            .unwrap()
            .bytes(),
        b"style.css"
    );
}

#[test]
fn canvas_history_exception_does_not_admit_missing_base_assets_or_new_incomplete_plans() {
    let fixture = Fixture::new();
    let mut loaded = fixture.legacy_record();
    assert!(
        loaded.record.plan.validate().is_err(),
        "new plans still require work.css"
    );
    loaded
        .record
        .plan
        .operations
        .retain(|op| op.asset != Asset::CanvasChrome);
    assert!(
        loaded.record.plan.validate_historical_receipt().is_err(),
        "historical generations must still contain every original base asset"
    );
}
