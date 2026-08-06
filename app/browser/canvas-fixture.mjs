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
}

export async function stopResident(fixture, resident) {
  const exited = resident.child.exitCode === null
    ? once(resident.child, "exit")
    : Promise.resolve();
  await execute(binary, ["--json", "down"], { cwd: fixture, timeout: 10_000 });
  await exited;
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
    await chooseCanvasView(page, "Mosaic");
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
