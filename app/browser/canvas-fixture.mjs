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
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import { HANDSHAKE_TIMEOUT_MS, RECONNECT_DELAY_MS } from "./client-deadlines.mjs";
import {
  TOOL_ACTIVITY_TEXT,
  activityRecord,
  sessionRecord,
  toolActivityRecord,
  toolCallId,
  toolResultRecord,
  triggeredActivityRecord,
} from "./canvas-activity-records.mjs";
import { childExited } from "./canvas-child-state.mjs";
import { describeConnection, describeResident } from "./canvas-endpoint-probes.mjs";

export { describeConnection, describeResident, TOOL_ACTIVITY_TEXT };

/// File in which a fixture's vendor stand-ins enrol their own pids.
///
/// Named once because two unrelated things must agree on it: the shell written
/// by `configureFixture`, which appends to it, and the teardown owner, which
/// reads it back to decide exactly which processes this run is responsible for.
export const VENDOR_PID_LEDGER = ".spice-vendor-pids";

const execute = promisify(execFile);
const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const binary = join(root, "target/debug/spice");

export async function buildResident() {
  try {
    await execute("cargo", ["xtask", "app"], {
      cwd: root,
      maxBuffer: 32 * 1024 * 1024,
    });
    await execute("cargo", ["build", "-p", "spice", "--bin", "spice"], {
      cwd: root,
      maxBuffer: 16 * 1024 * 1024,
    });
  } catch (error) {
    const stdout = error?.stdout ?? "<none>";
    const stderr = error?.stderr ?? "<none>";
    throw new Error(
      `canvas spec could not build its resident: stdout=${stdout} stderr=${stderr}`,
      { cause: error },
    );
  }
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
  const canvas = join(fixture, ".git/.spice/canvas");
  await mkdir(dirname(canvas), { recursive: true });
  const child = spawn(binary, ["--json", "up", "--port", "0"], {
    cwd: fixture,
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
      ...(lane ? { env: { ...process.env, SPICE_LANE: lane } } : {}),
    });
    return decodeOutcome(result.stdout, result.stderr);
  } catch (error) {
    if (typeof error?.stdout === "string" || typeof error?.stderr === "string") {
      return decodeOutcome(error.stdout ?? "", error.stderr ?? "");
    }
    throw error;
  }
}

export async function reply(fixture, args) {
  const outcome = await invoke(fixture, args);
  assert.equal(outcome.kind, "reply", `${args.join(" ")} replies: ${JSON.stringify(outcome)}`);
  return outcome;
}

// Run a fixture command under a seated lane's own identity.
//
// A claim records the lane it was won by, read from SPICE_LANE
// (`ambient_lane_identity`, spice/src/command_client.rs:647), and the watchband
// joins a task to a member through exactly that edge (`held_task`,
// app/src/watchband.rs:129). So a claim won by the operator never reaches a
// rendered standing, and this is what lets a fixture put a real held handle on
// a card the browser is looking at.
export async function replyAsLane(fixture, lane, args) {
  const outcome = await invoke(fixture, args, lane);
  assert.equal(outcome.kind, "reply", `${args.join(" ")} as ${lane}: ${JSON.stringify(outcome)}`);
  return outcome;
}

