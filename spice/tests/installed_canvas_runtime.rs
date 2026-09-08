#![cfg(unix)]

use std::fs;
use std::io::{Read as _, Write as _};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::de::DeserializeOwned;
use spice::install::{InstallPreview, InstallReceipt, InstallStatus};
use spice::repository::Repository;
use wire::Outcome;

#[path = "support/fixture_root.rs"]
mod fixture_root;
#[path = "support/git_repository.rs"]
mod git_repository;
#[path = "support/install_package.rs"]
mod install_package_fixture;
#[path = "support/readiness.rs"]
mod readiness;

use readiness::Readiness;

const AUTH_TOKEN: &str = "installed-canvas-test-token";
const REFUSAL_EXIT: i32 = 2;
static NEXT: AtomicU64 = AtomicU64::new(0);

struct Fixture {
    root: PathBuf,
    main: PathBuf,
    linked: PathBuf,
    package: install_package_fixture::PackageFixture,
}

impl Fixture {
    fn new() -> Self {
        let root = fixture_root::claim(
            &std::env::temp_dir(),
            &format!(
                "installed-canvas-runtime-{}",
                NEXT.fetch_add(1, Ordering::Relaxed)
            ),
        );
        let main = root.join("main");
        let linked = root.join("linked");
        fs::create_dir(&main).expect("create main worktree");
        git_repository::link(&main, &linked, "linked");
        fs::write(main.join("spice-rust.toml"), "# non-Rust repository\n")
            .expect("write main repository policy");
        fs::write(
            linked.join("spice-rust.toml"),
            "# non-Rust linked worktree\n",
        )
        .expect("write linked repository policy");
        let package = install_package_fixture::PackageFixture::create(
            root.join("release-package"),
            Path::new(env!("CARGO_BIN_EXE_spice")),
        );
        Self {
            root,
            main,
            linked,
            package,
        }
    }

    fn installed_binary(&self) -> PathBuf {
        self.main.join(".git/.spice-install/bin/spice")
    }

    fn app_root(&self) -> PathBuf {
        self.main.join(".git/.spice-install/app")
    }

    fn install(&self) -> InstallReceipt {
        let package = self
            .package
            .root()
            .to_str()
            .expect("package fixture path is UTF-8");
        let preview: InstallPreview = reply_body(invoke(
            Path::new(env!("CARGO_BIN_EXE_spice")),
            &self.linked,
            &["--json", "install", "--package", package],
        ));
        reply_body(invoke(
            Path::new(env!("CARGO_BIN_EXE_spice")),
            &self.linked,
            &[
                "--json",
                "install",
                "--package",
                package,
                &format!("--apply={}", preview.digest()),
            ],
        ))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove installed Canvas fixture");
    }
}

struct Resident {
    child: Option<Child>,
    executable: PathBuf,
    worktree: PathBuf,
}

impl Resident {
    fn start(executable: &Path, worktree: &Path) -> (Self, canvas::Discovery) {
        let repository = Repository::discover(worktree).expect("resident repository resolves");
        let discovery_path = repository
            .admit_resident_client()
            .expect("resident discovery path is admitted");
        let child = Command::new(executable)
            .args([
                "up",
                "--foreground",
                "--port",
                "0",
                "--auth-token",
                AUTH_TOKEN,
            ])
            .current_dir(worktree)
            .env_remove(spice::agent_launch::SPICE_RS_LANE)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("installed resident launches");
        let mut resident = Self {
            child: Some(child),
            executable: executable.to_path_buf(),
            worktree: worktree.to_path_buf(),
        };
        let watched = discovery_path
            .parent()
            .expect("resident discovery has a parent");
        fs::create_dir_all(watched).expect("resident runtime directory exists");
        match readiness::wait_until_ready(
            resident.child.as_mut().expect("resident child"),
            watched,
            readiness::ARRIVAL_HORIZON,
            "installed Canvas resident",
            || discovery_path.is_file(),
        ) {
            Readiness::Ready => {}
            Readiness::Exited => {
                let output = resident
                    .child
                    .take()
                    .expect("exited resident child")
                    .wait_with_output()
                    .expect("collect exited resident");
                panic!("installed resident exited early: {}", spoken(&output));
            }
        }
        let discovery =
            wire::decode(&fs::read(&discovery_path).expect("installed resident discovery reads"))
                .expect("installed resident discovery decodes");
        (resident, discovery)
    }

    fn stop(mut self) {
        let down = invoke(&self.executable, &self.worktree, &["--json", "down"]);
        assert!(
            down.status.success(),
            "resident down failed: {}",
            spoken(&down)
        );
        let output = self
            .child
            .take()
            .expect("resident child")
            .wait_with_output()
            .expect("installed resident exits");
        assert!(
            output.status.success(),
            "installed resident failed: {}",
            spoken(&output)
        );
    }
}

