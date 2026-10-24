use std::collections::BTreeMap;
use std::sync::mpsc;

use super::*;

fn fixture(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!(
        "spice-xtask-{name}-{}-{}",
        std::process::id(),
        NEXT_BUILD.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&root).expect("create fixture root");
    root
}

fn fixture_source() -> Digest {
    Digest::compute(Domain::DistInventory, b"fixture app sources")
}

fn write_raw_generation(stage: &Path, label: &str, source_identity: &Digest, valid: bool) {
    fs::create_dir_all(stage).expect("create raw generation");
    fs::write(stage.join("app.js"), format!("module {label}")).expect("write generated module");
    fs::write(
        stage.join("app_bg.wasm"),
        format!("wasm {source_identity} {label}"),
    )
    .expect("write wasm");
    fs::write(stage.join("style.css"), format!("style {label}")).expect("write stylesheet");
    fs::write(
        stage.join("spice_sidebar.css"),
        format!("spice sidebar {label}"),
    )
    .expect("write Spice sidebar stylesheet");
    fs::write(stage.join("mosaic.css"), format!("mosaic {label}"))
        .expect("write Mosaic stylesheet");
    fs::write(stage.join("chrome.css"), format!("chrome {label}"))
        .expect("write chrome stylesheet");
    fs::write(stage.join("composer.css"), format!("composer {label}"))
        .expect("write composer stylesheet");
    fs::write(stage.join("work.css"), format!("work {label}")).expect("write Work stylesheet");
    fs::write(stage.join("responsive.css"), format!("responsive {label}"))
        .expect("write responsive stylesheet");
    let second_wasm = if valid {
        "<link href=\"/app_bg.wasm\">"
    } else {
        ""
    };
    fs::write(
        stage.join("app.html"),
        format!(
            "<link href=\"/style.css\"><link href=\"/spice_sidebar.css\">\
             <link href=\"/mosaic.css\"><link href=\"/chrome.css\">\
             <link href=\"/composer.css\"><link href=\"/work.css\"><link href=\"/responsive.css\">\
             <script src=\"/app.js\"></script>\
             <link href=\"/app.js\"><link href=\"/app_bg.wasm\">{second_wasm}\
             <main id=\"app\" data-app-generation=\"{APP_GENERATION_PLACEHOLDER}\">{label}</main>"
        ),
    )
    .expect("write shell");
}

fn publish_fixture(root: &Path, label: &str) {
    let sources = fixture_source();
    app_at(
        root,
        &sources,
        &mut Vec::new(),
        |stage, _| {
            write_raw_generation(stage, label, &sources, true);
            Ok(())
        },
        || Ok(()),
    )
    .expect("fixture generation publishes");
}

fn fail_partial_build(stage: &Path) -> Result<(), AppBuildError> {
    fs::create_dir_all(stage).expect("create partial build");
    fs::write(stage.join("app.html"), b"partial build").expect("write partial build");
    Err(AppBuildError::Exit(Some(42)))
}

#[test]
fn source_edits_reuse_one_worktree_cache_namespace() {
    let outer = fixture("stable-app-target");
    let worktree = Digest::compute(Domain::DistInventory, b"stable worktree");
    let first_source = Digest::compute(Domain::DistInventory, b"first source revision");
    let second_source = Digest::compute(Domain::DistInventory, b"second source revision");
    let command = |source| {
        trunk_command(
            Path::new("/workspace"),
            &outer,
            OsStr::new("/cargo-home"),
            Path::new("/stage"),
            "remapped-flags".to_owned(),
            &worktree,
            source,
        )
    };
    let environment = |command: &Command| {
        command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect::<BTreeMap<_, _>>()
    };
    let first = environment(&command(&first_source));
    let second = environment(&command(&second_source));

    for name in ["CARGO_TARGET_DIR", "CARGO_BUILD_BUILD_DIR"] {
        assert_eq!(first.get(name), second.get(name), "{name} changed on edit");
    }
    assert_ne!(
        first.get(APP_SOURCE_IDENTITY_ENV),
        second.get(APP_SOURCE_IDENTITY_ENV),
        "source identity did not change on edit"
    );

    prepare_app_build(&outer, &worktree).expect("prepare first source build");
    fs::create_dir_all(app_target_directory(&outer, &worktree))
        .expect("cargo creates the target namespace");
    prepare_app_build(&outer, &worktree).expect("prepare second source build");
    let mut children = fs::read_dir(&outer)
        .expect("read outer target")
        .map(|entry| entry.expect("read target child").file_name())
        .collect::<Vec<_>>();
    children.sort();
    assert_eq!(
        children,
        [
            app_target_directory(&outer, &worktree)
                .file_name()
                .expect("target has a name")
                .to_owned(),
            app_build_directory(&outer, &worktree)
                .file_name()
                .expect("build directory has a name")
                .to_owned(),
        ]
    );
    fs::remove_dir_all(outer).expect("remove stable-target fixture");
}

