use std::ffi::{OsStr, OsString};
use std::fs;
use std::path::PathBuf;

use super::*;
use crate::toolchain::RimCapture;
use crate::toolchain::scratch::exclusive_scratch_root;

/// One rim directory whose `bin` search-path entry holds exactly `programs`.
struct Rim {
    root: PathBuf,
}

impl Rim {
    fn with(programs: &[&str]) -> Self {
        let root = exclusive_scratch_root("kill-rim");
        fs::create_dir_all(root.join("bin")).expect("create fixture search path");
        for program in programs {
            fs::write(Self::program(&root, program), b"").expect("write fixture program");
        }
        Self { root }
    }

    fn program(root: &Path, program: &str) -> PathBuf {
        root.join("bin")
            .join(format!("{program}{}", std::env::consts::EXE_SUFFIX))
    }

    fn capture(&self) -> RimCapture {
        RimCapture::new(
            self.root.clone(),
            Some(OsString::from("bin")),
            Vec::<String>::new(),
        )
        .expect("the fixture rim is absolute")
    }
}

impl Drop for Rim {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.root).expect("remove kill rim fixture");
    }
}

fn declared() -> Vec<&'static str> {
    [CARGO_PROGRAM, MUTANTS_PROGRAM]
        .into_iter()
        .chain(NATIVE_BUILD_PROGRAMS)
        .collect()
}

fn entry<'a>(tool: &'a MutationTool, name: &str) -> Option<&'a OsStr> {
    tool.base_environment()
        .entries()
        .iter()
        .find(|(candidate, _)| candidate == OsStr::new(name))
        .map(|(_, value)| value.as_os_str())
}

#[test]
fn resolution_binds_absolute_host_programs_on_a_hermetic_search_path() {
    let rim = Rim::with(&declared());
    let inherited = [
        ("HOME", "/home/fixture"),
        ("PATH", "/ambient/bin"),
        (CARGO_TARGET_DIRECTORY_ENVIRONMENT, "/shared/target"),
    ]
    .map(|(name, value)| (OsString::from(name), OsString::from(value)));

    let tool = MutationTool::resolve(PINNED_CARGO_MUTANTS, &rim.capture(), &inherited)
        .expect("every declared program resolves");

    assert_eq!(tool.cargo_program(), Rim::program(&rim.root, CARGO_PROGRAM));
    assert_eq!(
        tool.mutants_program(),
        Rim::program(&rim.root, MUTANTS_PROGRAM)
    );
    assert_eq!(
        entry(&tool, SEARCH_PATH_ENVIRONMENT),
        Some(rim.root.join("bin").as_os_str())
    );
    assert_eq!(entry(&tool, "HOME"), Some(OsStr::new("/home/fixture")));
    assert_eq!(entry(&tool, CARGO_TARGET_DIRECTORY_ENVIRONMENT), None);
}

#[test]
fn an_unresolved_program_refuses_the_kill_arm_by_name() {
    for missing in declared() {
        let present = declared()
            .into_iter()
            .filter(|program| *program != missing)
            .collect::<Vec<_>>();
        let rim = Rim::with(&present);

        let error = MutationTool::resolve(PINNED_CARGO_MUTANTS, &rim.capture(), &[])
            .expect_err("a missing declared program refuses resolution");

        assert!(
            matches!(error, KillError::ToolMissing { program } if program == missing),
            "{missing}: {error}"
        );
    }
}
