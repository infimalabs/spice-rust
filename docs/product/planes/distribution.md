# Product model — supporting planes — Distribution

[Product model — supporting planes](../planes.md) · [Spice product model](../README.md)
## Distribution

Distribution crosses authorities the appliance does not own outright: one
tracked Git release authority and an operator's repository. Immutable release
refs cannot be repaired by pretending an earlier effect did not happen, and
repository installation must preserve state the operator already owns. Exact
plans, durable receipts, convergence, and identity-aware reversal answer those
boundaries.

### External publication is a plan, not a verb

**Invariant —** Every release verb with externally visible publication effects
renders its complete ordered plan and performs none of those effects until
execution supplies that plan's exact digest. An absent or moved digest refuses
before effects; exact-plan execution is the only publication path. Local
mutations remain governed by their own transactional contracts.

**Invariant —** Plan identity covers the repository, action, version, source and
release commits, target matrix, ordered operations, notes content, artifacts,
proof receipts, sealed manifest, and captured preconditions. Step order is part
of it, because the ordering is the safety property. The correctness operation
means the complete canonical strict battery, never a mutable subset outside the
plan; a digest over step names alone would let an input change after approval.

**Invariant —** Preconditions that can be evaluated without effect are evaluated
while the plan is built, so a knowable failure never strands the operator after
an effect.

**Invariant —** Inspection is free of consequence. Checking the gates, drafting
the notes, and previewing the range never bump, commit, tag, push, or publish.

**Invariant —** Release documents are complete only as files. Bare
`spice release notes` prints a bounded summary, and `--output FILE` writes the
complete document. A publish preview retains the complete plan under the Git
common directory before it prints, names that file, and ends with the exact
`--apply=KEY` command; that key is the only accepted authority.

**Invariant —** One executor and one plan and receipt grammar serve preview,
execution, and recovery. The executor records an operation's exact prior state
before crossing its effect boundary and a typed receipt after convergence, and
resumption admits only the same plan and receipt prefix. A parallel
verification or publication body would be a second release authority.

### A versioned commit is sealed before publication

**Invariant —** Version activation is an ordinary task landing. Both release
actions require clean HEAD at the release commit, already integrated in the
tracked source branch, with requested, compiled, and tracked product versions
in agreement; every other shape refuses. Release never edits `dist/VERSION`,
synthesizes a commit, or infers task ownership from the board, and a red
candidate publishes nothing.

**Invariant —** Fresh publish executes the complete canonical strict battery
over the exact source commit the plan names, and every arm must admit. This
host-side battery is distinct from sealed target proof replay. Tag-and-announce
recovery instead resolves preconditions and credentials, awaits registry
visibility, then tags and announces, without rerunning the battery or
republishing the package.

**Invariant —** The artifact gate proves composition and use without rebuilding
after authorization. The package materializes from sealed content-addressed
objects and must equal the previewed package and tree identities. The
compiled-host executable named by the sealed inventory must match the plan's
artifact digest, and a plan-keyed executable copy of it must succeed at
`--json version`.

### Proof is sealed per target and composed last

**Invariant —** A release has exactly three target payloads, in canonical
order: `aarch64-apple-darwin`, `x86_64-unknown-linux-musl`, and
`x86_64-pc-windows-msvc`. Each receipt-free payload binds its target, source
identity, app generation G, the digest of the app ZIP its executable links,
native executable, build pins, and artifact inventory, and all three bind the
source digest of the already-versioned release commit. Every target binds the
same G and archive digest. The inventory carries no app files: the executable
is the app's only carrier.

**Invariant —** Every appliance executable produced by a current build path
links its app ZIP. The Darwin, Linux, and Windows release builds, and
`spice dev xtask -- install` on supported macOS and Linux hosts, read the
staged `generations/G.zip` once and admit it as G. They link it as one retained
read-only section: `__TEXT,__spice_app_zip` on Mach-O, `spice_app_zip` on ELF,
and `spiceapp` on PE32+. Mach-O and ELF follow their platform naming
conventions: Mach-O sections are lowercase after two underscores, and the
System V gABI reserves dot-prefixed ELF section names for the system.

