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
fresh execution, and every resumption. There is one release action and one
ordered operation graph. Before the first effect, admission proves the
converged prefix: each operation is met by its durable receipt or by
authoritative external state. An exact visible package meets every operation
through registry visibility, a tag at the release commit meets tagging, and an
announcement at the release commit meets announcing. A receipt whose own
registry, tag, or announcement state is absent, external state naming another
release, or a met operation after an unmet one refuses before any effect. A met
operation is receipted as already converged without crossing its effect, so
no resumption reruns the gate, republishes a visible package, retags, or
repeats an announcement. Every other operation records its exact prior state
before crossing its effect boundary and a typed receipt after convergence. A
journal written under an earlier journal schema refuses before any effect,
naming the release that last wrote it, and is never decoded for execution. A
parallel verification or publication body would be a second release authority.

### A versioned commit is sealed before publication

**Invariant —** Every landing is a version: its product version is derived from
`dist/VERSION` and the commit's own instant, as described under Version
identity in the Rust profile. Release requires clean HEAD at the release
commit, already integrated in the tracked source branch, with requested,
compiled, and tracked product versions in agreement. Compiled is this
executable's `dist/VERSION` applied to the release commit, and tracked is the
checked-out `dist/VERSION` applied to it. Every other shape refuses. Release
never edits `dist/VERSION`, synthesizes a commit, or infers task ownership from
the board, and a red candidate publishes nothing.

**Invariant —** A release whose package is not yet visible executes the
complete canonical strict battery over the exact source commit the plan names,
and every arm must admit. This host-side battery is distinct from sealed target
proof replay.

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
identity, app generation G, the digest of the spice rack its executable links,
native executable, build pins, and artifact inventory, and all three bind the
source digest of the already-versioned release commit. Every target binds the
same G and rack digest. The inventory carries no app files: the executable
is the app's only carrier.

**Invariant —** Every appliance executable produced by a current build path
links one spice rack: a ZIP whose `app/` members are the staged generation's
`generations/G.zip`, with stored streams copied unchanged, beside the root
`SKILL.md` from `spice/agent/SKILL.md` and `docs/...` from `docs/`. The
Darwin, Linux, and Windows release builds, and `spice dev xtask -- install` on
supported macOS, Linux, and x86-64 Windows hosts, assemble it through one path
and admit it as G, so the same staged generation and tracked sources link the
same bytes. Worktree
dSYMs exist only on macOS, so elsewhere that command publishes without them.
Each links it as one retained read-only section: `__TEXT,__spicezip` on Mach-O
and `spicezip` on ELF and PE32+. The Mach-O section follows Apple's
two-underscore convention, ELF remains undotted because the System V gABI
reserves dot-prefixed names for the system, and the one word exactly fills
PE's eight-byte section-name limit.

Every target's relocatable object defines `spicezip` and `spicezip_len`; the
final-image readers locate exactly one rack section and recover the rack bytes
from it. The Windows target inventory names `bin/spice.exe`
and the matching `symbols/spice.pdb`; both are package members, while
`proof.json` is the package's detached signature artifact.

The Darwin target inventories one schema-4 symbol artifact containing only the
captured `Info.plist` and complete `DWARF/spice` bytes. A short canonical
metadata header is followed directly by one deterministic deflate stream over
both members; their decoded lengths split the stream without duplicating either
member or expanding it through a text encoding. Either
decoded member and their combined decoded length are bounded at 402,653,184
bytes; the complete compressed artifact is bounded at 268,435,456 bytes.
Schemas 2 and 3 are retired and refused as last written by release
`0.2.0-dev.0`, never decoded through predecessor arms. Capture admits
`dsymutil`'s optional `Relocations/<one-architecture>/spice.yml` by-product but
does not publish it; any other extra dSYM entry refuses staging.

Windows deliberately admits no embedded Authenticode certificate. The
executable, PDB, payload, linked app archive, build pins, provider run, runner
image, and toolchain are instead bound by the public GitHub Sigstore
attestation over the hosted result. The PE image is re-admitted after the
link—the only byte-changing Windows step—and its content digest is re-admitted
when packaged, so neither symbol capture nor detached signing can mutate the
image.

The release adapter opens each native executable and Windows PDB once, admits
the held regular object's cross-platform identity, reads its exact admitted
length through that handle, and then requires both the handle and its final
no-follow name to retain the same identity. A link, reparse point, mutation, or
name replacement therefore refuses before artifact publication.

