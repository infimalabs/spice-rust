//! Disposable browser checks against this worktree's staged app generation.

use std::fmt;
use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};

use signal_hook::consts::signal::{SIGHUP, SIGINT, SIGTERM};
use signal_hook::iterator::Signals;
use wire::Digest;

static NEXT_ROOT: AtomicU64 = AtomicU64::new(0);

/// Build the current worktree's native Spice and run an isolated demo using
/// exactly that worktree's staged app generation.
///
/// # Errors
///
/// Refuses an unavailable or moving staged generation, a native build or demo
/// seed failure, or a resident that cannot launch or exit successfully.
pub fn app_demo(open: bool, output: &mut impl Write) -> Result<(), AppDemoError> {
    let start =
        std::env::current_dir().map_err(|error| fail(format!("current directory: {error}")))?;
    let repository = spice::repository::Repository::discover(&start)
        .map_err(|error| fail(format!("active worktree unavailable: {error}")))?;
    app_demo_at(repository.worktree_root(), open, output)
}

fn app_demo_at(root: &Path, open: bool, output: &mut impl Write) -> Result<(), AppDemoError> {
    let expected = staged_generation(root)?;
    let executable = build_spice(root, output)?;
    let demo = DemoRoot::create()?;

    seed_demo(&executable, root, demo.path())?;
    let served = matching_generation(demo.path(), &expected)?;
    writeln!(
        output,
        "[app-demo] worktree={} staged_generation={expected}",
        root.display()
    )
    .map_err(|error| fail(format!("app-demo output failed: {error}")))?;
    writeln!(
        output,
        "[app-demo] root={} served_generation={served}",
        demo.path().display()
    )
    .map_err(|error| fail(format!("app-demo output failed: {error}")))?;
    output
        .flush()
        .map_err(|error| fail(format!("app-demo output failed: {error}")))?;

    run_demo(&executable, demo.path(), open)?;
    demo.remove()
}

fn staged_generation(root: &Path) -> Result<Digest, AppDemoError> {
    let source = spice::serve::AppSource::resolve(root)
        .map_err(|error| fail(format!("worktree staged app refused: {error}")))?;
    source
        .current_generation()
        .map(|generation| generation.identity().clone())
        .map_err(|error| fail(format!("worktree staged app refused: {error}")))
}

fn matching_generation(root: &Path, expected: &Digest) -> Result<Digest, AppDemoError> {
    let served = staged_generation(root)?;
    if &served == expected {
        Ok(served)
    } else {
        Err(fail(format!(
            "worktree staged generation moved while seeding: expected {expected}, served {served}"
        )))
    }
}