The Windows release build's COFF object defines `spice_app_zip` and
`spice_app_zip_len`; its PE32+ final-image reader locates exactly one
`spiceapp` section using the header's virtual and raw lengths. The section name
fits PE's eight-byte limit. Windows installation remains outside the current
installer's supported-host contract.

The Windows release executable exposes `--version` and `--json version`, the
identity surface exercised by sealed proof. Unix process supervision,
resident, and handle-bound installation commands are not silently emulated:
they remain absent until they have native Windows contracts. Disposable
repository commands run through `hosted-windows`, not through this release
entry point.

The current Mach-O and ELF builds then derive symbols, strip, and sign where
applicable. No step appends bytes after signing. The final image's section
metadata is the canonical external extraction boundary; no build-time file
offset is persisted. The running process stores no offset and parses no image:
the linker resolves both symbols to the mapped address and byte length. The
final executable is refused unless exactly one section carries the staged
archive and its one version report names G. Only these builds write the object
carrying the archive and enable the `canvas` feature that names its symbols, so
no compile step reads a file outside the selected tree. A resident whose
executable carries no archive refuses installed serving at startup.

**Invariant —** Proof replays the complete current battery against the
candidate checkout: Linux mounts it read-only in the pinned OCI environment,
Darwin passes it to the attested native runner, and Windows dispatches the
batch to one attested ephemeral GitHub-hosted runner. Replay is bound to
payload, environment, and battery, but does not prove that the checkout equals
the payload's source digest. Unsupported hosts, drift, infrastructure failure,
or a non-clean replay publish no receipt; a refused replay is a typed refusal.

**Invariant —** A proof receipt is detached and canonical. Admission requires
target, payload digest, environment, battery digest, replay digest, and clean
standing to form one identity chain.

**Invariant —** The release manifest is post-proof: every supported target
with a clean, mutually consistent receipt, or no release. Content-addressed
objects publish before the sole manifest pointer, so a reader sees the prior or
the new complete seal, never a pointer to missing proof.

### Ephemeral native Windows execution

The repository's Windows execution boundary is a fresh GitHub-hosted
`windows-2025` x86-64 machine running Windows Server 2025 Datacenter on the
provider's Visual Studio 2026 image. No dedicated Windows host is owned, kept
running, or administered. Wine, the
Darwin-to-MSVC cross-toolchain, Linux containers, and synthetic PE fixtures
remain useful format/build checks but never substitute for this native
execution.

Run one argv, with no shell joining or reinterpretation, from a selected
worktree:

```sh
spice dev xtask -- windows-hosted -- PROGRAM ARG...
```

Stage the Windows release target through the same boundary:

```sh
spice dev xtask -- stage-target x86_64-pc-windows-msvc
```

`PROGRAM` is either a path beneath the selected worktree or one admitted tool
token: `@runner`, `@cargo`, `@rustc`, `@git`, `@cl`, or `@rc`. Proof replay
sends its five commands as one bounded batch, so one signed provider run
identity covers the complete battery. The client materializes the selected
worktree in a temporary Git index, creates a one-use commit, computes a
cross-platform SHA-256 identity from the exact source tree's recursive Git
listing, and pushes only a temporary `refs/tags/spice-hosted-*` ref. It never
pushes or moves a branch. It deletes that ref and the result artifact after the
run.

GitHub registers `workflow_dispatch` only from the default branch. The
repository command therefore becomes live only after
`.github/workflows/windows-hosted.yml` has landed there; a checkout that has
the client but predates that landing refuses at dispatch rather than selecting
another workflow.

