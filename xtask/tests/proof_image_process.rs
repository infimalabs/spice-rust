#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

const PROCESS_PATH: &str = concat!("PA", "TH");
const IDENTITY: &str = "sha256:3e8a97ea5628fad302eafb4e93f81da8236c6bf164fd3459509d6db71ee57334";

#[test]
fn proof_image_build_returns_the_buildx_exit_and_diagnoses_an_unexpected_eof() {
    let fixture = Fixture::new();
    let programs = fixture.root.join("programs");
    fs::create_dir(&programs).expect("create fixture program directory");
    executable(&programs.join("cargo"), "#!/bin/sh\nexit 0\n");
    executable(
        &programs.join("docker"),
        concat!(
            "#!/bin/sh\n",
            "if [ \"$1\" = buildx ] && [ \"$2\" = build ]; then\n",
            "  printf 'failed to solve: unexpected EOF\\n' >&2\n",
            "  exit 73\n",
            "fi\n",
            "printf 'unexpected docker fixture call: %s\\n' \"$*\" >&2\n",
            "exit 97\n",
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["proof-image", "build"])
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, &programs)
        .output()
        .expect("launch proof-image xtask");

    assert_eq!(output.status.code(), Some(73), "{}", streams(&output));
    let diagnostic = String::from_utf8(output.stderr).expect("diagnostic is UTF-8");
    for text in [
        "unexpected EOF",
        "no compiler refused",
        "global prune",
        "every lane",
    ] {
        assert!(diagnostic.contains(text), "missing {text:?}: {diagnostic}");
    }
    let context = fs::canonicalize(&fixture.repository)
        .expect("canonicalize fixture repository")
        .join("target/proof-image/context");
    assert!(
        diagnostic.trim_end().ends_with(&format!(
            "Clean up only this lane's disposable build context, {}, and rerun",
            context.display()
        )),
        "unsafe cleanup diagnostic: {diagnostic}"
    );
    assert_eq!(
        fs::read_to_string(fixture.repository.join("target/proof-image/build.status")).unwrap(),
        "73\n"
    );
}

#[test]
fn proof_image_verify_exercises_the_rust_build_and_load_paths_in_order() {
    let fixture = Fixture::new();
    let programs = fixture.root.join("programs");
    fs::create_dir(&programs).expect("create fixture program directory");
    executable(&programs.join("cargo"), "#!/bin/sh\nexit 0\n");
    let log = fixture.root.join("docker.log");
    executable(
        &programs.join("docker"),
        &format!(
            concat!(
                "#!/bin/sh\n",
                "printf '%s\\n' \"$*\" >> '{}'\n",
                "case \"$1\" in\n",
                "  buildx|run|load) exit 0 ;;\n",
                "  image) printf '{}\\n'; exit 0 ;;\n",
                "  save) printf abc > \"$3\"; exit 0 ;;\n",
                "esac\n",
                "exit 97\n",
            ),
            log.display(),
            IDENTITY,
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["proof-image", "verify"])
        .current_dir(&fixture.repository)
        .env(PROCESS_PATH, &programs)
        .output()
        .expect("launch proof-image verification");

    assert!(output.status.success(), "{}", streams(&output));
    let calls = fs::read_to_string(log).expect("read Docker call log");
    let calls = calls.lines().collect::<Vec<_>>();
    assert_eq!(calls.len(), 6, "{calls:?}");
    assert!(calls[0].starts_with("buildx build --load --platform linux/amd64"));
    assert!(calls[1].starts_with("image inspect --format {{.Id}}"));
    assert!(calls[2].starts_with("run --rm --network=none --platform linux/amd64"));
    assert!(calls[3].starts_with("save --output "));
    assert!(calls[4].starts_with("load --input "));
    assert!(calls[5].starts_with("image inspect --format {{.Id}}"));
    assert_eq!(
        calls.len(),
        6,
        "no shell-script action may trail the load check"
    );
}

struct Fixture {
    root: PathBuf,
    repository: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "spice-proof-image-process-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let repository = root.join("repository");
        let source = repository.join("dist/proof-image");
        fs::create_dir_all(&source).expect("create fixture proof-image source");
        for name in ["Dockerfile", "cargo-config.toml", "cargo-wrapper.sh"] {
            fs::write(source.join(name), format!("fixture {name}\n")).unwrap();
        }
        fs::write(source.join("identity.txt"), format!("{IDENTITY}\n")).unwrap();
        git(&repository, &["init", "-q"]);
        Self { root, repository }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove proof-image process fixture");
    }
}

fn executable(path: &Path, text: &str) {
    fs::write(path, text).expect("write fixture executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make fixture executable");
}

fn git(root: &Path, arguments: &[&str]) {
    let output = Command::new("/usr/bin/git")
        .args(arguments)
        .current_dir(root)
        .output()
        .expect("fixture Git launches");
    assert!(
        output.status.success(),
        "git {arguments:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
}

fn streams(output: &std::process::Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
