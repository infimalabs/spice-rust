use std::sync::atomic::{AtomicU64, Ordering};

use super::*;
use crate::install::{self, Asset};

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-install-app-history-{}-{}",
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
        let stage = self.0.join(serve::APP_STAGE);
        let generation = stage.join("generations").join(identity.as_str());
        fs::create_dir_all(&generation).expect("fixture generation");
        for asset in serve::APP.assets() {
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
            .remove_test_operations(&repository, &[Asset::AppWork], generation.as_ref())
            .expect("recreate the receipt written before work.css existed");
        // Write the historical shape exactly as the previous build did, without
        // asking the current build to authorize it as a new installation.
        loaded.generation =
            write_record_bytes(&repository, &loaded.record, Some(loaded.generation))
                .expect("publish historical fixture receipt");
        loaded
    }

    fn install(&self) -> Digest {
        let repository = self.repository();
        let plan = install::plan(&repository, Some(&self.binary())).expect("initial plan");
        install::apply(&repository, Some(&self.binary()), &plan.digest().unwrap())
            .expect("initial install");
        plan.app_generation().unwrap().clone()
    }

    /// Stage and install one new app generation through the prepared route.
    fn refresh(&self, label: &[u8]) -> Digest {
        let repository = self.repository();
        let current = self.stage(label);
        let prepared = install::prepare(&repository, Some(&self.binary())).expect("refresh plan");
        install::apply_prepared(&repository, Some(&self.binary()), prepared)
            .expect("prepared refresh applies");
        current
    }

    fn installed_generations(&self) -> Vec<String> {
        let store = self
            .repository()
            .common_dir()
            .join(install::app::GENERATIONS_PATH);
        let mut generations = fs::read_dir(store)
            .expect("installed app store")
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        generations.sort();
        generations
    }

    fn receipt(&self) -> Vec<u8> {
        fs::read(record_path(&self.repository()).unwrap()).expect("install receipt")
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).expect("fixture cleanup");
    }
}

#[test]
fn app_history_before_work_css_refreshes_without_rewriting_old_generations() {
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
        install::installed_app_source(&repository, &install::installed_binary(&repository))
            .expect("new installed app is complete");
    let old_route = format!("{}/{previous}/style.css", serve::APP_GENERATION_ROUTE);
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
fn app_history_exception_does_not_admit_missing_base_assets_or_new_incomplete_plans() {
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
        .retain(|op| op.asset != Asset::AppChrome);
    assert!(
        loaded.record.plan.validate_historical_receipt().is_err(),
        "historical generations must still contain every original base asset"
    );
}

fn sorted(generations: &[&Digest]) -> Vec<String> {
    let mut names = generations
        .iter()
        .map(|generation| generation.as_str().to_owned())
        .collect::<Vec<_>>();
    names.sort();
    names
}

#[test]
fn app_history_stays_bounded_and_ordered_across_refreshes() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let mut history = vec![fixture.install()];
    let mut steady = None;
    // Retirement starts once the predecessors fill; two later rounds give two
    // steady-state shapes to compare, which is what boundedness needs.
    for round in 0..install::RETAINED_APP_PREDECESSORS as u8 + 3 {
        let current = fixture.refresh(&[b'r', round]);
        install::inspect(&repository).expect("bounded refresh verifies");
        let retained = &history[history
            .len()
            .saturating_sub(install::RETAINED_APP_PREDECESSORS)..];
        let plan = read_record(&repository).unwrap().unwrap().record.plan;
        assert_eq!(plan.app_predecessors, retained, "recency order is exact");
        let live = retained.iter().chain([&current]).collect::<Vec<_>>();
        assert_eq!(fixture.installed_generations(), sorted(&live));
        let source =
            install::installed_app_source(&repository, &install::installed_binary(&repository))
                .expect("installed app is complete");
        for generation in &history {
            let route = format!("{}/{generation}/style.css", serve::APP_GENERATION_ROUTE);
            assert_eq!(
                matches!(source.served(&route), Ok(Some(_))),
                retained.contains(generation),
                "exactly the retained generations remain served"
            );
        }
        history.push(current);
        if history.len() > install::RETAINED_APP_PREDECESSORS + 2 {
            let shape = (
                plan.operations.len(),
                plan.retirements.len(),
                fixture.receipt().len(),
            );
            let first = *steady.get_or_insert(shape);
            assert_eq!((shape.0, shape.1), (first.0, first.1));
            assert!(shape.2 <= first.2 + 256, "receipt bytes do not grow");
        }
    }
    install::unapply(&repository).expect("a receipt with retired history reverses");
}