The supported provider repository is `github.com/infimalabs/spice-rust`.
Authenticate the machine-local `gh` client and Git transport separately.
The Git credential needs Contents write permission to create and delete tags.
The GitHub CLI credential needs Actions read/write, Attestations read, Contents
read, and Metadata read for this repository; the workflow's short-lived
`GITHUB_TOKEN` gets Attestations write for signing and Contents write only so
its `always` cleanup can retire the exact one-use `spice-hosted-*` tag, plus
OIDC identity-token write. The client cancels the exact dispatched run on
`SIGINT` or `SIGTERM`; provider cleanup is the backstop when the client
disappears before it can act. Tokens, mutable endpoints, and provider account
configuration are never tracked policy. Rotate or revoke GitHub credentials in
the provider, then run `gh auth logout --hostname github.com`; delete or rotate
the corresponding SSH key through GitHub and the local keychain.

`dist/windows-hosted.toml` is the tracked trust policy. Its `provider`,
`repository`, and `workflow` keys bind dispatch; `[image]` pins the
`windows-2025` label, image version, OS build, and `X64` architecture; and
`[toolchain]` pins Rust, Cargo, Git, MSVC, Windows SDK, Trunk and
`cargo-mutants`, plus every executable, `PATH`, include, and library location.
The bootstrap Cargo invokes the pinned `RUSTC` path under the same pinned
`RUSTUP_TOOLCHAIN`; workflow-step scope never lets it fall back to the runner
image's mutable stable proxy. The workflow pins every third-party action by
full commit. Updating the image
means reviewing the matching `actions/runner-images` release manifest,
changing every affected tool/path pin together, and proving the runner smoke
before landing.

The sealing step builds the guest from the selected source and executes the
sealed request in the disposable VM, then signs that exact
`hosted-windows.exe` and its result separately with GitHub's workload identity.
Nothing publishes unless both attestations succeed. Verification rejects
self-hosted attestations and binds both subjects to the one-use source ref and
commit. The guest hashes itself under
`spice.proof.native-runner.v1`; the client verifies both attestations, hashes
the downloaded runner, and refuses unless that independent identity equals the
one in the result. Runner or pin changes require the same native smoke before
landing.

Every dispatch has a fresh 256-bit challenge. The signed result must name that
challenge, this repository and workflow, the provider run and attempt, the
exact source archive, image, toolchain, runner digest, and every returned
staging artifact's path, length, and digest. The client verifies GitHub's
Sigstore/OIDC attestations before it reads the result as evidence. Changed
source, image, tools, runner, repository, workflow, run identity, challenge, or
artifact bytes refuse. A successful Windows stage copies content-addressed
objects first and the mutable target payload pointer last.

The guest gives the command batch a read-only source tree, one fresh Cargo home
and target directory, bounded output per member, bounded member runtimes, a
Windows Job Object that kills descendants, and a machine-wide guest-firewall
rule covering every non-loopback IPv4 and IPv6 destination. It passes each argv
directly through `CreateProcess`, clears ambient environment, and names every
executable by an admitted path. Result publication happens only after the rule
is removed and scratch teardown succeeds. Provider cancellation or the
75-minute workflow limit destroys the ephemeral VM. GitHub-hosted concurrency
and billing limits are the account's cost boundary; the repository requests
one machine per dispatch and defines no persistent or autoscaling fleet.

### Persistent Windows 11 ARM developer guest

Windows work iterates on the Apple-silicon development host before it reaches
the hosted boundary. One persistent Windows 11 Pro ARM64 guest runs the pinned
x64 cross-build under Windows' x64 emulation, so NTFS, Job Object, named-pipe,
process-tree, and install behavior come from a real Windows kernel. Its results
are developer evidence only: the guest publishes no receipt, satisfies no
release, sealing, or package-replay claim, and no gate or release path executes
or trusts it. Permanent tests stay fast and in process; every guest run is a
throwaway smoke.