/// A shared ambient target is only an outer allocation. Each worktree gets
/// a distinct inner target even when both build the exact same sources.
#[test]
fn concurrent_worktrees_use_distinct_children_of_one_outer_target() {
    use std::sync::{Arc, Barrier};

    let outer = fixture("shared-app-target");
    let worktree_a = outer.join("worktree-a");
    let worktree_b = outer.join("worktree-b");
    fs::create_dir(&worktree_a).expect("create first worktree fixture");
    fs::create_dir(&worktree_b).expect("create second worktree fixture");
    let worktrees = [
        worktree_identity(&worktree_a).expect("identify first worktree"),
        worktree_identity(&worktree_b).expect("identify second worktree"),
    ];
    let barrier = Arc::new(Barrier::new(2));
    let workers = worktrees
        .into_iter()
        .zip([b"worktree A wasm".as_slice(), b"worktree B wasm".as_slice()])
        .map(|(worktree, bytes)| {
            let target = app_target_directory(&outer, &worktree);
            let barrier = Arc::clone(&barrier);
            let bytes = bytes.to_vec();
            std::thread::spawn(move || {
                fs::create_dir_all(&target).expect("create inner app target");
                barrier.wait();
                fs::write(target.join("app_bg.wasm"), &bytes).expect("write worktree artifact");
                (target, bytes)
            })
        })
        .collect::<Vec<_>>();
    let built = workers
        .into_iter()
        .map(|worker| worker.join().expect("worktree build joins"))
        .collect::<Vec<_>>();

    assert_ne!(
        built[0].0, built[1].0,
        "two worktrees shared an inner target"
    );
    for (target, expected) in &built {
        assert_eq!(
            fs::read(target.join("app_bg.wasm")).expect("read worktree artifact"),
            *expected,
            "one concurrent worktree consumed its peer's artifact"
        );
    }
    fs::remove_dir_all(outer).expect("remove shared-target fixture");
}

/// A closed inventory is not provenance. Foreign Wasm paired with this
/// worktree's shell and stylesheet must fail before a pointer is written.
#[test]
fn foreign_wasm_with_right_css_cannot_publish() {
    let root = fixture("foreign-wasm");
    let expected = fixture_source();
    let foreign = Digest::compute(Domain::DistInventory, b"peer worktree sources");

    let error = app_at(
        &root,
        &expected,
        &mut Vec::new(),
        |stage, _| {
            write_raw_generation(stage, "right-css", &foreign, true);
            Ok(())
        },
        || Ok(()),
    )
    .expect_err("foreign Wasm must not publish");

    assert!(
        error
            .to_string()
            .contains("complete but foreign generation"),
        "the contamination refusal did not name the foreign generation: {error}"
    );
    assert!(
        !root.join(spice::serve::APP_STAGE).exists(),
        "foreign Wasm published a current generation"
    );
    fs::remove_dir_all(root).expect("remove contamination fixture");
}