// Observe the one navigation tab the operator reference qualifies by a count.
//
// The reference image shows "Filters 27", and `badge` renders nothing at zero
// (app/src/watchband.rs:205), so a fixture in which no lane holds a task shows
// no badge at all and the count stays unproven no matter how many tabs are
// clicked. `mode_badge` counts standings carrying a handle, so raising it means
// really claiming: this takes the badge from absent to 1 and back, and reads
// the held handle off the card, which is the same third placeholder line the
// reference shows a card wearing.
export async function assertHeldTaskCount(team, fixture, lane) {
  const badge = team.getByRole("tab", { name: /^Filters/ }).locator(".team-mode-badge");
  assert.equal(await badge.count(), 0, "Filters wears no badge while no lane holds a task");
  // Three independent things gate a held task and every one of them refuses as
  // the same `board::allocator-empty`, so each must be satisfied deliberately:
  //
  // 1. the board must carry an eligible task, and `task add` without an
  //    `--acceptance` starts in `plan`, which `workable` never selects;
  // 2. the lineage policy must subscribe to the allocator at all, and `steer`
  //    scopes selection to `Manual` (wire/src/intent.rs:395), which admits only
  //    operator-pinned projects -- this fixture pins none;
  // 3. under `drive` the scope is `ManualAndAutoProject`, and a project is
  //    learned automatically only from facts a *lane* wrote:
  //    `project_subscription_history` skips `Actor::Operator | Actor::Spice`
  //    outright (board/src/next.rs:45). A task the operator adds is therefore
  //    invisible to allocation even while it is otherwise perfectly eligible.
  //
  // So the task is added under the lane's own identity, which both makes it
  // eligible and teaches the lane's lineage the project it lives in.
  const lifetime = team.locator('.team-lifetime input[type="range"]');
  await lifetime.fill("1");
  await team.getByRole("button", { name: "Drive", exact: true }).waitFor();
  await replyAsLane(fixture, lane, [
    "task",
    "add",
    "Held task witness",
    "--project",
    "app.browser",
    "--acceptance",
    "the watchband counts the lane holding this task",
  ]);
  const won = await replyAsLane(fixture, lane, ["task", "next"]);
  // A claim answers with the task and the actor now holding it
  // (`ClaimAnswer`, wire/src/intent.rs:535), and an actor crosses as text under
  // a `lane:` prefix (wire/src/envelope.rs:87). Both are read back off the
  // render below, so this observation fails if the badge counts a standing the
  // claim never reached rather than passing on any task-shaped spelling.
  assert.equal(won.body.holder, `lane:${lane}`, "the claim binds to the lane that made it");
  await badge.waitFor();
  assert.equal(await badge.innerText(), "1", "Filters counts the lane now holding a task");
  const seat = team.locator(`.team-agent[data-agent-lane="${lane}"]`);
  const placeholder = await seat.getByRole("textbox").getAttribute("placeholder");
  const [handle, phase] = placeholder.split("\n").at(-1).split(", ");
  assert.equal(handle, won.body.task, `the held card names the claimed task: ${placeholder}`);
  assert.ok(phase, `the held card names the phase it holds: ${placeholder}`);
  await replyAsLane(fixture, lane, ["task", "free", handle]);
  await badge.waitFor({ state: "detached" });
  await lifetime.fill("0");
  await team.getByRole("button", { name: "Steer", exact: true }).waitFor();

  // The status strip follows one chronology across unlike transcript shapes.
  // The first one-shot fixture record is a real Claude tool_use block decoded
  // by the production capture path, not prose styled to resemble a tool.
  const ticker = team.locator(".ticker");
  await writeFile(join(fixture, ".spice-tool-trigger"), "emit\n");
  const toolLine = `tool: ${TOOL_ACTIVITY_TEXT}`;
  await ticker.filter({ hasText: toolLine }).waitFor({ timeout: 15_000 });
  assertNewestLaneActivity(await ticker.innerText(), toolLine);

  // The result of that same call arrives on a separate transcript record that
  // names only the vendor correlation id. The strip has to resolve it back to
  // the call, because `toolu_01...` is not something an operator can act on.
  await writeFile(join(fixture, ".spice-tool-result-trigger"), "emit\n");
  const resultLine = `${toolLine} succeeded`;
  await ticker.filter({ hasText: resultLine }).waitFor({ timeout: 15_000 });
  assertNewestLaneActivity(await ticker.innerText(), resultLine);

  // Falsification: the exact opaque line this task exists to remove must be
  // rejected by the same assertion the resolved line just passed. It is checked
  // without writing to the page, because overwriting a rendered node here
  // desynchronizes the client's diff and suppresses the next activity update.
  assert.throws(
    () => assertNewestLaneActivity(`tool: ${toolCallId} succeeded`, resultLine),
    /raw vendor tool identifier/,
    "the production assertion rejects a raw vendor identifier on the strip",
  );

  // The fake resident agent consumes this one-shot trigger in its real
  // transcript process. Capture then publishes a normal prose stream frame,
  // which is newer than the tool above and must replace it for the whole team.
  await writeFile(join(fixture, ".spice-prose-trigger"), "emit\n");
  const proseLine = "assistant: newest lane prose witness";
  await ticker.filter({ hasText: proseLine }).waitFor({ timeout: 15_000 });
  assertNewestLaneActivity(await ticker.innerText(), proseLine);

  // Falsification: restore the exact old receipt shape in the production DOM
  // and run the same assertion. It must reject the regression before the
  // observed activity line is restored for the remainder of the fixture.
  const observed = await ticker.innerText();
  await ticker.evaluate((node) => {
    node.textContent = "Lane snapshot: 4 lanes";
  });
  const restoredReceipt = await ticker.innerText();
  assert.equal(
    restoredReceipt,
    "Lane snapshot: 4 lanes",
    "the old receipt is restored in the DOM",
  );
  assert.throws(
    () => assertNewestLaneActivity(restoredReceipt, proseLine),
    /newest lane activity/,
    "the production assertion rejects the former snapshot receipt",
  );
  await ticker.evaluate((node, text) => {
    node.textContent = text;
  }, observed);
  assertNewestLaneActivity(await ticker.innerText(), proseLine);
}

