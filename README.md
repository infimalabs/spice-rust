# Spice

Spice is an operations console for supervising coding agents in real Git
worktrees. The [product model](docs/product/README.md) states what Spice is and
the invariants its code keeps.

## Install

Install from a source checkout with [rustup](https://rustup.rs), which reads
`rust-toolchain.toml`, and [Trunk](https://trunkrs.dev); the build names the
exact Trunk install command when the pinned version is missing. The xtask
builds the app, links it into the executable, and applies the digest-bound
installation plan derived from that build:

```sh
cargo run --locked -p xtask -- install --no-resident
```

After building an appliance above or obtaining a released executable, install
it into another Git repository by absolute path, previewing before applying:

```sh
cd /absolute/path/to/target-repository
SOURCE_SPICE=/absolute/path/to/spice
"$SOURCE_SPICE" install
"$SOURCE_SPICE" install --apply=DIGEST_FROM_PREVIEW
SPICE_COMMON=$(/usr/bin/git rev-parse --path-format=absolute --git-common-dir)
export PATH="$SPICE_COMMON/.spice-install/bin:$PATH"
hash -r
spice --json doctor
```

An installed appliance updates from a sealed release-package directory without
a Rust toolchain or source checkout:

```sh
spice install --package /absolute/path/to/spice-release-package
spice install --package /absolute/path/to/spice-release-package --apply=DIGEST_FROM_PREVIEW
```

[INSTALL.md](INSTALL.md) owns activation, upgrade, reversal, and recovery.

## Run

```sh
spice up
```

`up` returns once the resident is authenticated and prints its endpoint.
`--open` opens the browser, `--foreground` keeps the process attached, and
`spice down` drains the resident and releases its lock. Detached diagnostics
append to `<git-common>/.spice/resident.log`. The resident listens on a
loopback port the kernel picks unless `--host IP` or `--port PORT` names one; a
named port keeps the URL stable across restarts.

Lanes run the oh-my-pi agent, so `omp` must be on `PATH`. Seat a first lane
with `spice lane add [WORKTREE]`. A lane wakes and draws work under its
team's policy, Drive unless `lane.life` or `--life` says otherwise: a Drive
team starts its vacant lanes as work its own members filed appears, a Drain
team as any assignable work appears, and a Steer team's lane is started by
hand with `spice lane wake ID`. `--life` seeds only the policy of the agent's
own initial team, the one `lane add` forms when no `--team` is given; a lane
added with `--team ID` takes that team's policy instead.

A non-loopback listener requires `--allow-insecure-bind`. Add a URL-safe bearer
token unless the surrounding network is the intended access boundary:

```sh
spice up --host 0.0.0.0 --allow-insecure-bind \
  --auth-token correct-horse-battery-staple --open
```

The token reaches the detached resident through a one-shot private descriptor.
It never appears in the resident's argv or environment, discovery, log URLs, or
replies; `--open` places it in the URL fragment, outside the HTTP origin.

`spice app` shows the console in a native window instead of a browser tab:

```sh
spice app
```

Like `spice up`, it returns the terminal at once: a detached process attends
the window and the command answers with the endpoint once the window is up;
`spice app --foreground` attends from this terminal instead, blocking until
the window closes and reporting whether the resident was drained. Either way
it joins the resident already serving the repository, or starts one exactly as
`spice up` would (the same `--host`, `--port`, `--allow-insecure-bind`, and
`--auth-token` apply) and drains it when the window closes; a resident it
joined keeps running until `spice down`. Cmd+Q and Cmd+W close the window the
same way as its close button, so the resident is settled either way; the
window's Edit menu carries the standard copy, paste, cut, undo, and select-all
shortcuts and Start Dictation. The window admits its endpoint through the same
discovery check as `--open` and carries the token only over the private
descriptor `spice up` uses and in the URL fragment the window loads; the
detached attendant's own receipt goes to the resident log. A link to another
site opens in the system browser however it is followed (click, keyboard,
Cmd-click, or the context menu's Open Link) and the console stays put; the
console's own links stay in the window. Only web (http and https) links are
handed to the system. The window itself is drawn by
a child process running the same appliance from
`~/Library/Caches/dev.spice.app/Spice.app`, a minimal bundle `spice app`
writes and verifies on each launch, because macOS dictation only attaches to
a client with a bundle identity; the command that started or joined the
resident stays in charge of it. That bundle is also what names the
app Spice in the menu bar, the Dock, and the application switcher and gives
it the console's pepper mark as its icon, drawn by the appliance at every
size macOS asks for. The window is titled `Spice: <repository>` and reopens at
the size and position it last had for that repository, centred instead when
that frame is mostly off every connected screen. It takes the console's own
surface colour and its dark or light theme, including in the alerts and file
picker it opens, while the console's system theme keeps following macOS. It
has no separate title bar: the traffic lights keep their standard corner
position, as in every Mac app, inside the console's header, whose empty area
drags the window and answers a double-click with the system's choice of zoom,
minimize or nothing; the browser console's header is unchanged. On Windows
the window renders through Microsoft Edge WebView2 and opens centred on the
primary screen, shrunk to fit it. It has no system title bar either: the
header drags the window, a double-click maximizes or restores it, minimize,
maximize and close stand in the header's trailing corner as in every Windows
app, and every edge and corner resizes, with the shadow, rounded corners and
Snap of a native window. Elsewhere `spice app` refuses and `spice up --open`
remains the route. Under WebKit, the console reads `app_bg.wasm` from the
resident twice per load, because WebKit doesn't reuse its integrity-checked
preload. Over loopback that is one extra local read, so the preload stays for
Chromium. A reload with a task graph open also logs an access-control error
for the diagram renderer document. The reloaded graph still renders, so
nothing changes for it.

Supervised shells may route commands through an optional configured
[RTK](https://github.com/rtk-ai/rtk) executable. An unusable rewrite reports
`RTK rewrite degraded to native` and runs the original command unchanged; each
lane's RTK history is isolated under `<git-common>/.spice/rtk/`, and
`spice doctor` checks the configured executable.

On macOS, supervised shells refuse a command that starts a Chromium-family
executable (Chrome, Chrome for Testing, Chromium, Edge, Brave) without
`--use-mock-keychain`, reporting `browser::keychain-unmocked`. Without that
switch Chromium reads its profile key from the login Keychain, and an
untrusted build makes macOS ask the operator for their password. Playwright
and Puppeteer pass the switch themselves.

## Develop

[DEVELOPMENT.md](DEVELOPMENT.md) owns the development loop, crate seams, and
cleanup; [Gate evidence](docs/product/planes/gate-evidence.md) owns commit
proof; [SHELL.md](SHELL.md) inventories every tracked shell file;
[TASTES.md](TASTES.md) owns taste curation.
