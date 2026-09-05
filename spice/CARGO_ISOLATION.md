# Cargo isolation

Spice resident lanes receive a private `CARGO_HOME` over one immutable,
content-addressed directory-source store. Spice captures the bootstrap Cargo
home from `SPICE_RS_BOOTSTRAP_CARGO_HOME`, then `CARGO_HOME`, then
`HOME/.cargo`, in that precedence order. The store is rooted at the captured
home's `spice/source-store-v2` and is keyed by the exact `Cargo.lock` bytes
plus schema version. Descendant Cargo processes receive that captured home as
`SPICE_RS_BOOTSTRAP_CARGO_HOME` while their private home is exported as
`CARGO_HOME`. Cargo, rustc, and rustdoc are admitted as one exact native
toolchain before `CARGO_HOME` changes. A complete explicit
`SPICE_RS_NATIVE_CARGO`/`SPICE_RS_NATIVE_RUSTC`/`SPICE_RS_NATIVE_RUSTDOC`
trio takes precedence; otherwise Spice resolves all three through the channel
declared by the worktree's `rust-toolchain.toml` and the exact `rustup` below
the bootstrap Cargo home. Partial declarations, missing executables, and paths
from different toolchain directories are refused before Cargo starts. `PATH`
is not a compiler-authority resolver.

Task, session, and steering authority belongs to the active harness. Native
harness authority remains in force unless the invocation explicitly entered
through an installed Spice bootstrap. Development surfaces such as `dev cargo`
and `dev xtask` use only the exact Git-common Rust appliance; they do not
establish or imply harness authority. In a supervised repository shell,
tracked wrapper policy resolves ambient `spice` to that appliance:

```sh
spice dev cargo -- test --locked -p spice
spice dev xtask -- verify --package spice
```

Replace the trailing example argument vector with the exact Cargo or xtask
operation required; retain the ambient appliance command and `dev ... --`
boundary.

A missing, symbolic, non-regular, or non-executable appliance is a hard stop at
wrapper resolution. Do not select another Cargo route or point `CARGO_HOME` at
a shared mutable cache to bypass that refusal.

Package directory entries are sealed after their atomic publication. Regular
source files retain owner-write permission because supported Cargo build
scripts may preserve source permissions when copying generated inputs into
their mutable `OUT_DIR`; making those inputs read-only poisons later rebuilds.
Cargo's directory-source checksums reject changed package bytes during builds,
and Spice inspection independently verifies every package tree digest and
reports any permission or content drift without repairing it.

Use `spice dev cargo -- CARGO_ARGS...` whenever a command may create
or refresh `Cargo.lock`, or when a non-resident command needs the supported
isolated boundary. Use `spice dev xtask -- XTASK_ARGS...` for the
repository's build, verification, staging, cache, and refresh orchestrator; it
prepares the same private home before expanding to the locked `xtask` package.
Use `spice dev cargo-store` to inspect the owned schema
and `spice dev cargo-store --cleanup` to remove only recognized expired
temporary state and valid unreferenced generations/packages. Cleanup has a
seven-day grace, retains all generations if any home has uncertain liveness,
and reports malformed links, records, modes, file types, or digests rather than
repairing or deleting them.

Each Rust `dev cargo` process holds a unique kernel-backed reader pin from
before store preparation unlocks until its native Cargo child exits. Cleanup
retains every actively pinned generation, removes unlocked records left by a
crashed wrapper, and retains all generations if a pin cannot be classified.
Resident occupants need no transient pin: their lane-specific home remains the
current reference for the one process group that can occupy that lane, and a
replacement cannot advance the same home until the seat is vacant.