fn build_spice(root: &Path, output: &mut impl Write) -> Result<PathBuf, AppDemoError> {
    writeln!(output, "[app-demo] building current worktree spice")
        .map_err(|error| fail(format!("app-demo output failed: {error}")))?;
    output
        .flush()
        .map_err(|error| fail(format!("app-demo output failed: {error}")))?;
    let built = Command::new("cargo")
        .current_dir(root)
        .args([
            "build",
            "--locked",
            "--package",
            "spice",
            "--bin",
            "spice",
            "--message-format",
            "json-render-diagnostics",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .output()
        .map_err(|error| fail(format!("current spice build did not start: {error}")))?;
    if !built.status.success() {
        return Err(fail(format!(
            "current spice build exited with {}",
            exit_description(built.status.code())
        )));
    }
    built_executable(&String::from_utf8_lossy(&built.stdout))
        .ok_or_else(|| fail("current spice build reported no executable"))
}

fn built_executable(events: &str) -> Option<PathBuf> {
    events
        .lines()
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(line).ok())
        .filter(|event| {
            event.get("reason").and_then(serde_json::Value::as_str) == Some("compiler-artifact")
                && event
                    .pointer("/target/name")
                    .and_then(serde_json::Value::as_str)
                    == Some("spice")
        })
        .filter_map(|event| event.get("executable")?.as_str().map(PathBuf::from))
        .next_back()
}

fn seed_demo(executable: &Path, source: &Path, root: &Path) -> Result<(), AppDemoError> {
    let seeded = Command::new(executable)
        .current_dir(source)
        .arg("demo")
        .arg("--root")
        .arg(root)
        .args(["--seed-only", "--json"])
        .output()
        .map_err(|error| fail(format!("app-demo seed did not start: {error}")))?;
    if seeded.status.success() {
        return Ok(());
    }
    let detail = [seeded.stdout.as_slice(), seeded.stderr.as_slice()]
        .into_iter()
        .map(String::from_utf8_lossy)
        .map(|text| text.trim().to_owned())
        .filter(|text| !text.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Err(fail(format!(
        "app-demo seed exited with {}: {detail}",
        exit_description(seeded.status.code())
    )))
}

fn run_demo(executable: &Path, root: &Path, open: bool) -> Result<(), AppDemoError> {
    // The child owns graceful shutdown. Catch the same terminal signals in this
    // waiting parent so its disposable root is removed after the child exits.
    let _signals = Signals::new([SIGHUP, SIGINT, SIGTERM])
        .map_err(|error| fail(format!("app-demo signal handling unavailable: {error}")))?;
    let mut command = Command::new(executable);
    command
        .current_dir(root)
        .arg("demo")
        .arg("--reuse")
        .arg(root);
    if open {
        command.arg("--open");
    }
    let status = command
        .status()
        .map_err(|error| fail(format!("app-demo resident did not start: {error}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(fail(format!(
            "app-demo resident exited with {}",
            exit_description(status.code())
        )))
    }
}

fn exit_description(code: Option<i32>) -> String {
    code.map_or_else(|| "a signal".to_owned(), |code| code.to_string())
}

struct DemoRoot(PathBuf);

impl DemoRoot {
    fn create() -> Result<Self, AppDemoError> {
        loop {
            let path = std::env::temp_dir().join(format!(
                "spice-app-demo-{}-{}",
                std::process::id(),
                NEXT_ROOT.fetch_add(1, Ordering::Relaxed)
            ));
            match fs::create_dir(&path) {
                Ok(()) => return Ok(Self(path)),
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                Err(error) => return Err(fail(format!("{}: {error}", path.display()))),
            }
        }
    }

    fn path(&self) -> &Path {
        &self.0
    }

    fn remove(mut self) -> Result<(), AppDemoError> {
        fs::remove_dir_all(&self.0)
            .map_err(|error| fail(format!("app-demo cleanup {}: {error}", self.0.display())))?;
        self.0.clear();
        Ok(())
    }
}

impl Drop for DemoRoot {
    fn drop(&mut self) {
        if !self.0.as_os_str().is_empty() {
            let _ = fs::remove_dir_all(&self.0);
        }
    }
}

#[derive(Debug)]
pub struct AppDemoError(String);

fn fail(message: impl Into<String>) -> AppDemoError {
    AppDemoError(message.into())
}

impl fmt::Display for AppDemoError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for AppDemoError {}

#[cfg(test)]
mod tests {
    use super::*;

    fn stage(root: &Path, identity: &str, omitted: Option<&str>) {
        let generation = root
            .join(spice::serve::APP_STAGE)
            .join(spice::serve::APP_STAGE_GENERATIONS)
            .join(identity);
        fs::create_dir_all(&generation).unwrap();
        for asset in spice::serve::APP.assets() {
            if omitted != Some(asset.file()) {
                fs::write(generation.join(asset.file()), asset.file()).unwrap();
            }
        }
        fs::write(
            root.join(spice::serve::APP_STAGE)
                .join(spice::serve::APP_STAGE_CURRENT),
            format!("{identity}\n"),
        )
        .unwrap();
    }

    #[test]
    fn selects_only_a_complete_worktree_stage() {
        let source = DemoRoot::create().unwrap();
        let identity = "5555555555555555555555555555555555555555555555555555555555555555";
        stage(source.path(), identity, None);
        assert_eq!(staged_generation(source.path()).unwrap().as_str(), identity);

        let incomplete = DemoRoot::create().unwrap();
        stage(
            incomplete.path(),
            identity,
            Some(wire::manifest::APP_MODULE),
        );
        assert!(staged_generation(incomplete.path()).is_err());
        let missing = DemoRoot::create().unwrap();
        assert!(staged_generation(missing.path()).is_err());
    }

    #[test]
    fn refuses_a_seeded_generation_that_does_not_match_the_worktree() {
        let seeded = DemoRoot::create().unwrap();
        let expected = "7777777777777777777777777777777777777777777777777777777777777777"
            .parse()
            .unwrap();
        let served = "8888888888888888888888888888888888888888888888888888888888888888";
        stage(seeded.path(), served, None);

        let error = matching_generation(seeded.path(), &expected).unwrap_err();
        assert!(error.to_string().contains(served));
    }
}