#[test]
fn finalization_produces_the_closed_worktree_stage_inventory() {
    let root = fixture("worktree-app-finalize");
    let sources = fixture_source();
    write_raw_generation(&root, "first", &sources, true);

    let identity = finalize_stage(&root, &sources).expect("generated stage finalizes");

    let mut names = fs::read_dir(&root)
        .expect("read stage")
        .map(|entry| entry.expect("read entry").file_name())
        .collect::<Vec<_>>();
    names.sort();
    assert_eq!(
        names,
        [
            "app.html",
            "app.module",
            "app_bg.wasm",
            "chrome.css",
            "composer.css",
            "mosaic.css",
            "responsive.css",
            "spice_sidebar.css",
            "style.css",
            "work.css"
        ]
        .map(std::ffi::OsString::from)
    );
    let shell = fs::read_to_string(root.join("app.html")).expect("read shell");
    assert!(
        shell.contains(&format!("data-app-generation=\"{identity}\"")),
        "the shell does not name its loaded generation: {shell}"
    );
    assert!(!shell.contains(APP_GENERATION_PLACEHOLDER));
    for asset in spice::serve::APP
        .assets()
        .iter()
        .filter(|asset| asset.path() != "/")
    {
        assert!(
            shell.contains(&format!(
                "{}/{identity}/{}",
                spice::serve::APP_GENERATION_ROUTE,
                asset.file()
            )),
            "the shell did not pin {} to {identity}: {shell}",
            asset.file()
        );
    }
    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn a_current_generation_from_the_same_sources_stands_in_for_the_compiler() {
    let root = fixture("worktree-app-current");
    assert_eq!(current_generation_of(&root, &fixture_source()), None);
    publish_fixture(&root, "first");
    let current = spice::serve::AppSource::resolve(&root)
        .expect("published generation resolves")
        .current_generation()
        .expect("published generation is current");
    assert_eq!(
        current_generation_of(&root, &fixture_source()).as_ref(),
        Some(current.identity())
    );
    let edited = Digest::compute(Domain::DistInventory, b"edited app sources");
    assert_eq!(current_generation_of(&root, &edited), None);
    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn publication_retains_only_the_newest_generations() {
    let root = fixture("worktree-app-retention");
    let published = RETAINED_GENERATIONS + 3;
    for index in 0..published {
        publish_fixture(&root, &format!("generation {index}"));
    }
    let stage = root.join(spice::serve::APP_STAGE);
    let current = spice::serve::AppSource::resolve(&root)
        .expect("last generation resolves")
        .current_generation()
        .expect("last generation is current");
    let store = stage.join(spice::serve::APP_STAGE_GENERATIONS);
    let retained = fs::read_dir(&store)
        .expect("generation store lists")
        .map(|entry| entry.expect("generation entry").path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();
    assert_eq!(retained.len(), RETAINED_GENERATIONS + 1);
    assert_eq!(
        fs::read_dir(&store)
            .expect("generation store lists")
            .count(),
        2 * retained.len(),
        "every retained generation keeps exactly its archive beside it"
    );
    for generation in &retained {
        let identity = generation
            .file_name()
            .and_then(|name| name.to_str())
            .expect("generation names are UTF-8")
            .parse::<Digest>()
            .expect("generation names are digests");
        let archive = fs::read(archive_path(&store, &identity)).expect("archive is staged");
        Rack::admit_generation(&archive, &identity).expect("archive admits as its generation");
    }
    assert!(
        retained
            .iter()
            .any(|path| path.ends_with(current.identity().as_str()))
    );
    fs::remove_dir_all(root).expect("remove fixture");
}

/// Release assembly and a development refresh both link what
/// `StagedCanvas::read` assembles; the same staged generation and tracked
/// sources, written in another order, must admit as the same rack.
#[test]
fn the_same_generation_and_tracked_sources_assemble_one_rack() {
    let roots = [fixture("rack-first"), fixture("rack-second")];
    let sources: [(&str, &[u8]); 4] = [
        ("spice/agent/SKILL.md", b"# spice\n"),
        ("docs/README.md", b"# Docs\n"),
        ("docs/planes/distribution.md", b"# Distribution\n"),
        ("docs/product/rust.md", b"# Rust\n"),
    ];
    for (index, root) in roots.iter().enumerate() {
        publish_fixture(root, "first");
        let mut ordered = sources.to_vec();
        if index == 1 {
            ordered.reverse();
        }
        for (path, bytes) in ordered {
            let path = root.join(path);
            fs::create_dir_all(path.parent().expect("source has a parent"))
                .expect("create source directory");
            fs::write(path, bytes).expect("write tracked source");
        }
    }
    let [first, second] = roots
        .each_ref()
        .map(|root| StagedCanvas::read(root).expect("rack assembles"));
    assert_eq!(first.bytes, second.bytes);
    assert_eq!(first.canvas(), second.canvas());

    let rack = Rack::admit_generation(&first.bytes, first.canvas().generation())
        .expect("assembled rack admits as its generation");
    for (namespace, member, source) in [
        (Namespace::Skill, "SKILL.md", "spice/agent/SKILL.md"),
        (
            Namespace::Doc,
            "planes/distribution.md",
            "docs/planes/distribution.md",
        ),
    ] {
        let path = namespace.member(member);
        let inflated = rack
            .member(&path)
            .expect("tracked source is a member")
            .inflate(&path)
            .expect("member inflates");
        let expected = sources
            .iter()
            .find(|(candidate, _)| *candidate == source)
            .expect("source is listed")
            .1;
        assert_eq!(&*inflated, expected, "{path}");
    }
    assert_eq!(
        first.tree(),
        rack.namespace(Namespace::App)
            .map(|(_, member)| u64::from(member.size()))
            .sum::<u64>()
    );
    for root in roots {
        fs::remove_dir_all(root).expect("remove fixture");
    }
}

#[test]
fn publication_switches_only_after_an_event_releases_the_complete_generation() {
    let root = fixture("worktree-app-event");
    publish_fixture(&root, "first");
    let source = spice::serve::AppSource::resolve(&root).expect("first generation resolves");
    let first = source
        .current_generation()
        .expect("first generation remains current");
    let module = spice::serve::APP
        .asset("/app.module")
        .expect("module belongs to the closed inventory");
    let first_route = first.route(module);
    let (started_tx, started_rx) = mpsc::sync_channel(0);
    let (release_tx, release_rx) = mpsc::sync_channel(0);

    std::thread::scope(|scope| {
        let build_root = root.clone();
        let sources = fixture_source();
        let build = scope.spawn(move || {
            app_at(
                &build_root,
                &sources,
                &mut Vec::new(),
                |stage, _| {
                    started_tx.send(()).expect("announce build barrier");
                    release_rx.recv().expect("release build barrier");
                    write_raw_generation(stage, "second", &sources, true);
                    Ok(())
                },
                || Ok(()),
            )
        });

        started_rx.recv().expect("build reaches its event barrier");
        assert_eq!(
            source
                .served(&first_route)
                .expect("first generation remains readable")
                .expect("first route remains admitted")
                .into_bytes(),
            b"module first"
        );
        assert_eq!(
            source
                .current_generation()
                .expect("the pointer remains on the first generation")
                .identity(),
            first.identity()
        );
        release_tx.send(()).expect("release the complete build");
        build
            .join()
            .expect("build thread joins")
            .expect("second generation publishes");
    });

    let second = source
        .current_generation()
        .expect("second generation becomes current");
    assert_ne!(second.identity(), first.identity());
    assert_eq!(
        source
            .served(&first_route)
            .expect("first page generation remains readable")
            .expect("first page route remains admitted")
            .into_bytes(),
        b"module first",
        "the active pointer changed an in-flight page's generation-qualified route"
    );
    let shell = source
        .served("/")
        .expect("new shell is readable")
        .expect("root route remains admitted")
        .into_bytes();
    assert!(
        String::from_utf8(shell)
            .expect("fixture shell is UTF-8")
            .contains(second.identity().as_str()),
        "the new root did not pin its dependent routes to the new generation"
    );
    fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
fn a_failed_finalization_preserves_the_prior_pointer_or_first_build_absence() {
    let root = fixture("worktree-app-failure");
    publish_fixture(&root, "first");
    let source = spice::serve::AppSource::resolve(&root).expect("first generation resolves");
    let first = source
        .current_generation()
        .expect("first generation is current")
        .identity()
        .clone();
    let sources = fixture_source();

    let error = app_at(
        &root,
        &sources,
        &mut Vec::new(),
        |stage, _| {
            write_raw_generation(stage, "broken", &sources, false);
            Ok(())
        },
        || Ok(()),
    )
    .expect_err("invalid finalization refuses");
    assert!(error.to_string().contains("app_bg.wasm"));
    assert_eq!(
        source
            .current_generation()
            .expect("failed rebuild preserves current")
            .identity(),
        &first
    );

    let fresh = fixture("worktree-app-first-failure");
    app_at(
        &fresh,
        &sources,
        &mut Vec::new(),
        |stage, _| {
            write_raw_generation(stage, "broken", &sources, false);
            Ok(())
        },
        || Ok(()),
    )
    .expect_err("first invalid finalization refuses");
    assert!(
        !fresh.join(spice::serve::APP_STAGE).exists(),
        "a failed first build published a visible stage"
    );
    fs::remove_dir_all(root).expect("remove fixture");
    fs::remove_dir_all(fresh).expect("remove fresh fixture");
}

#[test]
fn a_failed_build_preserves_the_prior_pointer_or_first_build_absence() {
    let root = fixture("worktree-app-build-failure");
    publish_fixture(&root, "first");
    let source = spice::serve::AppSource::resolve(&root).expect("first generation resolves");
    let first = source
        .current_generation()
        .expect("first generation is current")
        .identity()
        .clone();
    let sources = fixture_source();

    let error = app_at(
        &root,
        &sources,
        &mut Vec::new(),
        |stage, _| fail_partial_build(stage),
        || Ok(()),
    )
    .expect_err("failed rebuild refuses");
    assert!(matches!(error, AppBuildError::Exit(Some(42))));
    assert_eq!(
        source
            .current_generation()
            .expect("failed rebuild preserves current")
            .identity(),
        &first
    );

    let fresh = fixture("worktree-app-first-build-failure");
    let error = app_at(
        &fresh,
        &sources,
        &mut Vec::new(),
        |stage, _| fail_partial_build(stage),
        || Ok(()),
    )
    .expect_err("failed first build refuses");
    assert!(matches!(error, AppBuildError::Exit(Some(42))));
    assert!(
        !fresh.join(spice::serve::APP_STAGE).exists(),
        "a failed first build published a visible stage"
    );
    assert!(
        fs::read_dir(fresh.join("dist"))
            .expect("read build parent")
            .next()
            .is_none(),
        "a failed first build left a partial temporary"
    );
    fs::remove_dir_all(root).expect("remove fixture");
    fs::remove_dir_all(fresh).expect("remove fresh fixture");
}
