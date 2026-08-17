use std::os::unix::fs::symlink;

use super::*;

#[test]
fn renamed_parent_at_publication_is_restored_on_refusal() {
    let root = std::env::temp_dir().join(format!(
        "spice-admitted-path-{}-{}",
        std::process::id(),
        TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed),
    ));
    let target = root.join("target");
    let parked = root.join("parked");
    let outside = root.join("outside");
    fs::create_dir_all(&target).unwrap();
    fs::create_dir(&outside).unwrap();
    fs::write(target.join("file"), b"before").unwrap();
    let metadata = fs::metadata(&target).unwrap();
    let parent = ParentObservation {
        ancestor: "target".to_owned(),
        generation: Generation::observe(&metadata),
        missing: String::new(),
    };
    let admitted = AdmittedPath::open(&root, "target/file", &parent, false, 0).unwrap();
    let expected = admitted.observe().unwrap().unwrap().generation;

    let error = admitted
        .publish_with(
            &FileImage::new(0o644, b"after".to_vec()),
            Some(expected),
            || {
                fs::rename(&target, &parked).unwrap();
                symlink(&outside, &target).unwrap();
                Ok(())
            },
        )
        .unwrap_err();

    assert!(matches!(error, InstallError::ParentChanged(_)));
    assert_eq!(fs::read(parked.join("file")).unwrap(), b"before");
    assert!(fs::read_dir(&outside).unwrap().next().is_none());
    drop(admitted);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replaced_destination_at_publication_is_restored_on_refusal() {
    let root = test_root("publish-replacement");
    let target = root.join("target");
    let destination = target.join("file");
    let parked = target.join("parked");
    fs::create_dir_all(&target).unwrap();
    fs::write(&destination, b"before").unwrap();
    let admitted = admitted_file(&root, &target, "target/file");
    let expected = admitted.observe().unwrap().unwrap().generation;

    let error = admitted
        .publish_with(
            &FileImage::new(0o644, b"after".to_vec()),
            Some(expected),
            || {
                fs::rename(&destination, &parked).unwrap();
                fs::write(&destination, b"racer").unwrap();
                Ok(())
            },
        )
        .unwrap_err();

    assert!(matches!(error, InstallError::ParentChanged(_)));
    assert_eq!(fs::read(&destination).unwrap(), b"racer");
    assert_eq!(fs::read(&parked).unwrap(), b"before");
    drop(admitted);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn appeared_destination_at_publication_is_unchanged_on_refusal() {
    let root = test_root("publish-appearance");
    let target = root.join("target");
    let destination = target.join("file");
    fs::create_dir_all(&target).unwrap();
    let admitted = admitted_file(&root, &target, "target/file");

    let error = admitted
        .publish_with(&FileImage::new(0o644, b"after".to_vec()), None, || {
            fs::write(&destination, b"racer").unwrap();
            Ok(())
        })
        .unwrap_err();

    assert!(matches!(error, InstallError::ParentChanged(_)));
    assert_eq!(fs::read(&destination).unwrap(), b"racer");
    drop(admitted);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn receipt_publication_rejects_reused_native_generation_with_different_content() {
    let root = test_root("receipt-generation-aba");
    let target = root.join("target");
    let destination = target.join("file");
    fs::create_dir_all(&target).unwrap();
    fs::write(&destination, b"winner receipt").unwrap();
    let admitted = admitted_file(&root, &target, "target/file");
    let winner = admitted.observe().unwrap().unwrap();
    let stale = ReceiptGeneration::published(
        winner.generation,
        &FileImage::new(0o644, b"stale receipt".to_vec()),
    );

    assert_eq!(stale.native(), winner.generation);
    assert!(!stale.matches(&winner));
    let error = admitted
        .publish_receipt(
            &FileImage::new(0o644, b"loser receipt".to_vec()),
            Some(stale),
        )
        .unwrap_err();

    assert!(matches!(error, InstallError::GenerationChanged(_)));
    assert_eq!(fs::read(&destination).unwrap(), b"winner receipt");
    drop(admitted);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn receipt_publication_restores_a_different_winner_from_the_final_exchange_window() {
    let root = test_root("receipt-final-exchange-aba");
    let target = root.join("target");
    let destination = target.join("file");
    fs::create_dir_all(&target).unwrap();
    fs::write(&destination, b"expected receipt").unwrap();
    let admitted = admitted_file(&root, &target, "target/file");
    let expected_file = admitted.observe().unwrap().unwrap();
    let expected = ReceiptGeneration::observed(&expected_file);

    let error = admitted
        .publish_receipt_with(
            &FileImage::new(0o644, b"loser receipt".to_vec()),
            Some(expected.clone()),
            || {
                fs::write(&destination, b"winner receipt").unwrap();
                Ok(())
            },
        )
        .unwrap_err();

    let winner = admitted.observe().unwrap().unwrap();
    assert_eq!(winner.generation, expected.native());
    assert!(!expected.matches(&winner));
    assert!(matches!(error, InstallError::GenerationChanged(_)));
    assert_eq!(winner.image.bytes(), b"winner receipt");
    assert_eq!(fs::read_dir(&target).unwrap().count(), 1);
    drop(admitted);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replaced_destination_at_removal_is_restored_on_refusal() {
    let root = test_root("remove-replacement");
    let target = root.join("target");
    let destination = target.join("file");
    let parked = target.join("parked");
    fs::create_dir_all(&target).unwrap();
    fs::write(&destination, b"before").unwrap();
    let admitted = admitted_file(&root, &target, "target/file");
    let expected = admitted.observe().unwrap().unwrap().generation;

    let error = admitted
        .remove_with(expected, || {
            fs::rename(&destination, &parked).unwrap();
            fs::write(&destination, b"racer").unwrap();
            Ok(())
        })
        .unwrap_err();

    assert!(matches!(error, InstallError::ParentChanged(_)));
    assert_eq!(fs::read(&destination).unwrap(), b"racer");
    assert_eq!(fs::read(&parked).unwrap(), b"before");
    drop(admitted);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replaced_new_directory_is_refused_by_descriptor_identity() {
    let root = test_root("create-replacement");
    fs::create_dir_all(&root).unwrap();
    let parent = unix_fs::open(
        &root,
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )
    .map(File::from)
    .unwrap();
    let name = OsString::from("created");
    let created = root.join(&name);
    let parked = root.join("parked");

    let error = create_directory(&parent, &name, &created, || {
        fs::rename(&created, &parked).unwrap();
        fs::create_dir(&created).unwrap();
        fs::write(created.join("racer"), b"outside").unwrap();
        Ok(())
    })
    .unwrap_err();

    assert!(matches!(error, InstallError::ParentChanged(_)));
    assert!(parked.is_dir());
    assert_eq!(fs::read(created.join("racer")).unwrap(), b"outside");
    drop(parent);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn planned_missing_directory_that_appears_before_open_is_refused() {
    let root = test_root("missing-appearance");
    fs::create_dir_all(&root).unwrap();
    let parent = ParentObservation {
        ancestor: String::new(),
        generation: Generation::observe(&fs::metadata(&root).unwrap()),
        missing: "created/nested".to_owned(),
    };
    let created = root.join("created");
    fs::create_dir(&created).unwrap();
    fs::write(created.join("racer"), b"outside").unwrap();

    let admitted = AdmittedPath::open(&root, "created/nested/file", &parent, true, 2).unwrap();
    let error = admitted
        .publish(&FileImage::new(0o644, b"after".to_vec()), None)
        .unwrap_err();

    assert!(matches!(error, InstallError::ParentChanged(_)));
    assert_eq!(fs::read(created.join("racer")).unwrap(), b"outside");
    assert!(!created.join("nested").exists());
    drop(admitted);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn replaced_introduced_directory_at_cleanup_is_restored_on_refusal() {
    let root = test_root("cleanup-replacement");
    fs::create_dir_all(&root).unwrap();
    let parent = ParentObservation {
        ancestor: String::new(),
        generation: Generation::observe(&fs::metadata(&root).unwrap()),
        missing: "created".to_owned(),
    };
    let created = root.join("created");
    let parked = root.join("parked");
    let admitted = AdmittedPath::open(&root, "created/file", &parent, true, 1).unwrap();

    let error = admitted
        .cleanup_directory_with(0, &created, || {
            fs::rename(&created, &parked).unwrap();
            fs::create_dir(&created).unwrap();
            fs::write(created.join("racer"), b"outside").unwrap();
            Ok(())
        })
        .unwrap_err();

    assert!(matches!(error, InstallError::ParentChanged(_)));
    assert!(fs::read_dir(&parked).unwrap().next().is_none());
    assert_eq!(fs::read(created.join("racer")).unwrap(), b"outside");
    drop(admitted);
    fs::remove_dir_all(root).unwrap();
}

fn admitted_file(root: &Path, target: &Path, destination: &str) -> AdmittedPath {
    let parent = ParentObservation {
        ancestor: "target".to_owned(),
        generation: Generation::observe(&fs::metadata(target).unwrap()),
        missing: String::new(),
    };
    AdmittedPath::open(root, destination, &parent, false, 0).unwrap()
}

fn test_root(case: &str) -> PathBuf {
    std::env::temp_dir().join(format!(
        "spice-admitted-path-{case}-{}-{}",
        std::process::id(),
        TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed),
    ))
}
