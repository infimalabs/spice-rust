// Shared resident, fixture, speech, and lineage scenarios for the sole Canvas
// browser entrypoint. Keeping them here makes file shape explicit, not optional.
import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { once } from "node:events";
import { unwatchFile, watchFile } from "node:fs";
import {
  chmod,
  cp,
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  realpath,
  rm,
  writeFile,
} from "node:fs/promises";
import { connect, createServer } from "node:net";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import { assertTiledWorkspace } from "./tiled-workspace.mjs";

const execute = promisify(execFile);
const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const binary = join(root, "target/debug/spice");
const artifacts = join(root, "artifacts/canvas");
const sessionRecord = JSON.stringify({
  type: "system",
  subtype: "init",
  session_id: "018f7777-7777-7444-8888-999999999999",
});
const activityRecord = JSON.stringify({
  type: "assistant",
  message: {
    type: "message",
    role: "assistant",
    content: [{ type: "text", text: "periodic live witness" }],
    usage: {
      input_tokens: 1,
      cache_read_input_tokens: 0,
      cache_creation_input_tokens: 0,
      output_tokens: 1,
    },
  },
});

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
    if (child.exitCode === null) {
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

async function invoke(fixture, args) {
  try {
    const result = await execute(binary, ["--json", ...args], {
      cwd: fixture,
      timeout: 20_000,
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

export async function configureFixture(fixture, linkedRoot, memberCount = 5) {
  await execute("git", ["init", "--quiet"], { cwd: fixture });
  await mkdir(join(fixture, "dist"), { recursive: true });
  await cp(join(root, "dist/stage-app"), join(fixture, "dist/stage-app"), { recursive: true });
  const vendor = join(fixture, "fake-claude");
  await writeFile(vendor, [
    "#!/bin/sh",
    "trap 'exit 0' TERM INT",
    "name=${PWD##*/}",
    `printf '%s\\n' '${sessionRecord}'`,
    concatAgentRecord(),
    'case "$PWD" in',
    "  *lane-2)",
    `    while :; do sleep 20; printf '%s\\n' '${activityRecord}'; done`,
    "    ;;",
    "  *) while :; do sleep 1; done ;;",
    "esac",
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
  await writeFile(join(fixture, ".gitignore"), ["dist/stage-app", "",].join("\n"));
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
    + "\"text\":\"Canvas message from ";
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
  const exited = resident.child.exitCode === null
    ? once(resident.child, "exit")
    : Promise.resolve();
  await execute(binary, ["--json", "down"], { cwd: fixture, timeout: 10_000 });
  await exited;
}

/// The page-side connection state, read from the topbar that carries it.
///
/// The class names Online against Offline and the label carries the reason the
/// client last recorded, so a page that never saw the socket close reads
/// differently from one that saw it and did not render the change.
///
/// The status element is NOT always present. The topbar holding it is rendered
/// only outside the mosaic workspace, so a wait on a status class is deaf while
/// the mosaic is active. An absent status therefore reports the layout that
/// hides it rather than a bare false, because the two are opposite diagnoses:
/// one is a client that never changed state, the other is a client whose state
/// is simply not on screen to be waited for.
export async function describeConnection(page) {
  return await page
    .evaluate(() => {
      const status = document.querySelector(".status");
      if (!status) {
        return {
          rendered: false,
          topbar: Boolean(document.querySelector(".topbar")),
          mosaic: Boolean(document.querySelector(".mosaic-workspace")),
        };
      }
      return {
        rendered: true,
        classes: status.className,
        label: status.querySelector(".status-copy")?.textContent ?? null,
      };
    })
    .catch((error) => ({ unavailable: String(error) }));
}

/// Whether the resident is gone, and whether anything still answers where it
/// used to listen.
///
/// These are separate questions. A listening socket that outlives the exited
/// process leaves the client connecting to something that never replies, which
/// is a different diagnosis from a refused connection and must not be reported
/// as the same silence.
export async function describeResident(resident, probeTimeout = 2_000) {
  if (!resident) return { known: false };
  const endpoint = resident.discovery?.endpoint ?? null;
  return {
    known: true,
    exited: resident.child.exitCode !== null || resident.child.signalCode !== null,
    exitCode: resident.child.exitCode,
    signal: resident.child.signalCode,
    endpoint,
    endpointState: await probeEndpoint(endpoint, probeTimeout),
  };
}

/// Ask the endpoint for a connection and one minimal HTTP answer.
///
/// A completed TCP handshake proves only that something still owns the port.
/// The stale-listener diagnosis is exactly a socket that accepts that handshake
/// and never answers, so only response bytes establish the healthy accepting
/// state; silence reaches the distinct timeout answer.
function probeEndpoint(endpoint, timeout = 2_000) {
  const target = parseEndpoint(endpoint);
  if (!target) return Promise.resolve("unknown");
  return new Promise((done) => {
    const socket = connect({ host: target.host, port: target.port });
    let settled = false;
    const settle = (outcome) => {
      if (settled) return;
      settled = true;
      socket.destroy();
      done(outcome);
    };
    socket.setTimeout(timeout);
    socket.on("connect", () => {
      socket.write(
        `GET / HTTP/1.1\r\nHost: ${target.host}:${target.port}\r\nConnection: close\r\n\r\n`,
      );
    });
    socket.on("data", () => settle("accepting"));
    socket.on("end", () => settle("accepted and closed before a response"));
    socket.on("timeout", () => settle("accepted nothing before the probe timed out"));
    socket.on("error", (error) => settle(`refused: ${error.code ?? error.message}`));
  });
}

function parseEndpoint(endpoint) {
  if (!endpoint) return null;
  try {
    const url = new URL(endpoint);
    return { host: url.hostname, port: Number(url.port) };
  } catch {
    return null;
  }
}

async function endpointFixture(onConnection = () => {}) {
  const sockets = new Set();
  const server = createServer((socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    onConnection(socket);
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert.ok(address && typeof address === "object", "endpoint fixture has a TCP address");
  return {
    endpoint: `http://127.0.0.1:${address.port}`,
    async close() {
      for (const socket of sockets) socket.destroy();
      await new Promise((resolveClose, rejectClose) => {
        server.close((error) => error ? rejectClose(error) : resolveClose());
      });
    },
  };
}

function residentAt(endpoint) {
  return {
    child: { exitCode: null, signalCode: null },
    discovery: { endpoint },
  };
}

/// A peer can accept the TCP connection and close cleanly without answering.
///
/// Once the peer sends FIN, the socket no longer generates an inactivity
/// timeout. Keep this fixture under its own outer bound so removing the `end`
/// settlement fails by name instead of hanging the rest of the Canvas suite.
export async function assertEndpointProbeClosesWithoutResponse() {
  const closingFixture = await endpointFixture((socket) => socket.end());
  let deadline;
  try {
    const closed = await Promise.race([
      describeResident(residentAt(closingFixture.endpoint), 100),
      new Promise((_, reject) => {
        deadline = setTimeout(
          () => reject(new Error("the accepted-and-closed endpoint probe did not settle")),
          250,
        );
      }),
    ]);
    assert.equal(
      closed.endpointState,
      "accepted and closed before a response",
      "an accepted socket that ends without bytes reports its own bounded state",
    );
    assert.notEqual(closed.endpointState, "accepting");
    assert.notEqual(closed.endpointState, "accepted nothing before the probe timed out");
    assert.doesNotMatch(closed.endpointState, /^refused:/);
  } finally {
    clearTimeout(deadline);
    await closingFixture.close();
  }
}

/// Drive every endpoint report answer against a real TCP endpoint.
///
/// The refused fixture reserves and closes an ephemeral port before probing it.
/// The healthy fixture answers the minimal request, while the stale fixture
/// accepts and deliberately stays silent. Their reports must remain pairwise
/// distinct so `accepting` cannot impersonate either failure cause.
export async function assertEndpointProbeStates() {
  const refusedFixture = await endpointFixture();
  const refusedEndpoint = refusedFixture.endpoint;
  await refusedFixture.close();
  const refused = await describeResident(residentAt(refusedEndpoint), 250);
  assert.match(
    refused.endpointState,
    /^refused: E[A-Z]+$/,
    "a port with no listener reports refusal and its errno",
  );

  const answeringFixture = await endpointFixture((socket) => {
    socket.once("data", () => {
      socket.end("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    });
  });
  const accepting = await describeResident(residentAt(answeringFixture.endpoint), 250);
  await answeringFixture.close();
  assert.equal(
    accepting.endpointState,
    "accepting",
    "an endpoint that accepts and answers reports healthy",
  );

  const silentFixture = await endpointFixture();
  const silent = await describeResident(residentAt(silentFixture.endpoint), 100);
  await silentFixture.close();
  assert.equal(
    silent.endpointState,
    "accepted nothing before the probe timed out",
    "an endpoint that accepts but never answers reports the stale-listener state",
  );
  assert.notEqual(silent.endpointState, accepting.endpointState);
  assert.notEqual(silent.endpointState, refused.endpointState);
}

/// Name the frame that armed the wait, so a shared selector is still locatable.
function callSite(marker) {
  const frames = (marker.stack ?? "").split("\n").slice(1);
  const caller = frames.find((frame) => !frame.includes("observedWait"));
  return caller?.trim() ?? "<call site unavailable>";
}

/// Run a wait and, when it exceeds its bound, replace the bare timeout with a
/// report naming the wait and the state both sides were in.
///
/// The suite is one script, so an unenriched rejection reaches the top level
/// naming only a selector. Several call sites share a selector, and the raw
/// timeout says nothing about whether the page or the resident was at fault,
/// which is exactly the distinction a load-sensitive failure needs.
export async function observedWait(page, label, run, context = {}) {
  const marker = new Error(label);
  try {
    return await run();
  } catch (error) {
    const connection = await describeConnection(page);
    const resident = await describeResident(context.resident);
    throw new Error(
      [
        `canvas wait failed: ${label}`,
        `  site: ${callSite(marker)}`,
        `  connection: ${JSON.stringify(connection)}`,
        `  resident: ${JSON.stringify(resident)}`,
      ].join("\n"),
      { cause: error },
    );
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
  const failure = await observedWait(
    page,
    label,
    () => page.waitForSelector(".spice-wait-instrument-absent", { timeout: 250 }),
    { resident },
  ).then(() => null, (error) => error);
  assert.ok(failure, "a wait that cannot be satisfied must fail");
  assert.match(failure.message, /canvas wait failed: instrument self-check/);
  assert.match(failure.message, /site: .*canvas-fixture\.mjs/);
  assert.match(failure.message, /"endpointState":/, "the report probes the resident endpoint");
  assert.ok(failure.cause, "the original timeout survives as the cause");

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
  await observedWait(
    page,
    "canvas reports online again after the resident restarts",
    () => page.waitForSelector(".status.online", { timeout: 10_000 }),
    { resident },
  );
  await observedWait(
    page,
    "canvas adopts the generation the restarted resident published",
    () =>
      page.waitForFunction(
        (epoch) => document.querySelector(".status-copy")?.textContent?.includes(epoch),
        resident.discovery.epoch,
      ),
    { resident },
  );
  return resident;
}

/// Wait for the canvas to report offline, on the one bound that was measured.
///
/// Three call sites share this: the resident stopping, a stale generation being
/// refused, and the fixture closing the socket from inside the page. All three
/// are the same class of transition -- the page has already been handed the
/// input by the time the wait arms -- so all three take `TRANSITION_MS` rather
/// than a bound chosen per site to outlast whatever last went wrong there.
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

export async function assertLineageLifeIsolation(browser) {
  const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-lineage-life-")));
  const linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-life-lanes-")));
  let resident;
  let context;
  try {
    const worktrees = await configureFixture(fixture, linkedRoot, 8);
    resident = await startResident(fixture);
    const pending = await reply(fixture, ["trust"]);
    await reply(fixture, ["trust", "grant", pending.body.digest]);

    await reply(fixture, [
      "lane", "add", worktrees[0], "--life", "Drain", "--vendor", "claude",
    ]);
    const firstSnapshot = await reply(fixture, ["lane"]);
    const host = firstSnapshot.body.lanes.find((lane) => lane.worktree === worktrees[0]);
    assert.ok(host, "the four-member Drain lineage has a host");
    for (const worktree of worktrees.slice(1, 4)) {
      await reply(fixture, [
        "lane", "add", worktree,
        "--lineage", host.lineage_id,
        "--life", "Drain",
        "--vendor", "claude",
      ]);
    }
    await reply(fixture, [
      "lane", "add", worktrees[4], "--life", "Drive", "--vendor", "claude",
    ]);
    const secondSnapshot = await reply(fixture, ["lane"]);
    const drive = secondSnapshot.body.lanes.find((lane) => lane.worktree === worktrees[4]);
    assert.ok(drive, "the four-member Drive lineage has a host");
    for (const worktree of worktrees.slice(5)) {
      await reply(fixture, [
        "lane", "add", worktree,
        "--lineage", drive.lineage_id,
        "--life", "Drive",
        "--vendor", "claude",
      ]);
    }
    const snapshot = await reply(fixture, ["lane"]);
    const drainMembers = snapshot.body.lanes.filter(
      (lane) => lane.lineage_id === host.lineage_id,
    );
    const driveMembers = snapshot.body.lanes.filter(
      (lane) => lane.lineage_id === drive.lineage_id,
    );
    assert.equal(drainMembers.length, 4, "one admitted lineage contains four members");
    assert.ok(drainMembers.every((lane) => lane.life === "drain"));
    assert.equal(driveMembers.length, 4, "the sibling lineage contains four members");
    assert.ok(driveMembers.every((lane) => lane.life === "drive"));
    assert.notEqual(drive.lineage_id, host.lineage_id);

    context = await browser.newContext({
      colorScheme: "dark",
      viewport: { width: 1000, height: 568 },
    });
    const page = await context.newPage();
    const monitor = monitorPage(page);
    await page.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
    await chooseCanvasView(page, "Agents");
    const console = page.locator('[data-pane="agent-console"]');
    await console.waitFor();
    const drainSurface = console.locator(
      `.team-surface[data-lineage-id="${host.lineage_id}"]`,
    );
    const driveSurface = console.locator(
      `.team-surface[data-lineage-id="${drive.lineage_id}"]`,
    );
    await drainSurface.getByRole("button", { name: "Drain", exact: true }).waitFor();
    await driveSurface.getByRole("button", { name: "Drive", exact: true }).waitFor();
    assert.equal(await console.locator(".team-surface").count(), 2);

    await drainSurface.getByRole("textbox").first().fill("drain lineage accessibility");
    await drainSurface.getByRole("button", { name: "Drain", exact: true }).click();
    const drainPlane = drainSurface.locator(".lane-mosaic-plane");
    await drainPlane
      .locator(".mosaic-message-card")
      .filter({ hasText: "drain lineage accessibility" })
      .waitFor({ timeout: 10_000 });
    await driveSurface.getByRole("textbox").first().fill("drive lineage accessibility");
    await driveSurface.getByRole("button", { name: "Drive", exact: true }).click();
    const drivePlane = driveSurface.locator(".lane-mosaic-plane");
    await drivePlane
      .locator(".mosaic-message-card")
      .filter({ hasText: "drive lineage accessibility" })
      .waitFor({ timeout: 10_000 });

    const planes = console.locator(".lane-mosaic-plane");
    assert.equal(await planes.count(), 2, "two admitted lineages own exactly two packed planes");
    assert.match(await drainPlane.ariaSnapshot(), /Drain policy, 4 members/);
    assert.match(await drivePlane.ariaSnapshot(), /Drive policy, 4 members/);
    await assertTiledWorkspace(
      page,
      console,
      join(artifacts, "canvas-tiled-workspace-dark.png"),
    );

    await drainSurface.locator('.team-lifetime input[type="range"]').fill("0");
    await drainSurface.getByRole("button", { name: "Steer", exact: true }).waitFor();
    const changed = await reply(fixture, ["lane"]);
    assert.equal(
      changed.body.subscriptions.find(
        (subscription) => subscription.lineage_id === host.lineage_id,
      )?.policy,
      "steer",
    );
    assert.ok(
      changed.body.lanes
        .filter((lane) => lane.lineage_id === host.lineage_id)
        .every((lane) => lane.life === "drain"),
      "changing task subscription preserves the lineage's terminal seat life",
    );
    assert.equal(
      await driveSurface.getByRole("button", { name: "Drive", exact: true }).count(),
      1,
      "changing the Drain lineage leaves its Drive sibling unchanged",
    );
    await drainSurface.getByRole("textbox").first().fill("isolated lineage submission");
    await drainSurface.getByRole("button", { name: "Steer", exact: true }).click();
    await drainPlane
      .locator(".mosaic-message-card")
      .filter({ hasText: "isolated lineage submission" })
      .waitFor({ timeout: 10_000 });
    assert.equal(
      await drainSurface.getByRole("button", { name: "Steer", exact: true }).count(),
      1,
      "submitting the changed lineage retains its own policy",
    );
    assert.equal(
      await drivePlane.getAttribute("data-lineage-policy"),
      "Drive",
      "submitting one lineage does not mutate its sibling policy",
    );
    monitor.assertClean();
  } finally {
    if (context) await context.close();
    if (resident?.child.exitCode === null) {
      const liveTeam = await reply(fixture, ["lane"]);
      for (const agent of liveTeam.body.lanes) {
        await reply(fixture, ["lane", "drain", agent.lane]);
      }
      await stopResident(fixture, resident);
    }
    await rm(fixture, { recursive: true, force: true });
    await rm(linkedRoot, { recursive: true, force: true });
  }
}

async function routeSpeechBootstrap(page, speech) {
  await page.route("**/bootstrap", async (route) => {
    const response = await route.fetch();
    const bootstrap = await response.json();
    bootstrap.speech = speech;
    await route.fulfill({
      response,
      contentType: "application/json; charset=utf-8",
      body: JSON.stringify(bootstrap),
    });
  });
}

export async function installBrowserSpeech(context, voices = ["Zoe (Premium)"]) {
  await context.addInitScript((voiceNames) => {
    window.__speechLog = [];
    class FakeVoice {
      constructor(name) { this.name = name; }
    }
    class FakeUtterance {
      constructor(text) {
        this.text = text;
        this.voice = null;
        this.rate = 1;
        this.onend = null;
        this.onerror = null;
      }
    }
    class FakeAudio {
      constructor(src) {
        this.src = src;
        this.currentTime = 0;
        this.onended = null;
        this.onerror = null;
        window.__speechLog.push({ action: "audio", src });
      }
      play() {
        window.__speechLog.push({ action: "play" });
        return Promise.resolve();
      }
      pause() { window.__speechLog.push({ action: "pause" }); }
    }
    window.SpeechSynthesisVoice = FakeVoice;
    window.SpeechSynthesisUtterance = FakeUtterance;
    window.Audio = FakeAudio;
    Object.defineProperty(window, "speechSynthesis", {
      configurable: true,
      value: {
        cancel() { window.__speechLog.push({ action: "cancel" }); },
        getVoices() { return voiceNames.map((name) => new FakeVoice(name)); },
        speak(utterance) {
          window.__speechLog.push({
            action: "speak",
            text: utterance.text,
            voice: utterance.voice?.name,
            rate: utterance.rate,
          });
        },
      },
    });
  }, voices);
}

export async function installSocketControl(context) {
  await context.addInitScript(() => {
    const NativeWebSocket = window.WebSocket;
    window.__canvasSockets = [];
    window.WebSocket = class FixtureWebSocket extends NativeWebSocket {
      constructor(...args) {
        super(...args);
        window.__canvasSockets.push(this);
      }
    };
  });
}

export async function assertSystemSpeech(page) {
  await chooseCanvasView(page, "Flow");
  const button = page.locator(".speech-button");
  await button.waitFor();
  assert.equal(await button.getAttribute("aria-label"), "Read current canvas");
  const before = await button.boundingBox();
  assert.ok(before, "speech control has stable geometry");
  await button.click();
  await page.locator('.speech-button[data-state="playing"]').waitFor();
  assert.equal(await button.getAttribute("aria-label"), "Stop reading");
  const after = await button.boundingBox();
  assert.deepEqual(after, before, "play state does not shift the speech control");
  const spoken = await page.evaluate(() => window.__speechLog.find((row) => row.action === "speak"));
  assert.equal(spoken.voice, "Zoe (Premium)");
  assert.equal(spoken.rate, 1);
  assert.match(spoken.text, /Flow/);
  assert.match(spoken.text, /Current work/);
  await button.click();
  await page.locator('.speech-button[data-state="idle"]').waitFor();
  const actions = await page.evaluate(() => window.__speechLog.map((row) => row.action));
  assert.ok(actions.includes("cancel"), "stop cancels system speech");
}

export async function assertSpeechVariants(browser, endpoint) {
  const unavailableContext = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  await installBrowserSpeech(unavailableContext, []);
  const unavailablePage = await unavailableContext.newPage();
  const unavailableMonitor = monitorPage(unavailablePage);
  await unavailablePage.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(unavailablePage, "Flow");
  await unavailablePage.waitForSelector(".status.online");
  await unavailablePage.locator(".speech-button").click();
  await unavailablePage.locator('.speech-button[data-state="error"]').waitFor();
  assert.match(await unavailablePage.locator(".speech-button").getAttribute("title"), /Zoe.*unavailable/);
  unavailableMonitor.assertClean();
  await unavailableContext.close();

  const variants = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  await installBrowserSpeech(variants);

  const disabled = await variants.newPage();
  const disabledMonitor = monitorPage(disabled);
  await routeSpeechBootstrap(disabled, { backend: "disabled" });
  await disabled.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(disabled, "Flow");
  await disabled.waitForSelector(".status.online");
  assert.equal(await disabled.locator(".speech-button").count(), 0, "disabled hides playback");
  disabledMonitor.assertClean();
  await disabled.close();

  const externalUnavailable = await variants.newPage();
  const externalUnavailableMonitor = monitorPage(externalUnavailable);
  await routeSpeechBootstrap(externalUnavailable, {
    backend: "external",
    content_type: "audio/wav",
    words_per_minute: 175,
    available: false,
  });
  await externalUnavailable.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(externalUnavailable, "Flow");
  await externalUnavailable.waitForSelector(".status.online");
  const unavailableButton = externalUnavailable.locator(".speech-button");
  assert.equal(await unavailableButton.getAttribute("aria-label"), "Speech unavailable");
  assert.equal(await unavailableButton.isDisabled(), true);
  externalUnavailableMonitor.assertClean();
  await externalUnavailable.close();

  const external = await variants.newPage();
  const externalMonitor = monitorPage(external);
  await routeSpeechBootstrap(external, {
    backend: "external",
    content_type: "audio/wav",
    words_per_minute: 175,
    available: true,
  });
  let submitted;
  await external.route("**/speech", async (route) => {
    submitted = route.request().postDataJSON();
    await route.fulfill({
      status: 200,
      contentType: "audio/wav",
      body: Buffer.from("fixture-audio"),
    });
  });
  await external.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(external, "Flow");
  await external.waitForSelector(".status.online");
  const externalButton = external.locator(".speech-button");
  await externalButton.click();
  await external.locator('.speech-button[data-state="playing"]').waitFor();
  assert.deepEqual(Object.keys(submitted), ["text"]);
  assert.match(submitted.text, /Flow/);
  await externalButton.click();
  await external.locator('.speech-button[data-state="idle"]').waitFor();
  const externalActions = await external.evaluate(() => window.__speechLog.map((row) => row.action));
  assert.ok(externalActions.includes("play"));
  assert.ok(externalActions.includes("pause"));
  externalMonitor.assertClean();
  await external.close();

  const malformed = await variants.newPage();
  const malformedMonitor = monitorPage(malformed);
  await routeSpeechBootstrap(malformed, { backend: "system", voice: "Zoe (Premium)" });
  await malformed.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(malformed, "Chronology");
  await malformed.waitForSelector(".status.offline");
  assert.match(await malformed.locator(".status-copy").innerText(), /bootstrap response was invalid/);
  assert.equal(await malformed.locator(".speech-button").count(), 0);
  assert.match(await malformed.locator(".event.fault").innerText(), /bootstrap response was invalid/);
  malformedMonitor.assertClean();
  await malformed.close();

  await variants.close();
}