The same complete Windows appliance exposes ordinary commands,
resident, journal, board, app serving, and distribution surfaces through their
native filesystem, process, and local-IPC adapters.

The Windows release executable exposes `--version` and `--json version`, the
identity surface exercised by sealed proof, the repository installer described
under Windows installation, and the resident `up` that serves the linked app.
Unix process supervision commands are not silently emulated: they remain absent
until they have native Windows contracts. Disposable repository commands run
through `hosted-windows`, not through this release entry point.

The current Mach-O and ELF builds then derive symbols, strip, and sign where
applicable. No step appends bytes after signing. The final image's section
metadata is the canonical external extraction boundary; no build-time file
offset is persisted. The running process stores no offset and parses no image:
the linker resolves both symbols to the mapped address and byte length. The
final executable is refused unless exactly one section carries the staged
archive and its one version report names both the admitted product version and
G exactly. Only these builds write the object carrying the archive and enable
the `canvas` feature that names its symbols, so
no compile step reads a file outside the selected tree. A resident whose
executable carries no archive refuses installed serving at startup.

**Invariant —** Darwin staging gives the release executable a notarized
Developer ID signature before the pipeline reads the bytes it digests: after
the dSYM is derived and the image stripped, it signs with the hardened runtime
and a secure timestamp, submits an archive of the signed file to Apple
notarization, and finishes only when strict `codesign` verification and
Gatekeeper's primary-signature assessment accept the file as notarized. The
target payload, its inventory, and the symbol artifact therefore bind the
signed bytes, and no step changes the executable after they are read. The
signature and notarization are a step of the one staging build, never a
rebuild: Apple's timestamp makes two signings of the same image differ, so a
signed digest is never reproduced, and nothing reproduces it. The artifact
gate and proof replay verify the sealed digests of the bytes the stage wrote.
The bare executable cannot carry a stapled ticket, so a recipient's first
assessment is online.

The signer is named by two machine-local Git-common keys,
`release.darwin.identity` (a `Developer ID Application` keychain identity) and
`release.darwin.notary_profile` (a `notarytool` keychain profile). Only names
enter configuration; the certificate, private key, and notarization credential
stay in the operator's keychain. Darwin `stage-target` and `seal-darwin` admit
the pair against the keychain and the notary service before the first build.
With neither key set they refuse before any build unless `--unsigned` asks for
an executable Gatekeeper blocks on every other Mac; with only one set they
refuse by naming the other.

Darwin is the only target with a signing route today. The Linux and Windows
executables ship unsigned, and those unsigned executables are their releases
for now, so every command that builds one names that result: `stage-target
x86_64-unknown-linux-musl`, `stage-target x86_64-pc-windows-msvc` (including
the command the hosted route runs remotely), and `windows-appliance` refuse
before any build unless given `--unsigned`. A signing route for either target
replaces its requirement with the configured signer, as Darwin's did.

`spice dev xtask -- share-darwin` applies the same signing to a separate built
Darwin executable outside staging; its result is not a sealed artifact.

**Invariant —** Linux staging links and executes the Linux release on Linux.
A Linux host stages `x86_64-unknown-linux-musl` natively. Any other host runs
that same `stage-target` inside the pinned `linux/amd64` proof image
(`dist/proof-image/identity.txt`, recorded as the payload's `proof_container`
pin) through the Docker or Podman executable `proof.container` names, so the
musl link uses the linker the image's root Cargo config pins and
`release:canvas-version` executes the built executable inside the image.
The host stages the app first; the image then mounts the worktree and its Git
common directory read-only, makes `dist/stage` its one writable worktree
path, and builds with a fresh Cargo home and target and no network. No Cargo
config above the worktree and no host tool reaches that build. With
`proof.container` unset or not an admitted runtime, or with the pinned image
not loaded, staging refuses before any build and names the missing piece.

**Invariant —** A native Darwin proof runner is sealed for exactly one
worktree: `dist/proof-runner/build.sh` bakes that worktree's HEAD commit and
vendor tree into the runner and binds it through `proof.runner`, which admits
only the worktree configuration layer. A Git-common default would hand every
other worktree a seal for a different commit, so one is reported as a policy
issue, never applied. A runner whose sealed commit differs from HEAD refuses
by naming both commits and the reseal, and the staging preflight names the
bound runner it asked. `spice dev xtask -- seal-darwin [--output DIR]
[--unsigned]` is the one-command point-in-time build: it seals and binds this
worktree's runner, stages and signs the Darwin target, and places the
executable under `DIR`, writing only this worktree's `target/`, `dist/stage/`,
Git-directory configuration, and
`DIR`; the installed appliance, the resident, and other worktrees are
untouched.