#[test]
fn version_16_receipt_refreshes_and_retires_its_pre_rename_generations() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    let oldest = fixture.install();
    let installed = fixture.refresh(b"installed");
    install::publish_test_historical_receipt(&repository, install::PREVIOUS_RECORD_VERSION)
        .expect("version-16 receipt");
    let shell = |generation: &Digest, file: &str| {
        repository
            .common_dir()
            .join(".spice-install/app/generations")
            .join(generation.as_str())
            .join(file)
    };
    let current = fixture.stage(b"current");
    let plan = install::plan(&repository, Some(&fixture.binary())).unwrap();
    assert_eq!(
        plan.digest().unwrap(),
        install::plan(&repository, Some(&fixture.binary()))
            .unwrap()
            .digest()
            .unwrap()
    );
    assert_eq!(plan.app_predecessors, [oldest.clone(), installed.clone()]);
    assert!(
        plan.retirements
            .iter()
            .all(|retirement| install::app::generation_file(&retirement.path).is_none())
    );
    install::apply(
        &repository,
        Some(&fixture.binary()),
        &plan.digest().unwrap(),
    )
    .expect("version-16 asset names migrate on refresh");
    install::inspect(&repository).expect("migrated receipt verifies");
    assert_eq!(
        fixture.installed_generations(),
        sorted(&[&oldest, &installed, &current])
    );
    for retained in [&oldest, &installed] {
        assert!(shell(retained, "canvas-app.html").is_file());
        assert!(!shell(retained, "app.html").exists());
    }
    assert!(shell(&current, "app.html").is_file());

    let newer = fixture.refresh(b"newer");
    let newest = fixture.refresh(b"newest");
    install::inspect(&repository).expect("receipt verifies once pre-rename generations retire");
    assert_eq!(
        fixture.installed_generations(),
        sorted(&[&current, &newer, &newest])
    );
}

#[test]
fn interrupted_generation_retirement_resumes_to_completion() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    fixture.install();
    let mut retained = (0..install::RETAINED_APP_PREDECESSORS as u8)
        .map(|round| fixture.refresh(&[b'r', round]))
        .collect::<Vec<_>>();
    let current = fixture.stage(b"current");
    let plan = install::plan(&repository, Some(&fixture.binary())).unwrap();
    let first = plan
        .retirements
        .iter()
        .position(|retirement| install::app::generation_file(&retirement.path).is_some())
        .expect("the oldest generation retires");
    let digest = plan.digest().unwrap();
    let interrupted = install::progress::apply_with_interruption(
        &repository,
        Some(&fixture.binary()),
        &digest,
        Some(plan.operations.len() + first + 1),
    );
    assert!(matches!(interrupted, Err(InstallError::Interrupted { .. })));
    install::apply(&repository, Some(&fixture.binary()), &digest)
        .expect("the fenced run resumes from any retired file");
    install::inspect(&repository).expect("resumed receipt verifies");
    retained.push(current);
    assert_eq!(
        fixture.installed_generations(),
        sorted(&retained.iter().collect::<Vec<_>>())
    );
}

#[test]
fn prepared_install_refuses_changes_after_preview_before_mutating() {
    let fixture = Fixture::new();
    let repository = fixture.repository();
    fixture.install();
    let binary = fixture.binary();
    let installed_binary = install::installed_binary(&repository);
    fixture.stage(b"next");
    let refused = |mutate: &dyn Fn()| {
        let prepared = install::prepare(&repository, Some(&binary)).unwrap();
        mutate();
        let receipt = fixture.receipt();
        let executable = fs::read(&installed_binary).unwrap();
        let generations = fixture.installed_generations();
        assert!(install::apply_prepared(&repository, Some(&binary), prepared).is_err());
        assert_eq!(fixture.receipt(), receipt, "the receipt is untouched");
        assert_eq!(fs::read(&installed_binary).unwrap(), executable);
        assert_eq!(fixture.installed_generations(), generations);
    };
    // Source bytes.
    refused(&|| fs::write(&binary, b"#!/bin/sh\nexit 1\n").unwrap());
    fs::write(&binary, b"#!/bin/sh\nexit 0\n").unwrap();
    // Source generation.
    refused(&|| {
        fixture.stage(b"later");
    });
    // Receipt: another process applies the same refresh first.
    refused(&|| {
        let rival = install::prepare(&repository, Some(&binary)).unwrap();
        install::apply_prepared(&repository, Some(&binary), rival).unwrap();
    });
    // Destination file generation.
    fixture.stage(b"final");
    refused(&|| fs::write(&installed_binary, b"drift").unwrap());
}