**Hypervisor.** The guest runs under Homebrew QEMU (`qemu-system-aarch64
-machine virt -accel hvf`) with edk2 UEFI firmware. QEMU is driven entirely
from argv and its QMP control socket, so the repository owns the whole machine
definition, and its open-source binaries and firmware are pinned by digest.
qcow2 overlays make the recorded clean state a file operation. It boots Windows
11 ARM through Hypervisor.framework. Apple Virtualization.framework, and so
tart, cannot boot Windows. UTM is a GUI front end to the same QEMU, and
executing commands through it needs a guest agent. VMware Fusion's `vmrun`
needs an account-gated download plus VMware Tools in the guest, and
Parallels' `prlctl` needs a paid edition. Each of those adds an opaque control
layer without adding Windows support.

**First-time setup.** Install the pinned bottles once with
`brew install qemu wimlib`, then run:

```sh
spice dev xtask -- windows-guest provision
```

`dist/windows-guest.toml` is the recorded configuration. It pins:
- each host tool and firmware file by path and SHA-256;
- the machine shape;
- the Windows 11 Pro ARM64 ESD from Microsoft's Media Creation Tool catalog;
- the virtio-win NetKVM driver ISO;
- the Win32-OpenSSH ARM64 MSI;
- the x64 VC++ runtime installer named by the package manifest that
  `dist/windows-msvc.toml` pins. It supplies the `VCRUNTIME140.dll` that
  Rust MSVC executables load and that the hosted image gets from Visual Studio.

Provisioning:
1. Fetches each download over its pinned URL and refuses any size or digest
   mismatch.
2. Builds read-only FAT32 Setup media with wimlib. The media carries Windows
   PE, Setup, the Pro image, the network driver, OpenSSH, the x64 runtime, the
   host's guest SSH public key, `dist/windows-guest/setup.ps1`, and
   `dist/windows-guest/autounattend.xml` with a random local password.
3. Installs Windows unattended (this takes tens of minutes).
4. Seals the installed disk and UEFI variables read-only as the clean base.
5. Learns the guest's SSH host key on first boot.

Updating a pin means changing it in the declaration and running
`provision --replace`; a guest provisioned from other pins or templates is
refused until then. Machine-local state lives in the shared
`.spice-rs/windows-guest/` directory and is never tracked. Sizing is 4 vCPUs,
8 GiB RAM, and a sparse 64 GiB NVMe disk. Provisioning needs about 25 GB of host
space for the download cache and media, and a working guest needs 10–20 GB.

**Commands.** Each subcommand runs as
`spice dev xtask -- windows-guest <subcommand>`:

