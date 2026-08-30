// Shared resident commands and browser waits for the sole Canvas entrypoint.
// Protocol records and focused scenarios live in their own fixture modules.
import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { once } from "node:events";
import { unwatchFile, watchFile } from "node:fs";
import {
  chmod,
  cp,
  mkdir,
  readFile,
  readdir,
  realpath,
  writeFile,
} from "node:fs/promises";
import { dirname, join } from "node:path";
import { promisify } from "node:util";

import {
  HANDSHAKE_TIMEOUT_MS,
  RECONNECT_BASE_MS,
} from "./client-deadlines.mjs";
import {
  binary,
  buildResident,
  prebuiltBinary,
  root,
} from "./canvas-build.mjs";
import {
  TOOL_ACTIVITY_TEXT,
  compactionActivityRecord,
  sessionRecord,
  toolActivityRecord,
  toolResultRecord,
  triggeredActivityRecord,
} from "./canvas-activity-records.mjs";
import { childExited } from "./canvas-child-state.mjs";
import { describeConnection, describeResident } from "./canvas-endpoint-probes.mjs";
import { assertHeldTaskCount as assertHeldTaskCountWith } from "./canvas-held-task.mjs";
import { describeLoad, LOAD_FIELD } from "./canvas-load.mjs";

export { buildResident, describeConnection, describeResident, TOOL_ACTIVITY_TEXT };
export { monitorPage } from "./canvas-monitor.mjs";

/// File in which a fixture's vendor stand-ins enrol their own pids.
///
/// Named once because two unrelated things must agree on it: the shell written
/// by `configureFixture`, which appends to it, and the teardown owner, which
/// reads it back to decide exactly which processes this run is responsible for.
export const VENDOR_PID_LEDGER = ".spice-vendor-pids";

const execute = promisify(execFile);
const SHUTDOWN_COMMAND_MS = 25_000;

/// Steering root that causes the isolated Canvas authority fixture to exist.
const CANVAS_SEED_STEERING_ORIGIN = "ack:1kLMtWnl";
/// Durable parent origin learned independently for each isolated fixture.
const taskOrigins = new Map();

export function fixtureCommandEnv(lane) {
  const env = { ...process.env };
  if (lane) env.SPICE_RS_LANE = lane;
  else delete env.SPICE_RS_LANE;
  return env;
}

function waitForDiscovery(child, canvas, timeout = 30_000) {
  return new Promise((resolveDiscovery, rejectDiscovery) => {
    let settled = false;
    let lastError;
    const published = (current, previous) => {
      if (current.ino !== previous.ino || current.size !== previous.size) readPublishedDiscovery();
    };
    watchFile(canvas, { interval: 50 }, published);
    const deadline = setTimeout(() => {
      finish(new Error("timed out waiting for resident canvas discovery", { cause: lastError }));
    }, timeout);
    const residentExit = (code) => {
      finish(new Error(`resident exited ${code} before publishing canvas discovery`));
    };

    function finish(error, discovery) {
      if (settled) return;
      settled = true;
      clearTimeout(deadline);
      unwatchFile(canvas, published);
      child.off("exit", residentExit);
      if (error) rejectDiscovery(error);
      else resolveDiscovery(discovery);
    }

    function readPublishedDiscovery() {
      readFile(canvas, "utf8").then(
        (bytes) => {
          try {
            finish(null, JSON.parse(bytes));
          } catch (error) {
            finish(new Error("resident canvas discovery is malformed", { cause: error }));
          }
        },
        (error) => {
          lastError = error;
          if (error.code !== "ENOENT") finish(error);
        },
      );
    }

    child.once("exit", residentExit);
    readPublishedDiscovery();
  });
}

export async function startResident(fixture) {
  const canvas = join(fixture, ".git/.spice-rs/canvas");
  await mkdir(dirname(canvas), { recursive: true });
  const child = spawn(binary, ["--json", "up", "--foreground", "--port", "0"], {
    cwd: fixture,
    env: fixtureCommandEnv(),
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk) => { stdout += chunk.toString(); });
  child.stderr.on("data", (chunk) => { stderr += chunk.toString(); });
  child.on("exit", (code) => {
    if (code && code !== 0) process.stderr.write(`resident exited ${code}: ${stderr}\n`);
  });
  try {
    const discovery = await waitForDiscovery(child, canvas);
    await waitForResidentReady(fixture, child);
    return { child, discovery };
  } catch (error) {
    let residue;
    try {
      residue = await readdir(fixture, { recursive: true });
    } catch (readError) {
      residue = [`<unreadable: ${readError}>`];
    }
    if (!childExited(child)) {
      const exited = once(child, "exit");
      child.kill("SIGKILL");
      await exited;
    }
    throw new Error(`resident did not publish discovery: stdout=${stdout || "<none>"} stderr=${stderr || "<none>"} files=${JSON.stringify(residue)}`, {
      cause: error,
    });
  }
}