impl Drop for Resident {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[test]
fn installed_generation_serves_every_linked_worktree_and_refuses_drift() {
    let fixture = Fixture::new();
    assert_refused(
        invoke(
            Path::new(env!("CARGO_BIN_EXE_spice")),
            &fixture.main,
            &["--json", "up", "--foreground", "--port", "0"],
        ),
        "canvas::installed-app-refused",
    );

    let receipt = fixture.install();
    assert_eq!(receipt.status(), InstallStatus::Complete);
    let installed = fixture.installed_binary();
    let app = fixture.app_root();
    let generation = fs::read_to_string(app.join("current"))
        .expect("installed generation pointer reads")
        .trim()
        .to_owned();
    fs::remove_dir_all(fixture.main.join(canvas::APP_STAGE))
        .expect("remove development stage before production startup");

    let main_state = Repository::discover(&fixture.main)
        .expect("main repository resolves")
        .admit_state()
        .expect("main runtime state is admitted");
    let linked_state = Repository::discover(&fixture.linked)
        .expect("linked repository resolves")
        .admit_state()
        .expect("linked runtime state is admitted");
    assert_ne!(
        main_state.worktree_runtime(),
        linked_state.worktree_runtime()
    );

    let (main, main_discovery) = Resident::start(&installed, &fixture.main);
    assert_generation_and_assets(&fixture, &main_discovery, &generation);
    main.stop();
    let (linked, linked_discovery) = Resident::start(&installed, &fixture.linked);
    assert_generation_and_assets(&fixture, &linked_discovery, &generation);
    linked.stop();

    let style = app
        .join("generations")
        .join(&generation)
        .join(wire::manifest::CANVAS_STYLE);
    let style_bytes = fs::read(&style).expect("installed style reads");
    fs::remove_file(&style).expect("remove one installed app member");
    assert_refused(
        installed_up(&installed, &fixture.main),
        "installed path drifted",
    );
    fs::write(&style, &style_bytes).expect("restore installed style");

    let pointer = app.join("current");
    let pointer_bytes = fs::read(&pointer).expect("installed pointer reads");
    fs::write(&pointer, format!("{}\n", "f".repeat(64))).expect("drift installed pointer");
    assert_refused(
        installed_up(&installed, &fixture.main),
        "installed path drifted",
    );
    fs::write(&pointer, pointer_bytes).expect("restore installed pointer");

    let mismatched = fixture.root.join("mismatched-spice");
    fs::write(&mismatched, b"another executable generation\n")
        .expect("write mismatched executable fixture");
    let repository = Repository::discover(&fixture.main).expect("main repository resolves");
    assert!(matches!(
        spice::install::installed_canvas_source(&repository, &mismatched),
        Err(spice::install::InstallError::BinaryContextChanged)
    ));

    let removed: InstallReceipt = reply_body(invoke(
        &installed,
        &fixture.linked,
        &["--json", "install", "--unapply"],
    ));
    assert_eq!(removed.status(), InstallStatus::Uninstalled);
    assert!(!installed.exists());
    assert!(!app.exists());
}

fn installed_up(executable: &Path, worktree: &Path) -> Output {
    invoke(
        executable,
        worktree,
        &["--json", "up", "--foreground", "--port", "0"],
    )
}

fn assert_generation_and_assets(
    fixture: &Fixture,
    discovery: &canvas::Discovery,
    generation: &str,
) {
    let bootstrap = get(
        &discovery.endpoint,
        &format!("/bootstrap?auth={AUTH_TOKEN}"),
    );
    assert!(bootstrap.starts_with(b"HTTP/1.1 200 OK\r\n"));
    assert_eq!(
        header(&bootstrap, wire::hello::CANVAS_GENERATION_HEADER),
        Some(generation)
    );
    let _: wire::CanvasBootstrap =
        wire::decode(body(&bootstrap)).expect("authenticated bootstrap decodes");
    for asset in canvas::APP.assets() {
        let route = if asset.path() == "/" {
            "/".to_owned()
        } else {
            format!(
                "{}/{generation}/{}",
                canvas::APP_GENERATION_ROUTE,
                asset.file()
            )
        };
        let response = get(&discovery.endpoint, &route);
        assert!(
            response.starts_with(b"HTTP/1.1 200 OK\r\n"),
            "installed route {route} did not answer successfully"
        );
        assert_eq!(body(&response), fixture.package.app_bytes(asset.file()));
    }
}

fn get(endpoint: &str, route: &str) -> Vec<u8> {
    let authority = endpoint
        .strip_prefix("http://")
        .expect("fixture endpoint uses HTTP");
    let mut stream = TcpStream::connect(authority).expect("Canvas HTTP connection opens");
    write!(
        stream,
        "GET {route} HTTP/1.1\r\nHost: {authority}\r\nConnection: close\r\n\r\n"
    )
    .expect("Canvas HTTP request writes");
    let mut response = Vec::new();
    stream
        .read_to_end(&mut response)
        .expect("Canvas HTTP response reads");
    response
}

fn header<'a>(response: &'a [u8], name: &str) -> Option<&'a str> {
    let end = response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")?;
    std::str::from_utf8(&response[..end])
        .ok()?
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find_map(|(candidate, value)| candidate.eq_ignore_ascii_case(name).then_some(value.trim()))
}

fn body(response: &[u8]) -> &[u8] {
    response
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .map(|offset| &response[offset + 4..])
        .expect("HTTP response carries a header boundary")
}

fn invoke(executable: &Path, worktree: &Path, arguments: &[&str]) -> Output {
    Command::new(executable)
        .args(arguments)
        .current_dir(worktree)
        .env_remove(spice::agent_launch::SPICE_RS_LANE)
        .output()
        .expect("Spice process launches")
}

fn reply_body<B: DeserializeOwned>(output: Output) -> B {
    assert!(
        output.status.success(),
        "command failed: {}",
        spoken(&output)
    );
    let Outcome::Reply(reply) =
        wire::decode::<Outcome<B>>(&output.stdout).expect("command outcome decodes")
    else {
        panic!("command did not reply: {}", spoken(&output));
    };
    reply.body
}

fn assert_refused(output: Output, detail: &str) {
    assert_eq!(
        output.status.code(),
        Some(REFUSAL_EXIT),
        "{}",
        spoken(&output)
    );
    let rendered = spoken(&output);
    assert!(
        rendered.contains("canvas::installed-app-refused"),
        "{rendered}"
    );
    assert!(rendered.contains(detail), "{rendered}");
}

fn spoken(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}