| Subcommand | Effect |
| --- | --- |
| `provision [--replace]` | Build the clean base from the recorded configuration. |
| `start` | Boot the persistent overlay in the background; returns once SSH answers. |
| `stop` | Shut Windows down cleanly; QEMU is stopped forcibly only if shutdown stalls. |
| `status` | Report whether the guest is provisioned from the current pins and whether it is running. |
| `reset` | While stopped, return the disk and UEFI variables to the clean base. |
| `sync [--artifact PATH]... [--source PATH]...` | Replace `C:\spice\work` with the named artifacts in `bin\` and the named tracked source in `source\`. |
| `run [--timeout S] [--output-bytes N] -- PROGRAM [ARG...]` | Run one argv in `C:\spice\work`. |

The guest persists across `stop` and `start` until `reset`. It reaches the
host only through loopback forwards: SSH on 22622 (key-only, pinned host key)
and QMP on 22623. QEMU's `restrict=on` networking gives the guest no outbound
network once it is installed.

`sync` admits only regular files beneath `target/x86_64-pc-windows-msvc/` whose
PE machine is x86-64. A native ARM64 build would run outside emulation and
bypass the pinned x64 cross-build, so it is refused. `--source` admits only
paths `git ls-files` tracks. Every sync also installs the guest executor, which
is built by
`spice dev cargo -- build --locked --release --target x86_64-pc-windows-msvc -p windows-guest`.

**Shared protocol with the hosted runner.** `run` reuses the hosted
boundary's types and bounds: the program-plus-arguments `Command`, the 3600 s
and 8 MiB defaults (also the maxima), the 256-argument limit, the base64
`Capture` of stdout and stderr with truncation flags, and the `Completion` exit
mapping (timeout reports 124). The command prints the captured bytes, reports
the exact Windows exit code on stderr, and exits with it, as
`windows-hosted` does.

The executor runs that argv like the hosted runner does:
- straight through `CreateProcess`, with no shell;
- with a cleared environment, keeping only a fixed Windows system allowlist,
  plus a `PATH` of `bin\` and the system directories and a private
  `TEMP`/`TMP`;
- inside a kill-on-close Job Object that bounds output and runtime and ends
  every descendant on timeout.

The hosted runner's own `--smoke-argv` mode runs unchanged in the guest after
`sync --artifact target/x86_64-pc-windows-msvc/release/hosted-windows.exe`. Its
attested batch mode does not run there: it requires GitHub's run identity and
an in-guest toolchain, and the guest must never imitate the former.

**Port smokes.** Cross-build the port's in-process test binary and run it under
the real Windows kernel. Cargo prints each test executable's path as
`target/x86_64-pc-windows-msvc/debug/deps/NAME-HASH.exe`. `spice` and `sill`
build for this target only once the corresponding port lands. Until then,
these are the commands each port's native developer smoke must pass.

```sh
spice dev cargo -- build --locked --release --target x86_64-pc-windows-msvc -p windows-guest
spice dev xtask -- windows-guest start

# Filesystem and Cargo isolation (PLATFOR-dWJhTHGms)
spice dev cargo -- test --locked --no-run --target x86_64-pc-windows-msvc -p spice --lib
spice dev cargo -- build --locked --target x86_64-pc-windows-msvc -p spice
spice dev xtask -- windows-guest sync --artifact target/x86_64-pc-windows-msvc/debug/deps/spice-HASH.exe --artifact target/x86_64-pc-windows-msvc/debug/spice.exe
spice dev xtask -- windows-guest run -- bin/spice-HASH.exe cargo_isolation

# Job Object process ownership (PLATFOR-dWJhVJ7Sd)
spice dev cargo -- test --locked --no-run --target x86_64-pc-windows-msvc -p sill
spice dev xtask -- windows-guest sync --artifact target/x86_64-pc-windows-msvc/debug/deps/sill-HASH.exe
spice dev xtask -- windows-guest run -- bin/sill-HASH.exe spawn

# Local IPC with daemon handoff (PLATFOR-dWJhWMJFc)
spice dev cargo -- test --locked --no-run --target x86_64-pc-windows-msvc -p spice --lib
spice dev xtask -- windows-guest sync --artifact target/x86_64-pc-windows-msvc/debug/deps/spice-HASH.exe
spice dev xtask -- windows-guest run -- bin/spice-HASH.exe resident::
spice dev xtask -- windows-guest run -- bin/spice-HASH.exe supervisor::