**Invariant —** Proof replays the complete current battery against the
candidate checkout: Linux mounts it read-only in the pinned OCI environment,
Darwin passes it to the attested native runner, and Windows dispatches the
batch to one attested ephemeral GitHub-hosted runner. Replay is bound to
payload, environment, and battery, but does not prove that the checkout equals
the payload's source digest. Unsupported hosts, drift, infrastructure failure,
or a non-clean replay publish no receipt; a refused replay is a typed refusal.

> **Not current.** The GitHub-hosted Windows route described in this plane does
> not work right now and is not the way to run native Windows work. Use the
> [local Windows guest](#persistent-windows-11-arm-developer-guest) instead.

The Windows proof boundary requires the configured repository visibility to be
`public` before it dispatches the release battery. A private repository returns
the typed hosted-attestation prerequisite and publishes no proof receipt;
cross-builds, Wine, the Windows 11 ARM developer VM, and self-hosted keys cannot
replace this boundary.

**Invariant —** A proof receipt is detached and canonical. Admission requires
target, payload digest, environment, battery digest, replay digest, and clean
standing to form one identity chain.

**Invariant —** The release manifest is post-proof: every supported target
with a clean, mutually consistent receipt, or no release. Content-addressed
objects publish before the sole manifest pointer, so a reader sees the prior or
the new complete seal, never a pointer to missing proof.

**Invariant —** Staged artifacts and target payload pointers, proof receipts,
and release manifest objects and pointer all publish through one rooted
regular-file boundary. A path must be relative and stay beneath its root; an
ancestor that is a symlink or not a directory, or a destination that is not a
regular file, refuses before mutation. Bytes and their exact mode land in a
private same-directory file that replaces the destination only once complete,
so an interrupted publication leaves the prior file or none.

### Ephemeral native Windows execution

> **Not current.** This GitHub-hosted route does not work right now and is not
> the way to run native Windows work. Use the
> [local Windows guest](#persistent-windows-11-arm-developer-guest) instead;
> the description below records the hosted design only.

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
spice dev xtask -- stage-target x86_64-pc-windows-msvc --unsigned
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
`SIGINT` or `SIGTERM`; its non-interactive provider subprocesses run in their
own process group, so a terminal interrupt reaches only the client. After the
provider returns an exact run identity, every failure first confirms that run
is terminal, cancelling it if necessary; success, failure, cancellation, and
verification refusal then retire any result artifact for that exact run. The
one-use tag is retired whenever a push was attempted. Provider cleanup accepts
only that tag and is the backstop when the client disappears before it can act.
Tokens, mutable endpoints, and provider account configuration are never tracked
policy. Rotate or revoke GitHub credentials in the provider, then run
`gh auth logout --hostname github.com`; delete or rotate the corresponding SSH
key through GitHub and the local keychain.

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
and target directory, bounded output per member, bounded member runtimes, and a
machine-wide guest-firewall rule covering every non-loopback IPv4 and IPv6
destination. Each command root is created suspended, assigned to a kill-on-close
Windows Job Object, and resumed only after assignment, so no descendant can run
outside the owned tree. The guest passes each argv directly through
`CreateProcess`, clears ambient environment, and names every executable by an
admitted path. Result publication happens only after the rule
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
`.spice/windows-guest/` directory and is never tracked. Sizing is 4 vCPUs,
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

The host owns every Cargo build. A complete appliance cycle is:

```sh
spice dev xtask -- windows-msvc
spice dev xtask -- windows-appliance --unsigned
spice dev xtask -- windows-guest sync --artifact target/x86_64-pc-windows-msvc/release/spice.exe
```

Until the hosted route is usable, this local cross-build is how Windows
executables ship. `windows-appliance` therefore applies the release source rule
before it builds: from a checkout equal to HEAD, with only release output
untracked, it links the same release product version the Darwin and Linux
stages of that commit report. From any other checkout it names the difference
and links the `-dev` version, so a release version never vouches for
uncommitted source. The result is still not a staged payload and carries no
receipt.

Only the x64 PE crosses into the guest; the initial guest needs neither Rust nor
Visual Studio, and Windows 11 ARM executes it through Microsoft x64 emulation.
Keep the one qcow2 disk thin-provisioned and retain no VM snapshots. Use
`spice dev xtask -- windows-guest reset` to discard smoke state and
`spice dev xtask -- build-cache --apply` when repository-owned host build roots
need reclamation.

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

Journal restore and artifact backup treat their roots as filesystem authority,
not path prefixes. Every intermediate directory is held and opened relative to
its held parent; a link, reparse point, or non-directory at any component is
corruption. Preflight, copy, and the post-copy identity check all repeat that
confined descent, so replacing an intermediate component cannot redirect bytes
into or out of a bundle.

**Port smokes.** Cross-build the port's in-process test binary and run it under
the real Windows kernel. Cargo prints each test executable's path as
`target/x86_64-pc-windows-msvc/debug/deps/NAME-HASH.exe`. `spice` and `sill`
build for this target only once the corresponding port lands. Until then,
these are the commands each port's native developer smoke must pass. Each
`sync` replaces the guest's `bin\`, so name every artifact a run needs in one
`sync`.

The guest account is a local administrator, and its SSH session, including
every `run`, holds an elevated token with backup and restore privileges that
override file DACLs. An elevated owner can therefore remove entries from a
sealed directory. Prove such owner refusals from an unelevated token: over the
guest's SSH, run the same test argv under
`runas /trustlevel:0x20000 "cmd /c C:\spice\work\bin\NAME-HASH.exe FILTER > C:\spice\tmp\OUT.txt 2>&1"`,
which starts asynchronously, then read `OUT.txt`.

```sh
spice dev cargo -- build --locked --release --target x86_64-pc-windows-msvc -p windows-guest
spice dev xtask -- windows-guest start

# Filesystem and Cargo isolation (PLATFOR-dWJhTHGms)
spice dev cargo -- test --locked --no-run --target x86_64-pc-windows-msvc -p spice --lib
spice dev cargo -- test --locked --no-run --target x86_64-pc-windows-msvc -p sill -p atomic -p serve --lib
spice dev cargo -- build --locked --target x86_64-pc-windows-msvc -p spice
spice dev xtask -- windows-guest sync --artifact target/x86_64-pc-windows-msvc/debug/deps/spice-HASH.exe --artifact target/x86_64-pc-windows-msvc/debug/deps/sill-HASH.exe --artifact target/x86_64-pc-windows-msvc/debug/deps/atomic-HASH.exe --artifact target/x86_64-pc-windows-msvc/debug/deps/serve-HASH.exe --artifact target/x86_64-pc-windows-msvc/debug/spice.exe
spice dev xtask -- windows-guest run -- bin/spice-HASH.exe config:: cargo_isolation agent_wrapper
spice dev xtask -- windows-guest run -- bin/sill-HASH.exe authority lock
spice dev xtask -- windows-guest run -- bin/atomic-HASH.exe
spice dev xtask -- windows-guest run -- bin/serve-HASH.exe discovery

# Journal backup, restore, and artifact member modes (PLATFOR-dWRp5cYTK)
spice dev cargo -- test --locked --no-run --target x86_64-pc-windows-msvc -p sill -p journal -p artifact --lib
spice dev cargo -- test --locked --no-run --target x86_64-pc-windows-msvc -p spice --bin spice
spice dev xtask -- windows-guest sync --artifact target/x86_64-pc-windows-msvc/debug/deps/sill-HASH.exe --artifact target/x86_64-pc-windows-msvc/debug/deps/journal-HASH.exe --artifact target/x86_64-pc-windows-msvc/debug/deps/artifact-HASH.exe --artifact target/x86_64-pc-windows-msvc/debug/deps/spice-HASH.exe
spice dev xtask -- windows-guest run -- bin/sill-HASH.exe authority
spice dev xtask -- windows-guest run -- bin/journal-HASH.exe
spice dev xtask -- windows-guest run -- bin/artifact-HASH.exe
spice dev xtask -- windows-guest run -- bin/spice-HASH.exe restore_command journal_command task_artifact_command

# Job Object process ownership (PLATFOR-dWJhVJ7Sd)
spice dev cargo -- test --locked --no-run --target x86_64-pc-windows-msvc -p sill
spice dev xtask -- windows-guest sync --artifact target/x86_64-pc-windows-msvc/debug/deps/sill-HASH.exe
spice dev xtask -- windows-guest run -- bin/sill-HASH.exe spawn

# Local IPC with daemon handoff (PLATFOR-dWJhWMJFc)
spice dev cargo -- test --locked --no-run --target x86_64-pc-windows-msvc -p door
spice dev cargo -- test --locked --no-run --target x86_64-pc-windows-msvc -p spice --bin spice
spice dev xtask -- windows-guest sync --artifact target/x86_64-pc-windows-msvc/debug/deps/door-HASH.exe --artifact target/x86_64-pc-windows-msvc/debug/deps/spice-HASH.exe
spice dev xtask -- windows-guest run -- bin/door-HASH.exe
spice dev xtask -- windows-guest run -- bin/spice-HASH.exe supervisor::

# Supervisor and keeper lifecycle on the Job boundary (PLATFOR-dWRdJ8yG9)
spice dev cargo -- test --locked --no-run --target x86_64-pc-windows-msvc -p sill
spice dev xtask -- windows-guest sync --artifact target/x86_64-pc-windows-msvc/debug/deps/sill-HASH.exe
spice dev xtask -- windows-guest run -- bin/sill-HASH.exe

# Repository appliance install (DIST-dWJhYqnct). The skipped test reads dSYM
# identity from metadata, which on Windows carries no file ID.
spice dev cargo -- test --locked --no-run --target x86_64-pc-windows-msvc -p spice -p sill --lib
spice dev xtask -- windows-guest sync --artifact target/x86_64-pc-windows-msvc/debug/deps/spice-HASH.exe --artifact target/x86_64-pc-windows-msvc/debug/deps/sill-HASH.exe
spice dev xtask -- windows-guest run -- bin/spice-HASH.exe install:: resident:: --skip install::symbols::tests::a_read_settles_only_on_one_unmoved_regular_file
spice dev xtask -- windows-guest run -- bin/sill-HASH.exe authority

spice dev xtask -- windows-guest stop
spice dev xtask -- windows-guest reset
```

Each port uses a throwaway end-to-end consumer beside its in-process tests; the
consumer is not retained in the permanent suite. The process-ownership consumer
starts a suspended child, proves `CTRL_BREAK` grace, forces an interrupt-ignoring
root plus descendant at its runtime bound, cancels a second tree, and independently
opens each descendant PID after completion to prove the Job reaped it. The
lifecycle consumer launches a two-process tree in a new group, reaches it only by
group id to interrupt and then force it, waits for the root's forced status, and
proves the emptied and an unknown group both refuse as not found. The
repository-install consumer needs Git for Windows under Program Files and the
`spice dev xtask -- windows-appliance --unsigned` image. It installs into a fresh
repository, runs the wrapper and hooks under Git's shell, reinstalls
convergently, and upgrades under a running resident that keeps serving the app.
It then reclaims the deferred image, resumes an apply killed mid-flight, and
proves an operator occupant is refused or restored exactly. The other port
smokes cover one prepare/select/cleanup cycle and the resident start,
lane-control round trip, and shutdown.

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

### Notes are read out of durable landing authority

**Invariant —** Notes derive from the journal; there is no maintained changelog.
Git first-parent ancestry only selects the exact integrated commit window. The
window runs from the highest-versioned prior tag reachable from the release
commit, addressed through its fully qualified tag ref so a same-named branch
cannot redefine it. With no reachable tag, a bounded window of recent commits
names that basis.

**Invariant —** Every selected commit belongs to exactly one contributing task
landing projected from a durable task Git boundary. Task identity, current title,
current project, review standing, and typed correction relationships come from
journal authority, never from commit subjects, trailers, or revert prose. A
pre-projection landing that cannot be recovered unambiguously refuses before
draft or release mutation and names the last release that wrote its shape.

**Invariant —** Landings sharing a task collapse into one entry at the task's
first selected commit. A correction task folds into its reviewed parent. A
parent whose latest durable review is a finding remains suppressed until a
later clean review. Entries group by the parent's exact current project, never
by touched paths, and use the parent's exact current title. Buckets appear in
first-seen order and keep entry order; equal titles merge only within a bucket
and retain every selected commit reference.

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
values or absence; and whether it introduced a parent directory. A refresh
preserves each destination's original reversal binding and transfers each
introduced directory to exactly one reversal while separately binding the
current parent used for publication. Referenced regular-file bytes are stored
once outside the repeatedly rewritten receipt, under their domain-separated
content identity. Prior state is captured once, at first application, and never
re-derived from the installation's own output. Recording only the new state
would make absence unrecoverable.

**Invariant —** The installed executable is the app's only carrier.
Installation writes no app file. Its receipt binds the canvas generation G
named by the executable's linked archive, beside the executable operation. A
plan reads the selected executable once and admits that archive, inflating
only its shell. Upgrading the app is replacing the executable.

**Invariant —** An install receipt of a schema before 19, including schema 18,
which published app sidecar files under `.spice-install/app`, is refused
before any mutation, naming the release that owns it; that release's own
executable unapplies it.

**Invariant —** Installation writes no agent skill file: a lane reads the
skill its executable links through `spice://SKILL.md`. Install schema 19, which
published a copy at `.spice-install/agents/skills/spice/SKILL.md`, is a
one-crossing boundary. Only `spice install` reads such a receipt, and only a
complete, canonical, digest-exact one. The schema-20 refresh derived from it
keeps every other effect's reversal image and plans the copy as a discarded
retirement, removed only while the file still holds the image its preview
observed. The refresh withdraws the directories schema 19 introduced for it
only once empty, and no unapply restores it. Once a receipt is complete or
uninstalled, a discard's recorded parent carries no authority: removing that
directory leaves the receipt readable, auditable, and refreshable, and the
next refresh binds the path where it resolves then. Every other install
command refuses a complete schema-19 receipt until that refresh applies; an
interrupted or uninstalled one refuses, naming `SPICE-dX13sl6Gm`.

**Invariant —** Generated artifacts live in a directory the product owns and are
activated by redirecting the host tool there, never by writing into the location
the repository already uses. Every generated artifact carries an ownership
digest, so an untouched installation is distinguishable from a locally modified
one and from a newer generated shape.

### Windows installation keeps the same contract under Windows names

**Invariant —** The Windows installer keeps the plan, receipt, and recovery
contract above. The executable publishes to `.spice-install\bin\spice.exe`.
Receipt destinations and introduced parents use `\`-separated relative names;
plan asset paths keep their logical `/` spelling. Modes are owner-only DACLs:
an executable image is `0o700`, every other file `0o600`, and a mode granting
group or other access refuses rather than being approximated. The same limit
binds prior state: a preview refuses, before anything is touched, an existing
target whose access reaches beyond its owner, because reversal could capture
its bytes but never restore that access.

**Invariant —** A receipt generation is the volume serial and file ID of the
object a name held. An identifier wider than 64 bits, as ReFS may assign,
refuses rather than truncating into another object's identity.

**Invariant —** Every directory from the Git common root to a destination's
parent is held without delete sharing for the whole operation, so none can be
renamed or removed beneath it. A new name is admitted by a no-replace move. An
expected destination is replaced without a gap: a second hard link first holds
the prior object and is proved to be the expected image, the staged image then
atomically replaces the destination, and a failed post-replacement proof moves
the prior object back. Every crash point leaves the destination holding the
prior or the staged image, which resume already distinguishes.

**Invariant —** Windows refuses to unlink an image a running process maps, or
a file opened without delete sharing. Removal and retirement then move that
object to a fresh canonical `.spice-install-<pid>-<counter>.tmp` name in the
same directory instead of failing. Maintenance scans the runtime directory and
the installed executable's directory. It reclaims such a file once it is
owner-only, owned by the current principal, older than the grace period, and
its minting process has exited; a file still in use stays for a later pass.

**Invariant —** Hook activation runs Git for Windows from the system Program
Files folder (`Git\cmd\git.exe`), never one found on `PATH`. `core.hooksPath`
and the generated hook launchers name local-drive paths in Git's `C:/...`
form; a common directory outside a local drive refuses.

**Invariant —** A file belongs to the current principal when its owner is a SID
the process token may assign as owner: the token user or an enabled owner
group, such as Administrators in an elevated token. Files an elevated install
creates therefore stay owner-only to every later process of that principal,
including hooks under Git for Windows' shell, whatever default owner the
launching runtime selected.

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
version. The current schema is the sole executable shape; the one exception is
the schema-19 crossing above, which only a refresh reads. Each retired schema
refuses before receipt-owned mutation and names the release change whose
executable can unapply it, recorded in the landing that retires it. Malformed,
inconsistent, and future receipts are store failures, never retired state.

---
