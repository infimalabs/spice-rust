# Sealed Darwin proof runner

`build.sh` seals the `aarch64-apple-darwin` half of the release appliance: the
`proof` crate's `spice-proof-darwin-runner` binary, built with one compiled pin
document naming the exact source commit, the vendored closure of `Cargo.lock`,
the SDK, developer, and Cargo cache directories, and the path and bytes of
every tool it will run. Seal the runner from the repository root:

```sh
dist/proof-runner/build.sh
```

The command refuses anything but native arm64 macOS, a tracked tree at `HEAD`,
and untracked files outside release output. It writes the runner, its vendored
closure, and a readable copy of the pin document under
`target/proof-native-darwin/`, then exercises the runner protocol end to end
before reporting the runner's identity.

Bind that path as `proof.runner` in machine-local configuration. Nothing else
selects a runner: `cargo xtask stage-target aarch64-apple-darwin` refuses to
stage until the configured runner attests, and proof replay admits the runner by
digesting the exact bytes it is about to execute.

Pins live inside the executable rather than beside it, so a runner built without
them refuses instead of running unpinned, and editing them produces a different
executable with a different identity. The runner re-reads its own bytes before
every execution, revalidates every pin, then emits its attestation on stderr and
runs the battery member under `sandbox-exec` with networking denied, the
repository read-only, the host's Cargo caches unreadable, and fresh empty Cargo
home and target directories that it removes afterward.