spice dev xtask -- windows-guest stop
spice dev xtask -- windows-guest reset
```

Each port adds its end-to-end smoke by syncing its built `spice.exe` or
`sill` consumer next to the tests and running that argv the same way:
- one prepare/select/cleanup cycle;
- the descendant, console-interrupt, forced-tree, and reap cycle;
- the resident start, lane-control round trip, and shutdown.

`reset` between ports keeps each run starting from the same clean state.

**Differences from the hosted job.** Each one keeps guest evidence below the
hosted boundary:

| Aspect | Guest | Hosted job |
| --- | --- | --- |
| Emulation | x64 code runs under Windows 11 ARM's x64 emulation (Prism) on an ARM64 kernel | Native x86-64 |
| CPU features | Emulation exposes a subset of x86-64 extensions and reports a different CPU identity; feature detection, intrinsics, and floating-point corner cases may differ | Real x86-64 |
| Timing | Translation warm-up, nested virtualization, and laptop power state skew durations, timeouts, and scheduling, so deadlines must not be calibrated from the guest | Provider timing |
| Installer architecture | Windows 11 Pro client build 26100.4349 with an ARM64 OS and ARM64 system components; x64 installers may select ARM64 or emulated paths | Windows Server 2025 Datacenter 26100 x64 |
| Attestation | No provider run identity, Sigstore attestation, challenge, or receipt; results are machine-local developer output | Attested and receipted |
| Configuration | Unactivated, with TPM and Secure Boot checks bypassed for Setup and no in-guest toolchain; networking is loopback only | Pinned image, toolchain, and firewall rule |

### Notes are read out of the ledger

**Invariant —** Notes derive from the ledger; there is no maintained changelog.
The window runs from the highest-versioned prior tag reachable from the release
commit, addressed through its fully qualified tag ref so a same-named branch
cannot redefine it. With no reachable tag, a bounded window of recent landings
names that basis. Only first-parent landings count, so one merged unit of work
is one entry.

**Invariant —** Work added and reverted inside one window is suppressed on both
sides. A revert pairs with the landing whose history contains the reverted
commit, not one whose identifier equals it, because the reverted commit usually
arrived on a side branch.

**Invariant —** Landings sharing a task collapse into one entry at the task's
first landing; its latest landing supplies the subject and project. Entries
group by that declared project, never by touched paths, with an empty or
`agent.*` declaration mapped to `general`. Buckets appear in first-seen order and
keep entry order; equal descriptions merge only within a bucket and retain every
reference.

**Invariant —** A trailing handle is stripped from a description only when it
matches that commit's own recorded identifier.

**Invariant —** Curation owns the region above the generated inventory. Plan
building regenerates the inventory against the release commit, so a curated
file never publishes a stale range. Notes refuse when untouched, when they still
carry the placeholder, or when the curated region holds only banner and
headings.

### The publication sequence

**Invariant —** The only external release provider is the tracked Git authority:
remote `origin`, source `refs/heads/main`, package
`refs/spice/packages/vVERSION`, tag `refs/tags/vVERSION`, and announcement
`refs/spice/releases/vVERSION`. Runtime configuration cannot redirect a step.

**Invariant —** Publication never moves the source branch. The package ref
converges first through a guarded no-effect rehearsal; tag and announcement
converge to the release commit only after bounded observation sees the exact
package object. Exhausting that observation is a terminal refusal, not
permission to advertise unobservable content.

**Invariant —** A version names exactly one package object and one release
commit. A package, tag, or announcement ref pointing elsewhere refuses rather
than moving; a matching ref is already converged and left untouched. An
interrupted release is completed by re-running it from the first incomplete
boundary, never unwound.

**Invariant —** An operator interruption terminates with a status distinct from a
gate failure.

### Installing into a repository the product does not own

One canonical status-bearing receipt owns the exact plan, fixed-size references
to first-captured prior images, durable progress, and reversal outcomes. The
referenced raw bytes live once in a receipt-owned content-addressed store.

**Invariant —** What the operator reviews is produced purely by observation.
Apply requires that plan's exact digest, re-resolves the plan and executable
repository configuration, and refuses before mutation when either moved. A
process that derives and applies its own plan, such as the development refresh,
may carry it into apply but still proves every bound source, generation,
destination, and receipt image before effect.

**Invariant —** Preview never invokes maintenance. After admission, apply and
`--unapply` may run one bounded pass that reclaims only recognized stale
install-record publications; it is not a plan operation and never imports,
repairs, or deletes foreign state.

**Invariant —** Before the first destination effect, apply atomically accepts
one canonical `applying` receipt with the exact plan, repository identity, zero
progress, and fixed-size prior-image references, then durably publishes every
referenced raw payload before changing a destination. No plan change happens
until the receipt describes it, and each completion is durably published before
the next mutation, so a crash leaves at most the recorded frontier ahead of its
receipt. Terminal `complete` is published only after every stage completes; an
interrupted apply resumes only the same plan.

**Invariant —** The receipt records the exact prior image the installation may
restore: a fixed-size mode/content reference or absence; prior configuration
values or absence; and whether it introduced a parent directory. Referenced
regular-file bytes are stored once outside the repeatedly rewritten receipt,
under their domain-separated content identity. Prior state is captured once, at
first application, and never re-derived from the installation's own output.
Recording only the new state would make absence unrecoverable.

**Invariant —** The installed executable is the app's only carrier.
Installation writes no app file. Its receipt binds the canvas generation G
named by the executable's linked archive, beside the executable operation. A
plan reads the selected executable once and admits that archive, inflating
only its shell. Upgrading the app is replacing the executable.

**Invariant —** Install schema 18, which published app sidecar files under
`.spice-install/app`, is a one-crossing boundary. Only `spice install` reads
such a receipt, and only a complete, canonical, digest-exact one. Its fixed,
executable, symbol, and hook effects keep their reversal images in a schema-19
refresh. Its app operations serve only as evidence of the sidecar files it
owns. That refresh publishes its `applying` receipt first. It then discards
each owned sidecar only while the file still holds the image schema 18
recorded, and withdraws each generation, `generations`, and `app` directory
only once empty. An interrupted, inexact, or otherwise unadmissible schema-18
receipt refuses, naming its schema and owner.

**Invariant —** Generated artifacts live in a directory the product owns and are
activated by redirecting the host tool there, never by writing into the location
the repository already uses. Every generated artifact carries an ownership
digest, so an untouched installation is distinguishable from a locally modified
one and from a newer generated shape.

### Reversal is a claim about identity

An uninstall that reverts by name destroys operator edits. Reversal restores
only what it can still recognize as its own.

**Invariant —** `--unapply` is the explicit reversal direction and has no
preview digest. It advances the exact `complete` receipt generation to
`unapplying`, runs the exact inverse of the canonical application order, and
publishes `uninstalled` only after every operation has a recorded outcome. A
moved generation refuses and preserves the current receipt.

**Invariant —** Identity drift is a typed refusal, not a partial success: it
overwrites nothing, preserves the changed target and the receipt, and retry
resumes the same receipt. A container the installation created is removed only
when still empty.

**Invariant —** An interrupted reversal blocks a new installation until it is
resumed, so two plans never contend for one receipt's first-captured prior
state.

**Invariant —** Both directions are resumable and idempotent: a re-run performs
only operations not yet recorded complete, and a plan already satisfied against
unchanged state performs no receipt-owned effect.

### The canonical receipt

The canonical receipt is the sole installation-status, transaction, and
reversal authority.

**Invariant —** Every receipt generation is canonical and published atomically
through a real non-symbolic file, only while the exact prior generation and
complete image still match; a stale or concurrent writer refuses.

**Invariant —** An unreadable, truncated, noncanonical, linked, or inconsistent
receipt refuses. Reading damaged provenance as "not installed" would strand
active modifications with no owner and no path to removal.

**Invariant —** Every receipt is bound to the repository it describes. A
nonterminal receipt refuses when it names another Git-common owner, as does a
terminal copy while its original owner still exists. A `complete` or
`uninstalled` receipt may rebase only after the old owner is gone, moving
repository, plan, and activation bindings together.

**Invariant —** The receipt remains after clean terminal directions, retaining
its plan, prior-image references, status, and progress. Its referenced raw
payloads remain available for the receipt's next executable direction. Absence
is never synthesized by deleting provenance.

**Invariant —** Install support is bounded by receipt schema, not product
version. The current schema is the sole executable shape. Each retired schema
refuses before receipt-owned mutation and names the release change whose
executable can unapply it, recorded in the landing that retires it. Malformed,
inconsistent, and future receipts are store failures, never retired state.

---
