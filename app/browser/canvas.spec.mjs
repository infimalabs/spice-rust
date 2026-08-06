import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { once } from "node:events";
import { unwatchFile, watchFile } from "node:fs";
import { chmod, mkdtemp, mkdir, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import { chromium } from "playwright";

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
const staleEpoch = "018f9999-9999-7444-8888-999999999999";

const axisPanes = new Map([
  ["Magnitude", ["Effort", "Pressure", "Residue", "Consent Volume"]],
  ["Flow", ["Tasks", "Claims", "Deferrals", "Reviews"]],
  ["Topology", ["Lanes", "Worktrees", "Lineage", "Occupancy", "Residents"]],
  ["Chronology", [
    "Compaction",
    "Queues",
    "Directives",
    "Consent Disposition",
    "Trust",
    "Commands",
    "Recovery",
  ]],
]);

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

async function startResident(fixture) {
  const canvas = join(fixture, ".git/.spice/canvas");
  await mkdir(dirname(canvas), { recursive: true });
  const child = spawn(binary, ["--json", "up"], {
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

async function reply(fixture, args) {
  const outcome = await invoke(fixture, args);
  assert.equal(outcome.kind, "reply", `${args.join(" ")} replies: ${JSON.stringify(outcome)}`);
  return outcome;
}

async function configureFixture(fixture) {
  const vendor = join(fixture, "fake-claude");
  await writeFile(vendor, [
    "#!/bin/sh",
    "trap 'exit 0' TERM INT",
    `printf '%s\\n' '${sessionRecord}'`,
    "while :; do sleep 1; done",
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
    'stems = ["app", "canvas"]',
    "",
    "[vendor.claude]",
    `binary = ${JSON.stringify(vendor)}`,
    "",
    "[canvas.pack]",
    "rows = 2",
    "",
  ].join("\n"));
}

async function journalExtent(fixture) {
  const report = await reply(fixture, ["journal"]);
  assert.ok(report.body.cursor, "running authority has a journal cursor");
  return {
    cursor: Number(report.body.cursor),
    facts: Number(report.body.facts),
  };
}

async function seedAuthority(fixture) {
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
  await reply(fixture, ["task", "note", first.body.task, "first causal observation"]);
  await reply(fixture, ["task", "note", first.body.task, "second causal observation"]);

  return {
    task: first.body.task,
    extent: await journalExtent(fixture),
  };
}

function monitorPage(page) {
  const pageErrors = [];
  const requestFailures = [];
  let expectedNetworkInterruption = false;
  page.on("pageerror", (error) => pageErrors.push(error.stack ?? error.toString()));
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

async function stopResident(fixture, resident) {
  const exited = resident.child.exitCode === null
    ? once(resident.child, "exit")
    : Promise.resolve();
  await execute(binary, ["--json", "down"], { cwd: fixture, timeout: 10_000 });
  await exited;
}

async function assertCanvas(page, scheme) {
  try {
    await page.waitForSelector(".status.online", { timeout: 10_000 });
  } catch (error) {
    const body = await page.locator("body").innerText().catch(() => "<body unavailable>");
    throw new Error(`canvas did not come online; body=${JSON.stringify(body)}`, { cause: error });
  }
  await page.waitForFunction(() => document.querySelector("#app")?.childElementCount > 0);
  await assert.doesNotReject(() => page.locator(".brand-name:visible, .mobile-brand:visible").waitFor());
  await assert.doesNotReject(() => page.getByRole("heading", { name: "Agents", exact: true }).waitFor());
  await page.waitForFunction(() => {
    const cursor = document.querySelector(".rail-meta")?.textContent ?? "";
    return /cursor \d+/.test(cursor);
  });

  const pixels = await page.evaluate(() => {
    const root = document.querySelector("#app");
    const canvas = document.querySelector(".content");
    const rootStyle = root ? getComputedStyle(root) : null;
    const canvasBox = canvas?.getBoundingClientRect();
    const workspaceBox = document.querySelector(".workspace")?.getBoundingClientRect();
    const railBox = document.querySelector(".rail")?.getBoundingClientRect();
    return {
      appChildren: root?.childElementCount ?? 0,
      bodyWidth: document.body.scrollWidth,
      viewportWidth: window.innerWidth,
      canvasHeight: Math.round(canvasBox?.height ?? 0),
      ink: rootStyle?.color ?? "",
      background: getComputedStyle(document.body).backgroundColor,
      mobileNavOverlap: window.innerWidth <= 560
        && (workspaceBox?.bottom ?? 0) > (railBox?.top ?? window.innerHeight),
      overflowingControls: [...document.querySelectorAll("button, .pane-chip")]
        .filter((element) => {
          const style = getComputedStyle(element);
          return style.display !== "none" && style.visibility !== "hidden";
        })
        .filter((element) => element.scrollWidth > element.clientWidth + 1)
        .map((element) => element.textContent?.trim() ?? element.className),
    };
  });
  assert.ok(pixels.appChildren > 0, "Wasm mounted a nonblank app");
  assert.ok(pixels.canvasHeight > 300, "primary canvas has visible content");
  assert.ok(pixels.bodyWidth <= pixels.viewportWidth, "page has no viewport overflow");
  assert.notEqual(pixels.ink, pixels.background, "foreground and background pixels differ");
  assert.equal(pixels.mobileNavOverlap, false, "mobile workspace stops above bottom navigation");
  assert.deepEqual(pixels.overflowingControls, [], "controls and pane labels fit their bounds");

  for (const [axis, panes] of axisPanes) {
    await page.getByRole("button", { name: axis }).click();
    await page.getByRole("heading", { name: axis, exact: true }).waitFor();
    assert.deepEqual(
      await page.locator(".pane-chip").allTextContents(),
      panes,
      `${axis} renders its complete declared pane registry`,
    );
  }
  await page.keyboard.press("Tab");
  assert.ok(await page.locator(":focus-visible").count(), "keyboard focus remains visible");
  if ((page.viewportSize()?.width ?? 1000) <= 560) {
    await page.getByRole("button", { name: "Flow" }).click();
    await page.locator(".workspace").evaluate((element) => element.scrollTo(0, element.scrollHeight));
    assert.ok(await page.getByRole("button", { name: "Add note" }).isVisible());
    await page.locator(".workspace").evaluate((element) => element.scrollTo(0, 0));
  }
  await page.getByRole("button", { name: "Mosaic" }).click();
  await page.getByRole("heading", { name: "Agents", exact: true }).waitFor();
  await page.screenshot({ path: join(artifacts, `canvas-${scheme}.png`), fullPage: true });
}

async function assertAgentConversation(page, fixture) {
  const console = page.locator('[data-pane="agent-console"]');
  await page.getByRole("button", { name: "Mosaic" }).click();
  await console.getByRole("heading", { name: "Agents", exact: true }).waitFor();
  await console.getByLabel("Worktree").fill(fixture);
  await console.getByLabel("Vendor").fill("claude");
  await console.getByLabel("Life").fill("steer");
  await console.getByRole("button", { name: "Seat agent" }).click();

  const tab = console.locator(".agent-tab");
  try {
    await tab.waitFor({ timeout: 15_000 });
  } catch (error) {
    await page.getByRole("button", { name: "Chronology" }).click();
    const rendered = await page.locator("body").innerText();
    const lanes = await invoke(fixture, ["lane"]);
    throw new Error(`Canvas seat did not create a lane: lanes=${JSON.stringify(lanes)} body=${JSON.stringify(rendered)}`, {
      cause: error,
    });
  }
  const lanes = await reply(fixture, ["lane"]);
  const seated = lanes.body.lanes.find((lane) => lane.worktree === fixture);
  assert.ok(seated, "the Canvas seat control creates a lane in the requested worktree");
  assert.equal(seated.vendor, "claude");
  assert.equal(seated.life, "steer");
  assert.equal(await tab.getAttribute("data-lane"), seated.lane);

  const message = "browser direction survives streaming";
  await console.getByLabel("Message").fill(message);
  await console.getByRole("button", { name: "Send direction" }).click();
  const outgoing = console.locator(".message.outgoing").filter({ hasText: message });
  await outgoing.waitFor({ timeout: 10_000 });
  assert.match(await outgoing.innerText(), /pending/);

  const inbox = await reply(fixture, ["inbox", seated.lane]);
  assert.equal(inbox.body.lanes.length, 1);
  assert.equal(inbox.body.lanes[0].pending.length, 1);
  assert.equal(inbox.body.lanes[0].pending[0].text, message);
  await assertMosaicPacking(page, console);
  await page.screenshot({ path: join(artifacts, "canvas-light-mosaic-agent.png"), fullPage: true });
  return seated.lane;
}

async function assertMosaicPacking(page, console) {
  const feed = console.locator(".mosaic-feed");
  const laneCount = Number(await feed.getAttribute("data-lanes"));
  assert.ok(laneCount >= 1 && laneCount <= 6, "geometry exposes a legal base-lane count");
  const targetCards = laneCount * 2 + 1;
  for (let index = 1; index < targetCards; index += 1) {
    await console.getByLabel("Message").fill(`mosaic placement ${index}`);
    await console.getByRole("button", { name: "Send direction" }).click();
  }
  const cards = console.locator(".mosaic-card");
  await cards.nth(targetCards - 1).waitFor({ timeout: 15_000 });
  assert.equal(await cards.count(), targetCards);
  assert.equal(await cards.first().getAttribute("data-state"), "frozen");

  const placements = await cards.evaluateAll((elements) => elements.map((element) => {
    const box = element.getBoundingClientRect();
    return {
      track: Number(element.dataset.track),
      span: Number(element.dataset.span),
      base: Number(element.dataset.base),
      rows: Number(element.dataset.rows),
      left: box.left,
      right: box.right,
      top: box.top,
      bottom: box.bottom,
    };
  }));
  assert.ok(placements.every((card) => card.span >= 2 && card.track + card.span <= 12));
  assert.ok(placements.every((card) => card.rows >= 2));
  for (let left = 0; left < placements.length; left += 1) {
    for (let right = left + 1; right < placements.length; right += 1) {
      const a = placements[left];
      const b = placements[right];
      const separated = a.right <= b.left + 1 || b.right <= a.left + 1
        || a.bottom <= b.top + 1 || b.bottom <= a.top + 1;
      assert.ok(separated, `mosaic cards ${left} and ${right} do not overlap`);
    }
  }

  await feed.evaluate((element) => element.scrollTo(0, element.scrollHeight));
  const before = await feed.evaluate((element) => element.scrollTop);
  await console.getByLabel("Message").fill("mosaic scroll compensation");
  await console.getByRole("button", { name: "Send direction" }).click();
  await cards.nth(targetCards).waitFor({ timeout: 10_000 });
  const after = await feed.evaluate((element) => element.scrollTop);
  assert.ok(after >= before, "incremental placement preserves a scrolled viewport");
  await page.setViewportSize({ width: 1200, height: 900 });
  await page.setViewportSize({ width: 1440, height: 900 });
  await cards.nth(targetCards).waitFor();
}

async function assertAuthoritySurface(page, seeded) {
  await page.getByRole("button", { name: "Flow" }).click();
  const currentWork = page.locator(".section").filter({ hasText: "Current work" });
  await currentWork.getByText("Browser fixture task", { exact: true }).waitFor();
  await currentWork.getByText("Continuation witness", { exact: true }).waitFor();

  await page.getByRole("button", { name: "Magnitude" }).click();
  await page.getByRole("heading", { name: "Consent budgets", exact: true }).waitFor();
  await page.getByText("browser", { exact: true }).waitFor();
  assert.match(await page.locator(".section").filter({ hasText: "Consent budgets" }).innerText(), /2\s+2/);

  await page.getByRole("button", { name: "Topology" }).click();
  const topology = page.locator("main.content");
  await topology.getByText("Resident topology", { exact: true }).waitFor();
  await topology.getByText("claude", { exact: true }).waitFor();
  assert.match(await topology.innerText(), new RegExp(seeded.lane.slice(0, 8)));

  await page.getByRole("button", { name: "Chronology" }).click();
  await page.getByText("browser direction survives streaming", { exact: true }).waitFor();
  const trust = page.locator('[data-pane="trust"]');
  await trust.waitFor();
  const trustText = await trust.innerText();
  assert.match(trustText, /trust::pended/);
  assert.match(trustText, /trust::granted/);
  assert.match(trustText, /granted/);
}

async function assertPackedHistory(page, extent, linked = false) {
  await page.getByRole("button", { name: "Chronology" }).click();
  const journal = page.locator('[data-pane="commands"]');
  const facts = journal.locator(".event strong");
  await facts.nth(extent.facts - 1).waitFor();
  assert.equal(await facts.count(), extent.facts, `all ${extent.facts} admitted facts render`);

  const provenance = await journal.locator('[data-subject="facts"]').allTextContents();
  assert.ok(provenance.length > 1, "two-row packing produces multiple fact pages");
  assert.match(provenance[0], /omitted \/ continue/, "first page exposes overflow continuation");
  assert.doesNotMatch(provenance.at(-1), /omitted \/ continue/, "final page closes continuation");
  const horizons = await journal
    .locator('[data-subject="facts"] span')
    .evaluateAll((nodes) => nodes.map((node) => node.textContent?.trim()).filter((text) => text?.startsWith("horizon ")));
  const horizonValues = horizons.map((text) => Number(text.split(" ")[1]));
  assert.equal(horizonValues.at(-1), extent.cursor, "latest page reaches the current horizon");
  assert.ok(
    horizonValues.every((value, index) => index === 0 || value >= horizonValues[index - 1]),
    "continued and later live pages retain monotone packed horizons",
  );

  const sequences = await journal.locator(".event strong").evaluateAll((nodes) => nodes.map((node) => {
    const value = Number(node.textContent?.split("/")[0].trim());
    if (!Number.isInteger(value)) throw new Error(`invalid rendered sequence: ${node.textContent}`);
    return value;
  }));
  assert.equal(new Set(sequences).size, extent.facts, "continuation fabricates no duplicate fact");
  assert.deepEqual(
    [...sequences].sort((left, right) => left - right),
    Array.from({ length: extent.facts }, (_, index) => index + 1),
    "continuation loses no fact before the packed horizon",
  );

  await page.getByRole("button", { name: "Topology" }).click();
  const thread = page.locator('[data-pane="lineage"]');
  const minimumRows = linked ? 2 : 1;
  const threadRowsLocator = thread.locator("tbody tr");
  await threadRowsLocator.nth(minimumRows - 1).waitFor();
  assert.ok(
    await threadRowsLocator.count() >= minimumRows,
    linked ? "multi-link causal thread renders" : "terminal causal thread renders",
  );
  assert.match(await thread.locator(".section-count").innerText(), /root/);
  const threadRows = await thread.locator("tbody tr").allTextContents();
  if (linked) {
    assert.ok(threadRows.some((row) => !row.endsWith("root")), "causal walk includes a linked cause");
  }

  await page.getByRole("button", { name: "Chronology" }).click();
  await page.screenshot({ path: join(artifacts, "canvas-chronology.png"), fullPage: true });
}

async function assertNoCredentialOrLandingGap(page, capabilities) {
  const content = await page.content();
  assert.doesNotMatch(content, /release::not-landed/);
  for (const capability of capabilities) {
    assert.ok(!content.includes(capability), "rendered DOM does not expose a capability");
  }
  const urls = await page.evaluate(() => performance.getEntriesByType("resource").map((entry) => entry.name));
  for (const capability of capabilities) {
    assert.ok(urls.every((url) => !url.includes(capability)), "resource URLs do not expose a capability");
  }
}

async function assertStaleGeneration(browser, endpoint, currentEpoch) {
  const context = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  const page = await context.newPage();
  const monitor = monitorPage(page);
  await page.route("**/bootstrap", async (route) => {
    const response = await route.fetch();
    const bootstrap = await response.json();
    assert.equal(bootstrap.epoch, currentEpoch);
    bootstrap.epoch = staleEpoch;
    await route.fulfill({
      response,
      contentType: "application/json; charset=utf-8",
      body: JSON.stringify(bootstrap),
    });
  });
  await page.goto(endpoint, { waitUntil: "domcontentloaded" });
  await page.waitForSelector(".status.offline", { timeout: 10_000 });
  await page.getByRole("button", { name: "Chronology" }).click();
  const fault = page.locator(".event.fault");
  await fault.waitFor();
  assert.match(await fault.innerText(), /Canvas connection reset/);
  assert.match(await fault.innerText(), /accepted|frame|epoch|socket/i);
  monitor.assertClean();
  await context.close();
}

async function assertSameGenerationResume(
  page,
  monitor,
  fixture,
  seeded,
  epoch,
  beforeExtent,
) {
  await page.getByRole("button", { name: "Chronology" }).click();
  const before = await page.locator('[data-pane="commands"] .event strong').allTextContents();
  monitor.expectNetworkInterruption(true);
  await page.evaluate(() => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    socket.close(4000, "fixture reconnect");
  });
  await page.waitForSelector(".status.offline", { timeout: 10_000 });
  assert.match(await page.locator(".status-copy").innerText(), /Reconnecting/);
  await page.getByRole("button", { name: "Chronology" }).click();
  await page.locator(".event.fault").waitFor();

  await reply(fixture, ["task", "note", seeded.task, "written during browser disconnect"]);
  const advanced = await journalExtent(fixture);
  assert.ok(advanced.facts > beforeExtent.facts, "offline command advances resident authority");

  await page.waitForSelector(".status.online", { timeout: 15_000 });
  await page.waitForFunction((expected) => document.querySelector(".status-copy")?.textContent?.includes(expected), epoch);
  monitor.expectNetworkInterruption(false);
  await assertPackedHistory(page, advanced);
  const after = await page.locator('[data-pane="commands"] .event strong').allTextContents();
  assert.ok(after.length > before.length, "same-generation resume appends missed facts");
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

async function installBrowserSpeech(context, voices = ["Zoe (Premium)"]) {
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

async function installSocketControl(context) {
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

async function assertSystemSpeech(page) {
  await page.getByRole("button", { name: "Flow" }).click();
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

async function assertSpeechVariants(browser, endpoint) {
  const unavailableContext = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  await installBrowserSpeech(unavailableContext, []);
  const unavailablePage = await unavailableContext.newPage();
  const unavailableMonitor = monitorPage(unavailablePage);
  await unavailablePage.goto(endpoint, { waitUntil: "networkidle" });
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
  await external.waitForSelector(".status.online");
  await external.getByRole("button", { name: "Flow" }).click();
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
  await malformed.waitForSelector(".status.offline");
  assert.match(await malformed.locator(".status-copy").innerText(), /bootstrap response was invalid/);
  assert.equal(await malformed.locator(".speech-button").count(), 0);
  await malformed.getByRole("button", { name: "Chronology" }).click();
  assert.match(await malformed.locator(".event.fault").innerText(), /bootstrap response was invalid/);
  malformedMonitor.assertClean();
  await malformed.close();

  await variants.close();
}

const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-browser-")));
await mkdir(join(fixture, ".git"));
await mkdir(artifacts, { recursive: true });
await configureFixture(fixture);

let resident;
let browser;
try {
  resident = await startResident(fixture);
  const seeded = await seedAuthority(fixture);
  browser = await chromium.launch({ headless: true });

  const light = await browser.newContext({
    colorScheme: "light",
    viewport: { width: 1440, height: 900 },
  });
  await installBrowserSpeech(light);
  await installSocketControl(light);
  const lightPage = await light.newPage();
  const lightMonitor = monitorPage(lightPage);
  lightPage.on("console", (message) => process.stderr.write(`browser console: ${message.text()}\n`));
  await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await assertCanvas(lightPage, "light");
  seeded.lane = await assertAgentConversation(lightPage, fixture);
  seeded.extent = await journalExtent(fixture);
  await assertAuthoritySurface(lightPage, seeded);
  await assertPackedHistory(lightPage, seeded.extent, true);
  await assertNoCredentialOrLandingGap(lightPage, [resident.discovery.capability]);
  await assertSystemSpeech(lightPage);
  await lightPage.getByRole("button", { name: "Flow" }).click();
  await lightPage.getByLabel("Task").fill(seeded.task);
  await lightPage.getByLabel("Observation").fill("browser mutation reached the resident");
  await lightPage.getByRole("button", { name: "Add note" }).click();
  await lightPage.getByRole("button", { name: "Chronology" }).click();
  await lightPage.getByText(/Note appended to/).waitFor({ timeout: 10_000 });
  await lightPage.getByRole("button", { name: "Flow" }).click();
  const existingTasks = await lightPage.locator("#note-tasks option").evaluateAll(
    (options) => options.map((option) => option.value),
  );
  const absentTask = ["SPICE-aBcD", "SPICE-zYxW"].find(
    (candidate) => !existingTasks.includes(candidate),
  );
  assert.ok(absentTask, "an absent canonical task handle is available");
  await lightPage.getByLabel("Task").fill(absentTask);
  await lightPage.getByLabel("Observation").fill("this task does not exist");
  await lightPage.getByRole("button", { name: "Add note" }).click();
  await lightPage.getByRole("button", { name: "Chronology" }).click();
  await lightPage.locator(".event.refusal").waitFor({ timeout: 10_000 });
  const beforeReconnect = await journalExtent(fixture);
  await assertPackedHistory(lightPage, beforeReconnect);

  const dark = await browser.newContext({
    colorScheme: "dark",
    viewport: { width: 390, height: 844 },
  });
  await installBrowserSpeech(dark);
  const darkPage = await dark.newPage();
  const darkMonitor = monitorPage(darkPage);
  await darkPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await assertCanvas(darkPage, "dark-narrow");
  await assertAuthoritySurface(darkPage, seeded);
  await assertNoCredentialOrLandingGap(darkPage, [resident.discovery.capability]);
  darkMonitor.assertClean();
  await assertSpeechVariants(browser, resident.discovery.endpoint);
  await assertStaleGeneration(browser, resident.discovery.endpoint, resident.discovery.epoch);

  await assertSameGenerationResume(
    lightPage,
    lightMonitor,
    fixture,
    seeded,
    resident.discovery.epoch,
    beforeReconnect,
  );
  lightMonitor.assertClean();
  await dark.close();

  await reply(fixture, ["lane", "drain", seeded.lane]);
  const firstEpoch = resident.discovery.epoch;
  const firstCapability = resident.discovery.capability;
  lightMonitor.expectNetworkInterruption(true);
  await stopResident(fixture, resident);
  await lightPage.waitForSelector(".status.offline", { timeout: 10_000 });
  await lightPage.getByRole("button", { name: "Chronology" }).click();
  await lightPage.locator(".event.fault").waitFor({ timeout: 10_000 });
  resident = await startResident(fixture);
  assert.notEqual(resident.discovery.epoch, firstEpoch, "restart publishes a new generation");
  assert.notEqual(
    resident.discovery.capability,
    firstCapability,
    "restart publishes a new operator capability",
  );
  await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await lightPage.waitForSelector(".status.online", { timeout: 10_000 });
  await lightPage.waitForFunction(
    (epoch) => document.querySelector(".status-copy")?.textContent?.includes(epoch),
    resident.discovery.epoch,
  );
  lightMonitor.expectNetworkInterruption(false);
  await assertCanvas(lightPage, "light-restarted");
  await lightPage.getByRole("button", { name: "Chronology" }).click();
  assert.equal(
    await lightPage.getByText(/Note appended to/).count(),
    0,
    "new generation clears prior command reply state",
  );
  const restartedExtent = await journalExtent(fixture);
  await assertPackedHistory(lightPage, restartedExtent);
  await assertAuthoritySurface(lightPage, seeded);
  await assertNoCredentialOrLandingGap(lightPage, [firstCapability, resident.discovery.capability]);
  lightMonitor.assertClean();

  await light.close();
} finally {
  if (browser) await browser.close();
  if (resident?.child.exitCode === null) {
    const exited = once(resident.child, "exit");
    resident.child.kill("SIGKILL");
    await exited;
  }
  await rm(fixture, { recursive: true, force: true });
}