async function waitForResidentReady(fixture, child, timeout = 30_000) {
  const deadline = Date.now() + timeout;
  let lastOutcome = null;
  while (Date.now() < deadline && !childExited(child)) {
    const outcome = await invoke(fixture, ["journal"]);
    if (outcome.kind === "reply") return;
    lastOutcome = outcome;
    if (!isStartupTransient(outcome)) {
      throw new Error(`resident readiness command returned ${JSON.stringify(outcome)}`);
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(
    `resident did not answer readiness command before startup bound: `
      + `status=${child.exitCode ?? child.signalCode ?? "running"} `
      + `last=${JSON.stringify(lastOutcome)}`,
  );
}

function isStartupTransient(outcome) {
  return outcome.kind === "fault"
    && outcome.class === "environment"
    && [
      "canvas::deadline-elapsed",
      "canvas::publisher-replaced",
      "canvas::publisher-gone",
      "canvas::discovery-unavailable",
    ].includes(outcome.reason);
}

function decodeOutcome(stdout, stderr = "") {
  const text = stdout.trim() || stderr.trim();
  assert.ok(text, "spice command returns one JSON outcome");
  return JSON.parse(text);
}

async function invoke(fixture, args, lane) {
  try {
    const result = await execute(binary, ["--json", ...args], {
      cwd: fixture,
      timeout: 20_000,
      env: fixtureCommandEnv(lane),
    });
    return decodeOutcome(result.stdout, result.stderr);
  } catch (error) {
    if (typeof error?.stdout === "string" || typeof error?.stderr === "string") {
      return decodeOutcome(error.stdout ?? "", error.stderr ?? "");
    }
    throw error;
  }
}

function withTaskOrigin(fixture, args) {
  if (args[0] !== "task" || args[1] !== "add" || args.includes("--origin")) return args;
  const origin = taskOrigins.get(fixture);
  assert.ok(origin, `task add before seedAuthority supplied a parent for ${fixture}`);
  return [...args, "--origin", origin];
}

export async function reply(fixture, args) {
  const command = withTaskOrigin(fixture, args);
  const outcome = await invoke(fixture, command);
  assert.equal(outcome.kind, "reply", `${command.join(" ")} replies: ${JSON.stringify(outcome)}`);
  return outcome;
}

// Run a fixture command under a seated lane's own identity.
//
// A claim records the lane it was won by, read from the Rust-specific
// SPICE_RS_LANE handoff (`ambient_lane_identity` in
// spice/src/command_client.rs), and the watchband joins a task to a member
// through exactly that edge (`held_task` in app/src/watchband.rs). So a claim
// won by the operator never reaches a rendered standing, and this is what lets
// a fixture put a real held handle on a card the browser is looking at.
export async function replyAsLane(fixture, lane, args) {
  const command = withTaskOrigin(fixture, args);
  const outcome = await invoke(fixture, command, lane);
  assert.equal(
    outcome.kind,
    "reply",
    `${command.join(" ")} as ${lane}: ${JSON.stringify(outcome)}`,
  );
  return outcome;
}

export function assertHeldTaskCount(team, fixture, lane) {
  return assertHeldTaskCountWith(team, fixture, lane, replyAsLane);
}

export async function configureFixture(
  fixture, linkedRoot, memberCount = 5, { noCardMember = null } = {},
) {
  assert.ok(
    noCardMember === null
      || (Number.isInteger(noCardMember) && noCardMember >= 2 && noCardMember <= memberCount),
    "a no-card fixture member names one linked worktree",
  );
  const noCardWorktree = noCardMember === null ? "" : `lane-${noCardMember}`;
  const triggeredActivity = JSON.parse(triggeredActivityRecord);
  delete triggeredActivity.timestamp;
  const triggeredActivityTail = JSON.stringify(triggeredActivity).slice(1);
  await execute("git", ["init", "--quiet"], { cwd: fixture });
  await writeFile(join(fixture, "Cargo.toml"), [
    "[package]",
    'name = "canvas-fixture"',
    'version = "0.0.0"',
    'edition = "2024"',
    "",
  ].join("\n"));
  await writeFile(join(fixture, "Cargo.lock"), [
    "version = 4",
    "",
    "[[package]]",
    'name = "canvas-fixture"',
    'version = "0.0.0"',
    "",
  ].join("\n"));
  await cp(join(root, "rust-toolchain.toml"), join(fixture, "rust-toolchain.toml"));
  await mkdir(join(fixture, "dist"), { recursive: true });
  await cp(join(root, "dist/stage-app"), join(fixture, "dist/stage-app"), { recursive: true });
  const vendor = join(fixture, "fake-claude");
  await writeFile(vendor, [
    "#!/bin/sh",
    "trap 'exit 0' TERM INT",
    // Each stand-in enrols its own pid in the ledger of the fixture that wrote
    // this script, derived from $0 so it can never enrol in another one.
    "vendors=${0%/*}/" + VENDOR_PID_LEDGER,
    'printf \'%s\\n\' "$$" >> "$vendors"',
    "name=${PWD##*/}",
    `printf '%s\\n' '${sessionRecord}'`,
    `if [ "$name" != "${noCardWorktree}" ]; then`,
    `  ${concatAgentRecord()}`,
    "fi",
    "while :; do",
    "  if [ -f .spice-tool-trigger ]; then",
    "    rm -f .spice-tool-trigger",
    `    printf '%s\\n' '${toolActivityRecord}'`,
    "  fi",
    "  if [ -f .spice-tool-result-trigger ]; then",
    "    rm -f .spice-tool-result-trigger",
    `    printf '%s\\n' '${toolResultRecord}'`,
    "  fi",
    "  if [ -f .spice-prose-trigger ]; then",
    "    rm -f .spice-prose-trigger",
    "    timestamp=$(date -u '+%Y-%m-%dT%H:%M:%S.000Z')",
    `    printf '{"timestamp":"%s",%s\\n' "$timestamp" '${triggeredActivityTail}'`,
    "  fi",
    "  if [ -f .spice-compaction-trigger ]; then",
    "    rm -f .spice-compaction-trigger",
    `    printf '%s\\n' '${compactionActivityRecord}'`,
    "  fi",
    "  sleep 1",
    "done",
    "",
  ].join("\n"));
  await chmod(vendor, 0o700);
  await writeFile(join(fixture, "spice-rust.toml"), [
    "[consent]",
    'posture = "queue"',
    "",
    "[[consent.rule]]",
    'name = "browser"',
    'tool = "shell:*"',
    'answer = "allow"',
    "budget = 2",
    "",
    "[lane]",
    'lease = "5s"',
    'vendor = "claude"',
    "",
    "[tasks]",
    'stems = ["app", "canvas", "dist", "gate", "spice", "steer"]',
    "",
    "[vendor.claude]",
    `binary = ${JSON.stringify(vendor)}`,
    "",
    "[canvas.pack]",
    "rows = 2",
    "",
  ].join("\n"));
  await writeFile(
    join(fixture, ".gitignore"),
    ["dist/stage-app", ".spice-tool-trigger", ".spice-prose-trigger", VENDOR_PID_LEDGER, ""]
      .join("\n"),
  );
  await execute(
    "git",
    [
      "add",
      ".gitignore",
      "Cargo.lock",
      "Cargo.toml",
      "fake-claude",
      "rust-toolchain.toml",
      "spice-rust.toml",
    ],
    { cwd: fixture },
  );
  await execute("git", [
    "-c", "user.name=Canvas Fixture",
    "-c", "user.email=canvas@example.invalid",
    "commit", "--quiet", "-m", "canvas fixture",
  ], { cwd: fixture });
  const skill = join(fixture, ".git/.spice-install/agents/skills/spice/SKILL.md");
  await mkdir(dirname(skill), { recursive: true });
  await cp(join(root, "spice/agent/SKILL.md"), skill);
  const worktrees = [fixture];
  for (let index = 2; index <= memberCount; index += 1) {
    const worktree = join(linkedRoot, `lane-${index}`);
    await execute("git", ["worktree", "add", "--quiet", "--detach", worktree], { cwd: fixture });
    worktrees.push(await realpath(worktree));
  }
  return worktrees;
}

function concatAgentRecord() {
  const prefix = "{\"type\":\"assistant\",\"message\":{\"type\":\"message\","
    + "\"role\":\"assistant\",\"content\":[{\"type\":\"text\","
    + "\"text\":\"Canvas message reports CONTROL-1kKbC3j4 from ";
  const suffix = "\"}],\"usage\":{\"input_tokens\":1,\"cache_read_input_tokens\":0,"
    + "\"cache_creation_input_tokens\":0,\"output_tokens\":1}}}";
  return `printf '%s%s%s\\n' '${prefix}' "$name" '${suffix}'`;
}

export async function journalExtent(fixture) {
  const report = await reply(fixture, ["journal"]);
  assert.ok(report.body.cursor, "running authority has a journal cursor");
  return {
    cursor: Number(report.body.cursor),
    facts: Number(report.body.facts),
  };
}

// Re-declare the vendor section so later launches carry a different identity.
//
// `[vendor.claude]` in this fixture names only a binary, so the model and effort
// a launch records fall to their builtins and every occupant wears one identity.
// Configuration is resolved per launch rather than held from resident startup --
// `lane_surface/launch.rs` calls `config::load_repository` on the way in -- so a
// section rewritten between two launches is what makes the second one differ.
export async function declareVendorModel(fixture, model) {
  const path = join(fixture, "spice-rust.toml");
  const text = await readFile(path, "utf8");
  const cleared = text.replace(/^model = .*\n/m, "");
  const declared = cleared.replace(
    /^\[vendor\.claude\]\n/m,
    `[vendor.claude]\nmodel = ${JSON.stringify(model)}\n`,
  );
  assert.notEqual(declared, cleared, "the fixture's vendor section admits a model");
  await writeFile(path, declared);
  // Committed rather than left in the tree, because the phase this rewrite spans
  // is closed by `task done`, which refuses a dirty worktree with
  // `git::worktree-dirty`. Leaving the rewrite uncommitted would end the seeding
  // on that refusal instead of on the identity it exists to move.
  await execute("git", ["add", "spice-rust.toml"], { cwd: fixture });
  await execute("git", [
    "-c", "user.name=Canvas Fixture",
    "-c", "user.email=canvas@example.invalid",
    "commit", "--quiet", "-m", `canvas fixture launches ${model}`,
  ], { cwd: fixture });
}

// Seed one task tree whose causal root is operator steering.
//
// The first task carries the steering root explicitly because the isolated
// operator holds no claim. Every later fixture task crosses the shared command
// helper, which supplies the durable first task as its parent.
export async function seedAuthority(fixture) {
  const pending = await reply(fixture, ["trust"]);
  const digest = pending.body.digest;
  assert.equal(pending.body.standing, "pended");
  await reply(fixture, ["trust", "grant", digest]);

  const first = await reply(fixture, [
    "task",
    "add",
    "Browser fixture task",
    "--project",
    "app.browser",
    "--origin",
    CANVAS_SEED_STEERING_ORIGIN,
  ]);
  taskOrigins.set(fixture, `task:${first.body.task}`);
  await reply(fixture, [
    "task",
    "add",
    "Continuation witness",
    "--project",
    "canvas.browser",
  ]);
  for (const [title, project] of [
    ["Distribution witness", "dist.browser"],
    ["Gate witness", "gate.browser"],
    ["Spice witness", "spice.browser"],
    ["Steering witness", "steer.browser"],
  ]) {
    await reply(fixture, ["task", "add", title, "--project", project]);
  }
  await reply(fixture, ["task", "note", first.body.task, "first causal observation"]);
  await reply(fixture, ["task", "note", first.body.task, "second causal observation"]);

  return {
    task: first.body.task,
    extent: await journalExtent(fixture),
  };
}

// Switch the canvas to a named view, which several waits treat as a precondition.
//
// BROWSER-1kKZ9jJs traced why a `.status.offline` wait can spend its whole
// bound. The connection flip itself is not the slow part and is not polled:
// `app/src/browser.rs` `run_surface` sets `Connection::Offline` the instant
// `run_connection` returns, and `run_connection` returns as soon as the socket
// reader yields nothing, mapped to "resident closed the socket". No interval
// sits between the close and the flip, and `stopResident` below awaits the
// resident process exit, so the close has already happened before any such
// wait is armed.
//
// The client has since gained a liveness exchange, which does not change that.
// It is what *produces* an end for a socket that would otherwise never yield --
// it cannot delay one that already yielded. So a wait that follows a real close
// is still bounded by `TRANSITION_MS`. The half-open browser proof advances
// the client clock across its two probe intervals instead of spending them on
// the host.
//
// What is not settled before the wait is armed is *this* function. The status
// element is not part of a permanently rendered chrome: `app/src/browser.rs`
// starts `mosaic` at `true` and `app/src/browser/view.rs` renders the topbar
// holding `.status` only under `if !**render.mosaic`, so `.status` is absent
// from the document in the default Agents view. Every `.status` wait in
// `canvas.spec.mjs` is therefore preceded by a switch to a non-mosaic view,
// and each one is really waiting on two things: the connection state, and this
// click having been applied. This function returns once the click is
// dispatched; nothing here confirms the view actually changed. A wait that
// times out consequently cannot distinguish "`.status` never carried
// `offline`" from "`.status` was never in the document at all", and only the
// second is a view-switch failure. That is demonstrated, not supposed: pointing
// one such wait at the mosaic "Agents" view instead reproduces the reported
// flake's signature exactly, down to the same bare uncaught TimeoutError on
// `waiting for locator('.status.offline') to be visible`, from a cause that
// never involved the connection at all.
//
// Keying this on the transition it triggers, and reporting which of the two
// cases a timeout hit, is BROWSER-1kKZDVGZ and BROWSER-1kKZDKpT respectively.
/// Bound for a DOM transition the page has already been handed the input for.
///
/// Justified against a measured distribution rather than chosen to outlast an
/// observed failure. `app/browser/offline-cycle.mjs` drives the smallest cycle
/// that reproduces the reported flake -- start the resident, open the page,
/// choose the view, stop the resident, wait for offline -- and 120 runs on a
/// 12-core host at load average 13 to 16 put the offline flip at 2ms minimum,
/// 4ms median, 19ms maximum and the companion fault event at 7ms minimum, 11ms
/// median, 32ms maximum. Seventy of those runs were held to a 300ms bound and
/// none failed. Two seconds is roughly a hundred times the slowest observation,
/// and the margin survives a loaded machine: at load average 124 the flip still
/// lands in 78ms. A timeout here is a defect or a tail event, not a slow box.
export const TRANSITION_MS = 2_000;

/// Slack allowed on top of a client deadline for the render that follows it.
///
/// The bounds below are sums rather than numbers so that no wait can end up
/// stricter than the deadline the client keeps for itself, however that deadline
/// moves. This is the only part of them that is picked rather than read, and the
/// measurements say it is never spent.
const RENDER_SLACK_MS = 1_000;

/// Bound for a wait on the page connecting, which it has not been handed yet.
///
/// Coming online is not the same class as the transitions above and must not
/// borrow their bound. `TRANSITION_MS` is justified by the page already holding
/// the input, so the only cost left is a render; a connection instead has to
/// complete a handshake the client itself puts a deadline on. A test bound under
/// that deadline would expire before the client gave up, so its red would mean
/// the suite is stricter than the contract rather than that anything is broken --
/// which is why this is the deadline itself plus a render, read from the client
/// rather than copied out of it. The measurement says the slack is never spent:
/// 25 cycles at load average 8 to 9 put a cold connect after `networkidle` at 1ms
/// minimum, 2ms median and 3ms maximum, because the socket is already up by the
/// time that wait arms.
export const CONNECT_MS = HANDSHAKE_TIMEOUT_MS + RENDER_SLACK_MS;

/// Bound for a wait on the page reconnecting after its socket closed.
///
/// This one has a floor written into the client, which sleeps at least
/// `RECONNECT_BASE_MS` before retrying, so no reconnect can be observed sooner
/// and any bound near the offline flip's 2ms would fail every single time. The
/// base rather than the ceiling is the right floor here only because every
/// caller reaches this after an *admitted* connection ended, which is exactly
/// where the client resets its ladder; a wait placed after a failed attempt
/// would be entitled to the ladder's ceiling and would have to say so. The sum
/// is the longest a single successful retry can take: the delay, one whole
/// handshake attempt at its own deadline, and the render.
///
/// The measurement below predates the ladder, when the delay was a flat 1200ms:
/// 1283ms minimum, 1287ms median, 1290ms maximum over 25 cycles, that delay plus
/// 83ms to 90ms of real handshake. Only the delay term changed, so the handshake
/// and render those runs measured still stand and the observation is now
/// expected 700ms sooner rather than invalidated.
export const RECONNECT_MS = RECONNECT_BASE_MS + HANDSHAKE_TIMEOUT_MS + RENDER_SLACK_MS;

export async function chooseCanvasView(page, name) {
  const button = page.getByRole("button", { name: new RegExp(`^${name}(?:\\s|$)`) });
  if (!(await button.isVisible())) {
    await page.getByLabel("Open Spice Canvas menu").click();
  }
  await button.click();
  const openMenu = page.locator(".canvas-menu[open]");
  if (await openMenu.count()) {
    await openMenu.locator("summary").click();
  }
  // Returning here would return on dispatch, which is what made every later
  // wait ambiguous: a selector wait that follows an unapplied switch is deaf
  // rather than slow, and both spell the same bare TimeoutError. Every view
  // confirms the same way, on the pill the switch selects: `render_pill` in
  // app/src/browser/view/chrome.rs sets `aria-current` from the state the
  // click just wrote, so the marker appears only once the switch is applied.
  //
  // Agents was special-cased onto `.mosaic-workspace` on the reasoning that
  // its pill sits inside the menu this function has just closed and so can
  // never be visible. That was wrong twice over: `render_canvas_menu` holds
  // only Magnitude and Flow, and the mosaic bar renders its own Agents pill
  // into `nav.canvas-shortcuts`, a sibling of the menu that team-menu.mjs
  // already reads the text and bounding rect of. The branch is gone because
  // the marker was measured, not argued: with it removed the suite passes,
  // and at every Agents switch `.mosaic-workspace` was already present the
  // moment the marker settled, so the marker never lands the earlier.
  const applied = button.and(page.locator('[aria-current="page"]'));
  await applied.waitFor({ timeout: TRANSITION_MS });
}

export async function stopResident(fixture, resident) {
  const exited = !childExited(resident.child)
    ? once(resident.child, "exit")
    : Promise.resolve();
  await execute(binary, ["--json", "down"], {
    cwd: fixture,
    env: fixtureCommandEnv(),
    timeout: SHUTDOWN_COMMAND_MS,
  });
  await exited;
}


/// Name the frame that armed the wait, so a shared selector is still locatable.
const waitWrapperNames = [
  "observedWait",
  "observedBoundedWait",
  "waitForPageFunction",
  "waitForLocator",
];

function callSite(marker) {
  const frames = (marker.stack ?? "").split("\n").slice(1);
  const caller = frames.find((frame) => {
    return !waitWrapperNames.some((wrapper) => frame.includes(`${wrapper} (`));
  });
  return caller?.trim() ?? "<call site unavailable>";
}

/// Run a wait and, when it exceeds its bound, replace the bare timeout with a
/// report naming the wait and the state both sides were in.
///
/// The suite is one script, so an unenriched rejection reaches the top level
/// naming only a selector. Several call sites share a selector, and the raw
/// timeout says nothing about whether the page or the resident was at fault,
/// which is exactly the distinction a load-sensitive failure needs.
///
/// `context.observed` is how a call site adds the one reading its own wait is
/// about. An expired bound says only that a predicate stayed false; the value it
/// stayed false against is usually the diagnosis, and it is gone by the time the
/// rejection reaches the top level.
export async function observedWait(page, label, run, context = {}) {
  const marker = new Error(label);
  try {
    return await run();
  } catch (error) {
    const connection = await describeConnection(page);
    const resident = await describeResident(context.resident);
    const report = [
      `canvas wait failed: ${label}`,
      `  site: ${callSite(marker)}`,
      `  connection: ${JSON.stringify(connection)}`,
      `  resident: ${JSON.stringify(resident)}`,
      `${LOAD_FIELD}${JSON.stringify(describeLoad())}`,
    ];
    if (context.observed) {
      report.push(`  observed: ${JSON.stringify(await readObservation(context.observed))}`);
    }
    throw new Error(report.join("\n"), { cause: error });
  }
}

/// Read what the wait stayed false against, without letting that read take over.
///
/// It runs only after the wait has already failed, so it cannot be why the wait
/// failed. Its own failure is reported in place rather than raised: a page that
/// closed underneath the wait is itself the answer, and a second error thrown
/// here would replace the report that found it with one naming only the read.
async function readObservation(observe) {
  try {
    return await observe();
  } catch (error) {
    return `unreadable: ${error.message}`;
  }
}

/// Prove the instrument reports instead of regressing to a bare timeout.
///
/// Drives a wait that cannot ever be satisfied and asserts the failure names the
/// wait, locates its call site, and carries both descriptions. Without this a
/// future edit could drop the enrichment and restore an unattributable timeout
/// with every other test still green.
///
/// Returns the connection state the report carried, so a caller running this at
/// two layouts can prove the report tells a hidden status from a rendered one
/// instead of only ever seeing whichever the default happens to be.
export async function assertWaitReporting(page, resident) {
  const label = "instrument self-check";
  const started = performance.now();
  const failure = await waitForPageFunction(
    page,
    label,
    WAIT_PROBE_MS,
    () => false,
    undefined,
    { resident },
  ).then(() => null, (error) => error);
  const elapsed = performance.now() - started;
  assert.ok(failure, "a wait that cannot be satisfied must fail");
  assert.match(failure.message, /canvas wait failed: instrument self-check/);
  assert.match(
    failure.message,
    /site: at assertWaitReporting \(file:.*canvas-fixture\.mjs:\d+:\d+\)/,
    "the report names the self-check that armed the wait rather than an instrument wrapper",
  );
  assert.match(failure.message, /"endpointState":/, "the report probes the resident endpoint");
  // A dropped load field fails assertBoundaryLoadReading first; neutralise it to reach these two.
  assert.match(failure.message, /load: \{"loadAverage":\d[^}]*"cores":\d/, "the load report names cores");
  assert.match(failure.message, /load: \{[^}]*"factor":\d/, "the load report names factor");
  assert.ok(failure.cause, "the original timeout survives as the cause");
  assert.ok(elapsed >= WAIT_PROBE_MS, "the page-function probe reaches its named bound");
  assert.ok(
    elapsed < WAIT_PROBE_MS + TRANSITION_MS,
    "the page-function probe never falls through to Playwright's default",
  );

  for (const invalid of [undefined, 0]) {
    const refusal = await waitForPageFunction(page, "invalid-bound self-check", invalid, () => true)
      .then(() => null, (error) => error);
    assert.ok(refusal instanceof TypeError, `the ${String(invalid)} bound is refused`);
  }

  // Both halves of the observation are proven here, because each fails on its
  // own. A readable one has to survive into the report verbatim, or a call site
  // that names its reading still ships a bare timeout. An unreadable one has to
  // be noted in place, or the read that cannot run replaces the report that says
  // what could not be waited for -- and a closed page, the likeliest reason the
  // read fails, is exactly when that report matters most.
  const carried = await waitForPageFunction(
    page,
    "observation self-check",
    WAIT_PROBE_MS,
    () => false,
    undefined,
    { resident, observed: () => "dangling at 14" },
  ).then(() => null, (error) => error);
  assert.ok(carried, "a wait that cannot be satisfied must fail");
  assert.equal(
    reportField(carried.message, "observed"),
    '"dangling at 14"',
    "the report carries the reading its call site named, not only that the bound expired",
  );

  const unreadable = await waitForPageFunction(
    page,
    "unreadable-observation self-check",
    WAIT_PROBE_MS,
    () => false,
    undefined,
    { resident, observed: () => { throw new Error("target page has been closed"); } },
  ).then(() => null, (error) => error);
  assert.match(
    unreadable.message,
    /canvas wait failed: unreadable-observation self-check/,
    "an observation that cannot be read leaves the wait's own report intact",
  );
  assert.match(
    reportField(unreadable.message, "observed"),
    /unreadable: target page has been closed/,
    "an observation that cannot be read is reported in place rather than raised",
  );

  // The connection state is asserted by shape rather than by one literal, because
  // both shapes are real and each is a different diagnosis. Reading it back out
  // of the rendered message proves the report carries it, not merely that the
  // helper could compute it.
  const connection = JSON.parse(reportField(failure.message, "connection"));
  assert.ok("rendered" in connection, "the report says whether the status rendered");
  if (connection.rendered) {
    assert.match(connection.classes, /^status/, "a rendered status names Online against Offline");
  } else {
    assert.ok("mosaic" in connection, "an absent status reports the layout that hides it");
  }
  return connection;
}

/// Run the self-check at a second layout and prove the two reports disagree.
///
/// The workspace starts as a mosaic, which renders no topbar and therefore no
/// status element at all, so a wait on a status class is deaf until a single
/// view is chosen. `hidden` is the report taken at the mosaic; taking another
/// once a view is open proves the instrument separates a status the layout
/// hides from one that rendered and never changed, which are opposite
/// diagnoses for the same timeout.
export async function assertWaitReportDistinguishesLayout(page, resident, hidden) {
  const shown = await assertWaitReporting(page, resident);
  assert.notEqual(
    hidden.rendered,
    shown.rendered,
    "the report tells a status hidden by the layout from one that rendered",
  );
}

/// A restarted resident republishes a generation and the canvas rejoins it.
///
/// Returns the restarted resident. The rejoin waits report through the
/// instrument for the same reason the loss waits do: a bare timeout here says
/// only that a selector never appeared, and cannot tell a client that rejoined
/// a stale generation from one that never rejoined at all.
export async function assertResidentRestart(fixture, page, previous) {
  const resident = await startResident(fixture);
  assert.notEqual(resident.discovery.epoch, previous.epoch, "restart publishes a new generation");
  assert.notEqual(
    resident.discovery.capability,
    previous.capability,
    "restart publishes a new operator capability",
  );
  const restarted = await reply(fixture, ["lane"]);
  assert.equal(
    restarted.body.subscriptions.find(
      (subscription) => subscription.lineage_id === previous.policy.lineage,
    )?.policy,
    previous.policy.policy,
    "restart preserves the configured task policy instead of deriving it from lane life",
  );
  await page.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(page, "Chronology");
  // This is a cold connect, not a reconnect: the line above navigates afresh and
  // waits for `networkidle`, so the client is not sitting out a retry delay
  // and the page has a live socket before this wait arms. Bounding it as a
  // reconnect would hide a genuinely stalled restart behind the retry delay.
  await assertGenerationOnline(
    page,
    resident.discovery.epoch,
    "the resident restarts",
    CONNECT_MS,
    { resident },
  );
  return resident;
}

/// Wait for the canvas to report offline, on the one bound that was measured.
///
/// Four call sites share this: the resident stopping, a stale generation being
/// refused, the fixture closing the socket from inside the page, and a malformed
/// bootstrap being rejected. All four are the same class of transition -- the
/// page has already been handed the input by the time the wait arms -- so all
/// four take `TRANSITION_MS` rather than a bound chosen per site to outlast
/// whatever last went wrong there. Coming back is a different class and takes
/// `CONNECT_MS` or `RECONNECT_MS` through the helpers below, because a wait that
/// has to sit through a handshake cannot be bounded by one that does not.
/// Every one of them reports through the instrument, because a bare
/// `TimeoutError` on `.status.offline` cannot say whether the page never
/// processed the close or the status was never on screen to carry the class.
export async function assertOfflineObserved(page, cause, context = {}) {
  await observedWait(
    page,
    `canvas reports offline after ${cause}`,
    () => page.waitForSelector(".status.offline", { timeout: TRANSITION_MS }),
    context,
  );
}

/// Refuse a wait whose bound was never named, instead of taking the worst one.
///
/// Playwright reads a missing `timeout` as its own 30000ms default, so omitting
/// the argument does not fail -- it silently selects the largest bound in the
/// file, which is the exact default these helpers exist to abolish. A helper
/// whose whole point is that the bound is stated at the call site cannot leave
/// the bound optional in practice, so an unnamed one is an error here rather
/// than a default there. Zero is refused with it, and not for tidiness:
/// Playwright reads a zero timeout as no timeout at all, so the one value that
/// looks like the strictest possible bound is the only one weaker than the
/// default it replaces.
function requireBound(bound, helper) {
  if (!Number.isFinite(bound) || bound <= 0) {
    throw new TypeError(
      `${helper} needs a positive bound: pass CONNECT_MS for a cold connect, RECONNECT_MS for a live one`,
    );
  }
}

/// Bound used only by the executable never-satisfied instrumentation probe.
export const WAIT_PROBE_MS = 250;
const reportBrowserWaits = process.env.SPICE_BROWSER_WAIT_REPORT === "1";

/// Run one browser wait with a positive bound, actionable failure, and receipt.
export async function observedBoundedWait(page, label, bound, run, context = {}) {
  requireBound(bound, "observedBoundedWait");
  const started = performance.now();
  try {
    return await observedWait(page, label, run, context);
  } finally {
    if (reportBrowserWaits) {
      process.stdout.write(`${JSON.stringify({
        browserWait: label,
        elapsedMs: Math.round(performance.now() - started),
        boundMs: bound,
      })}\n`);
    }
  }
}

/// Wait for a page predicate without permitting Playwright to choose the bound.
export async function waitForPageFunction(
  page,
  label,
  bound,
  predicate,
  argument,
  context = {},
) {
  return observedBoundedWait(
    page,
    label,
    bound,
    () => page.waitForFunction(predicate, argument, { timeout: bound }),
    context,
  );
}

/// Wait for a locator with the same bound enforcement and elapsed receipt.
export async function waitForLocator(page, label, bound, locator, options = {}) {
  return observedBoundedWait(
    page,
    label,
    bound,
    () => locator.waitFor({ ...options, timeout: bound }),
  );
}

/// Wait for the canvas to report online, on the bound its class was measured at.
///
/// The bound is an argument rather than a default because the two classes are
/// three orders of magnitude apart -- a cold connect lands in 2ms and a live
/// reconnect cannot land before 1200ms -- so a helper that picked one for the
/// caller would silently hand every site the wrong one. Naming `CONNECT_MS` or
/// `RECONNECT_MS` at the call site is the whole point: it says which transition
/// the site believes it is waiting on, and that belief is now checkable.
export async function assertOnlineObserved(page, cause, bound, context = {}) {
  requireBound(bound, "assertOnlineObserved");
  await observedWait(
    page,
    `canvas reports online after ${cause}`,
    () => page.waitForSelector(".status.online", { timeout: bound }),
    context,
  );
}

/// Wait for the canvas to come online and adopt the generation it was given.
///
/// Online is not enough on its own: a socket can be up against the generation
/// the page was already carrying, so the status class alone cannot tell a resumed
/// connection from an adopted one. Both waits are one helper because both call
/// sites need both, and neither is meaningful without the other -- a page that is
/// online but still showing the old epoch has not resumed, and an epoch that
/// appears without the status coming online is not a connection.
///
/// The adoption wait takes the same bound as the connection wait rather than
/// `TRANSITION_MS`: the generation is published over the socket the wait above
/// just watched come up, so it is the same class of transition and not the
/// already-delivered one. `waitForFunction` carries its timeout in a third
/// argument, which is easy to leave off and worth 30000ms when it is.
export async function assertGenerationOnline(page, epoch, cause, bound, context = {}) {
  requireBound(bound, "assertGenerationOnline");
  await assertOnlineObserved(page, cause, bound, context);
  await waitForPageFunction(
    page,
    `canvas adopts the generation published when ${cause}`,
    bound,
    (expected) => document.querySelector(".status-copy")?.textContent?.includes(expected),
    epoch,
    context,
  );
}

/// The canvas must notice the resident is gone.
///
/// The status flips Offline and a fault lands in the event stream. Both waits
/// report through the instrument, so a timeout names which of the two failed
/// and what the client and the resident looked like when it did, rather than
/// killing the node process with a bare TimeoutError.
export async function assertResidentLossObserved(page, resident) {
  // Both waits below are gated on the same layout: `view.rs` renders the topbar
  // carrying `.status` only outside the mosaic, and the analytics pane carrying
  // `.event.fault` is the else branch of that same test. Waiting for the gate
  // first means a wrong view fails saying the status never rendered, instead of
  // spending the whole bound waiting for a class on an element that is not in
  // the document and then blaming the connection for it.
  await observedWait(
    page,
    "the status element is rendered to be waited on",
    () => page.waitForSelector(".status", { timeout: TRANSITION_MS }),
    { resident },
  );
  await assertOfflineObserved(page, "the resident stops", { resident });
  await observedWait(
    page,
    "a fault event is recorded after the resident stops",
    () => page
      .locator(".event.fault")
      .filter({ hasText: "Canvas connection reset" })
      .waitFor({ timeout: TRANSITION_MS }),
    { resident },
  );
}

function reportField(message, name) {
  const line = message.split("\n").find((entry) => entry.startsWith(`  ${name}: `));
  assert.ok(line, `the report carries the ${name} state`);
  return line.slice(`  ${name}: `.length);
}
