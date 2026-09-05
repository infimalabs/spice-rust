use std::ffi::{OsStr, OsString};
use std::fs;
use std::os::unix::fs::PermissionsExt as _;
use std::sync::atomic::{AtomicU64, Ordering};

use super::cargo_authority;

#[test]
fn cargo_authority_binds_absolute_tools_before_planning() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = std::env::temp_dir().join(format!(
        "spice-prove-cargo-authority-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed),
    ));
    let bootstrap = root.join("bootstrap");
    let private_home = root.join("private-home");
    let tools = root.join("tools");
    fs::create_dir_all(&bootstrap).expect("create bootstrap Cargo home");
    fs::create_dir(&private_home).expect("create private Cargo home");
    fs::create_dir(&tools).expect("create native tool directory");
    let cargo = tools.join("cargo");
    let rustc = tools.join("rustc");
    let rustdoc = tools.join("rustdoc");
    for tool in [&cargo, &rustc, &rustdoc] {
        fs::write(tool, "fixture\n").expect("write native tool fixture");
        fs::set_permissions(tool, fs::Permissions::from_mode(0o700))
            .expect("make native tool fixture executable");
    }
    let environment = vec![
        (
            OsString::from(spice::cargo_isolation::BOOTSTRAP_HOME_VARIABLE),
            bootstrap.into_os_string(),
        ),
        (
            OsString::from(spice::cargo_isolation::CARGO_HOME_VARIABLE),
            private_home.clone().into_os_string(),
        ),
        (
            OsString::from(spice::cargo_isolation::NATIVE_CARGO_VARIABLE),
            cargo.clone().into_os_string(),
        ),
        (
            OsString::from(spice::cargo_isolation::NATIVE_RUSTC_VARIABLE),
            rustc.into_os_string(),
        ),
        (
            OsString::from(spice::cargo_isolation::NATIVE_RUSTDOC_VARIABLE),
            rustdoc.into_os_string(),
        ),
        (OsString::from("PATH"), OsString::new()),
    ];

    let Ok(authority) = cargo_authority(&root, &environment) else {
        panic!("resolve Cargo authority");
    };
    assert_eq!(authority.cargo, fs::canonicalize(&cargo).unwrap());
    let entries = authority
        .environment
        .iter()
        .cloned()
        .collect::<std::collections::BTreeMap<_, _>>();
    assert_eq!(
        entries[OsStr::new(spice::cargo_isolation::CARGO_HOME_VARIABLE)],
        private_home
    );
    assert_eq!(
        entries[OsStr::new(spice::cargo_isolation::NATIVE_CARGO_VARIABLE)],
        authority.cargo
    );
    assert_eq!(entries[OsStr::new("PATH")], OsString::new());

    fs::remove_file(&cargo).expect("remove admitted Cargo fixture");
    let error = cargo_authority(&root, &environment)
        .expect_err("stale Cargo authority must refuse before planning");
    assert_eq!(
        error.reason(),
        "cargo-isolation::native-toolchain-unavailable"
    );
    fs::remove_dir_all(&root).expect("remove Cargo authority fixture");
}