function assertNewestLaneActivity(actual, expected) {
  assert.doesNotMatch(
    actual,
    /(?:Board|Lane) snapshot|Steering inbox/,
    "newest lane activity excludes protocol snapshot receipts",
  );
  assert.doesNotMatch(
    actual,
    /toolu_/,
    "newest lane activity never shows a raw vendor tool identifier",
  );
  assert.equal(actual, expected, "newest lane activity wins across tool and prose shapes");
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
  await execute("git", ["init", "--quiet"], { cwd: fixture });
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
    "periodic=0",
    'case "$PWD" in',
    "  *lane-2) periodic=1 ;;",
    "esac",
    "elapsed=0",
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
    `    printf '%s\\n' '${triggeredActivityRecord}'`,
    "  fi",
    "  sleep 1",
    "  elapsed=$((elapsed + 1))",
    "  if [ \"$periodic\" -eq 1 ] && [ \"$elapsed\" -ge 20 ]; then",
    `    printf '%s\\n' '${activityRecord}'`,
    "    elapsed=0",
    "  fi",
    "done",
    "",
  ].join("\n"));
  await chmod(vendor, 0o700);
  await writeFile(join(fixture, "spice.toml"), [
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
    'lease = "30s"',
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
  await execute("git", ["add", ".gitignore", "fake-claude", "spice.toml"], { cwd: fixture });
  await execute("git", [
    "-c", "user.name=Canvas Fixture",
    "-c", "user.email=canvas@example.invalid",
    "commit", "--quiet", "-m", "canvas fixture",
  ], { cwd: fixture });
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
  ]);
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

export function monitorPage(page) {
  const pageErrors = [];
  const requestFailures = [];
  let expectedNetworkInterruption = false;
  page.on("pageerror", (error) => pageErrors.push(error.stack ?? error.toString()));
  page.on("console", (message) => {
    if (message.type() === "error") pageErrors.push(`console: ${message.text()}`);
  });
  page.on("requestfailed", (request) => {
    if (!expectedNetworkInterruption) {
      requestFailures.push(`${request.url()} ${request.failure()?.errorText ?? "unknown failure"}`);
    }
  });
  return {
    expectNetworkInterruption(value) {
      expectedNetworkInterruption = value;
    },
    assertClean() {
      assert.deepEqual(pageErrors, [], "page emits no uncaught errors");
      assert.deepEqual(requestFailures, [], "page emits no unexpected request failures");
    },
  };
}

// Switch the canvas to a named view, which several waits treat as a precondition.
//
// BROWSER-1kKZ9jJs traced why a `.status.offline` wait can spend its whole
// bound. The connection flip itself is not the slow part and is not polled:
// `app/src/browser.rs` `run_surface` sets `Connection::Offline` the instant
// `run_connection` returns, and `run_connection` returns as soon as the socket
// reader yields nothing, mapped to "resident closed the socket". No heartbeat
// or interval sits anywhere on that path, and `stopResident` below awaits the
// resident process exit, so the close has already happened before any such
// wait is armed.
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
/// so a timeout at this bound is a defect rather than a busy machine, and the
/// earlier ten seconds bounded nothing that was ever slow.
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
/// This one has a floor written into the client, which sleeps `RECONNECT_DELAY_MS`
/// before every retry with no backoff, so no reconnect can be observed sooner and
/// any bound near the offline flip's 2ms would fail every single time. The
/// measurement lands exactly where that predicts and barely varies: 1283ms
/// minimum, 1287ms median and 1290ms maximum over 25 cycles, the flat 1200ms
/// delay plus 83ms to 90ms of real handshake. The sum is the longest a single
/// successful retry can take: the delay, one whole handshake attempt at its own
/// deadline, and the render.
export const RECONNECT_MS = RECONNECT_DELAY_MS + HANDSHAKE_TIMEOUT_MS + RENDER_SLACK_MS;

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
  // rather than slow, and both spell the same bare TimeoutError. The two modes
  // confirm differently, but not because the marker stops being rendered: the
  // mosaic bar at `chrome.rs:141` renders `render_canvas_menu`, which carries
  // its own Agents pill at `chrome.rs:159` whose selected argument is the
  // mosaic state, so `aria-current` really is set on it. That pill is never
  // *visible*, because it sits inside the `<details class="canvas-menu">` this
  // function has just closed and `waitFor` defaults to the visible state.
  // Confirming on the workspace class instead asks for the state directly
  // rather than for a marker whose visibility is incidental to it.
  const applied =
    name === "Agents"
      ? page.locator(".mosaic-workspace")
      : button.and(page.locator('[aria-current="page"]'));
  await applied.waitFor({ timeout: TRANSITION_MS });
}

export async function stopResident(fixture, resident) {
  const exited = !childExited(resident.child)
    ? once(resident.child, "exit")
    : Promise.resolve();
  await execute(binary, ["--json", "down"], { cwd: fixture, timeout: 10_000 });
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
  // waits for `networkidle`, so the client is not sitting out `RECONNECT_DELAY`
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
    () => page.locator(".event.fault").waitFor({ timeout: TRANSITION_MS }),
    { resident },
  );
}

function reportField(message, name) {
  const line = message.split("\n").find((entry) => entry.startsWith(`  ${name}: `));
  assert.ok(line, `the report carries the ${name} state`);
  return line.slice(`  ${name}: `.length);
}
