# Cargo isolation

Resident lanes receive a private `CARGO_HOME` over one immutable,
content-addressed directory-source store keyed by the exact `Cargo.lock` bytes.
`spice dev cargo -- CARGO_ARGS...` and `spice dev xtask -- XTASK_ARGS...` are the
only supported Cargo routes; [AGENTS.md](../AGENTS.md#cargo-isolation) states
when to use them.

## Homes and toolchain

The bootstrap Cargo home is `SPICE_BOOTSTRAP_CARGO_HOME`, else `CARGO_HOME`,
else `HOME/.cargo`, and descendants inherit it as `SPICE_BOOTSTRAP_CARGO_HOME`
while their private home is `CARGO_HOME`. Cargo, rustc, and rustdoc are
admitted as one native toolchain before `CARGO_HOME` changes: either a complete
`SPICE_NATIVE_CARGO`, `SPICE_NATIVE_RUSTC`, `SPICE_NATIVE_RUSTDOC` trio, or the
channel `rust-toolchain.toml` declares, resolved by the bootstrap home's own
`rustup`. A partial trio, a missing executable, or tools from different
toolchain directories refuse before Cargo starts; `PATH` never selects the
compiler.

Package trees are sealed after atomic publication, except that regular source
files keep owner-write permission: build scripts that copy inputs into
`OUT_DIR` preserve permissions, and read-only inputs would poison later
rebuilds. Cargo's directory-source checksums reject changed bytes, and
`spice dev cargo-store` verifies every package digest and reports drift without
repairing it. `spice dev cargo-store --cleanup` removes only recognized expired
temporary state and unreferenced generations, and retains everything when any
home's liveness is uncertain.

Each `dev cargo` process holds a kernel-backed reader pin on its generation
until its Cargo child exits, and cleanup never removes a pinned generation. A
resident lane's home needs no pin: only one process group can occupy the lane,
and a replacement cannot advance that home until the seat is vacant. Proof
planning binds the staged lock bytes to an already-admitted home and never
advances a lane home or seeds dependencies; unadmitted generations refuse.

## Output

Both surfaces forward the argument vector, environment, and exit status
exactly. Cargo's diagnostics are captured whole in
`<git-common>/.spice-rs/cargo-output/DIGEST.log`, with a periodic heartbeat
naming Cargo's current step. Success prints one summary line with the capture
path; failure adds a bounded tail and an inspection command. Output that is the
command's product (`dev xtask` tasks, `cargo run`, `metadata`, `--list`, JSON
message formats, and similar) reaches standard output byte for byte and is also
retained. `--verbose` before `--` streams captured lines live. Under a
pre-commit worker, which captures its configured leaf itself, the streams stay
verbatim.

## Target budget

Each worktree's `target/` has a size bound. `dev cargo`, `dev xtask`,
`spice gate prove`, and the pre-commit gate share the worktree's Cargo-output
lease for their whole run; the first to find it free trims least recently used
profile directories until `target/` fits, leaving whole any profile whose Cargo
lock a live Cargo or rust-analyzer holds. A run that cannot take the lease
refuses before touching Cargo output. `spice dev cargo-footprint` reports usage
per profile and the bound.
