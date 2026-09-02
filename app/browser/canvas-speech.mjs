// Speech installation and backend scenarios for the Canvas browser suite.
import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import {
  assertOfflineObserved,
  assertOnlineObserved,
  chooseCanvasView,
  CONNECT_MS,
  monitorPage,
  reply,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";
import { launchBrowser } from "./headless-launch.mjs";

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

async function settleSpeechBackend(page) {
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
}

export async function installBrowserSpeech(context, voices = ["Zoe (Premium)"]) {
  await context.addInitScript((voiceNames) => {
    window.__speechLog = [];
    window.__speechUtterances = [];
    window.__speechActive = 0;
    window.__speechMaxActive = 0;
    window.__holdSpeechPlay = false;
    window.__speechPlayPending = [];
    window.__speechAudios = [];
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
        this.onpause = null;
        this.ended = false;
        window.__speechAudios.push(this);
        window.__speechLog.push({ action: "audio", src });
      }
      play() {
        window.__speechLog.push({ action: "play" });
        if (window.__holdSpeechPlay) {
          return new Promise((resolve, reject) => {
            window.__speechPlayPending.push({ resolve, reject });
          });
        }
        return Promise.resolve();
      }
      pause() {
        window.__speechLog.push({ action: "pause" });
        this.onpause?.(new Event("pause"));
      }
    }
    window.SpeechSynthesisVoice = FakeVoice;
    window.SpeechSynthesisUtterance = FakeUtterance;
    window.Audio = FakeAudio;
    Object.defineProperty(window, "speechSynthesis", {
      configurable: true,
      value: {
        cancel() {
          window.__speechLog.push({ action: "cancel" });
          window.__speechActive = 0;
        },
        getVoices() { return voiceNames.map((name) => new FakeVoice(name)); },
        speak(utterance) {
          const ended = utterance.onend;
          const failed = utterance.onerror;
          utterance.onend = (...args) => {
            window.__speechActive = Math.max(0, window.__speechActive - 1);
            ended?.(...args);
          };
          utterance.onerror = (...args) => {
            window.__speechActive = Math.max(0, window.__speechActive - 1);
            failed?.(...args);
          };
          window.__speechUtterances.push(utterance);
          window.__speechActive += 1;
          window.__speechMaxActive = Math.max(window.__speechMaxActive, window.__speechActive);
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
        this.__fixtureInboundFrames = 0;
        this.__fixtureSubscriptionReady = new Promise((resolve) => {
          this.addEventListener("message", () => {
            this.__fixtureInboundFrames += 1;
            if (this.__fixtureInboundFrames === 2) resolve();
          });
        });
        window.__canvasSockets.push(this);
      }
    };
  });
}

export async function assertAutomaticSpeechEngines(endpoint, fixture, worktree, lane) {
  for (const engine of ["chromium", "firefox"]) {
    await assertAutomaticSpeechEngine(engine, endpoint, fixture, worktree, lane);
  }
}

async function assertAutomaticSpeechEngine(engine, endpoint, fixture, worktree, lane) {
  const browser = await launchBrowser(engine);
  const context = await browser.newContext({ viewport: { width: 1000, height: 720 } });
  await installBrowserSpeech(context);
  const page = await context.newPage();
  const monitor = monitorPage(page);
  try {
    await page.goto(endpoint, { waitUntil: "networkidle" });
    await chooseCanvasView(page, "Agents");
    const surface = page.locator(".team-surface", {
      has: page.locator(`[data-agent-lane="${lane}"]`),
    });
    await surface.getByRole("tab", { name: /^Compose/ }).click();
    const mode = surface.locator('.team-speech input[type="range"]');
    assert.equal(await mode.inputValue(), "1", `${engine} defaults to Speak`);
    assert.equal(await speechCount(page), 0, `${engine} materialized history stays silent`);

    await emitAssistantRecord(worktree, page, lane, `${engine} ordinary progress`);
    assert.equal(await speechCount(page), 0, `${engine} Speak ignores commentary`);

    const acked = await reply(fixture, ["steer", lane, `${engine} acknowledged direction`]);
    await emitAssistantRecord(worktree, page, lane, `ACK ${acked.body.key}: applied ${engine}`);
    await waitForSpeech(page, 1, `${engine} canonical ACK`);
    assert.equal(await latestSpeechText(page), `applied ${engine}`);
    await settleLatestSpeech(page);

    await resetSpeech(page);
    await emitAssistantRecord(worktree, page, lane, "ACK not-a-key: malformed");
    assert.equal(await speechCount(page), 0, `${engine} malformed ACK stays silent`);
    const declined = await reply(fixture, ["steer", lane, `${engine} declined direction`]);
    await emitAssistantRecord(worktree, page, lane, `NACK ${declined.body.key}: unsafe`);
    assert.equal(await speechCount(page), 0, `${engine} NACK-only prose stays silent`);

    const finalText = [
      `Opened [the ${engine} final](https://example.test).`,
      "middle paragraph must stay silent",
      "Landed 7a12bc99.",
    ].join("\n\n");
    await emitAssistantRecord(worktree, page, lane, finalText, { final: true });
    await waitForSpeech(page, 1, `${engine} final answer`);
    assert.equal(
      await latestSpeechText(page),
      `Opened the ${engine} final. Landed commit 7 a 1 2 b c 9.`,
    );
    await settleLatestSpeech(page);

    await page.reload({ waitUntil: "networkidle" });
    await chooseCanvasView(page, "Agents");
    assert.equal(await speechCount(page), 0, `${engine} reload replays no speech`);
    assert.equal(await mode.inputValue(), "1", `${engine} retains default Speak after reload`);

    await mode.fill("0");
    await page.reload({ waitUntil: "networkidle" });
    await chooseCanvasView(page, "Agents");
    assert.equal(await mode.inputValue(), "0", `${engine} persists Quiet`);
    await emitAssistantRecord(worktree, page, lane, `${engine} quiet final`, { final: true });
    assert.equal(await speechCount(page), 0, `${engine} Quiet suppresses a final answer`);

    await mode.fill("2");
    await page.reload({ waitUntil: "networkidle" });
    await chooseCanvasView(page, "Agents");
    assert.equal(await mode.inputValue(), "2", `${engine} persists Narrate`);
    await emitAssistantRecord(worktree, page, lane, `${engine} narrated commentary`);
    await waitForSpeech(page, 1, `${engine} Narrate commentary`);
    assert.equal(await latestSpeechText(page), `${engine} narrated commentary`);
    await settleLatestSpeech(page);
    await mode.fill("1");
    monitor.assertClean();
  } finally {
    await context.close();
    await browser.close();
  }
}

async function emitAssistantRecord(worktree, page, lane, text, { final = false } = {}) {
  const cards = page.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
  );
  const before = await cards.count();
  const message = {
    type: "message",
    role: "assistant",
    content: [{ type: "text", text }],
    usage: {
      input_tokens: 1,
      cache_read_input_tokens: 0,
      cache_creation_input_tokens: 0,
      output_tokens: 1,
    },
  };
  if (final) message.stop_reason = "end_turn";
  await writeFile(join(worktree, ".spice-authored-prose-trigger"), `${JSON.stringify({
    timestamp: new Date().toISOString(),
    type: "assistant",
    message,
  })}\n`);
  await cards.nth(before).waitFor({ timeout: CONNECT_MS });
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
}

function speechCount(page) {
  return page.evaluate(() => window.__speechLog.filter((row) => row.action === "speak").length);
}

function latestSpeechText(page) {
  return page.evaluate(() => window.__speechLog.filter((row) => row.action === "speak").at(-1)?.text);
}

function resetSpeech(page) {
  return page.evaluate(() => {
    window.__speechLog = [];
    window.__speechUtterances = [];
    window.__speechActive = 0;
  });
}

async function waitForSpeech(page, count, label) {
  await waitForPage(page, `${label} dispatches speech`, TRANSITION_MS,
    (expected) => window.__speechLog.filter((row) => row.action === "speak").length === expected,
    count);
}

async function settleLatestSpeech(page) {
  await page.evaluate(() => {
    const utterance = window.__speechUtterances.at(-1);
    if (!utterance?.onend) throw new Error("automatic speech has no completion callback");
    utterance.onend(new Event("end"));
  });
  await waitForPage(page, "automatic speech settles", TRANSITION_MS,
    () => window.__speechActive === 0);
}

export async function assertSpeechVariants(browser, endpoint) {
  const unavailableContext = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  await installBrowserSpeech(unavailableContext, []);
  const unavailablePage = await unavailableContext.newPage();
  const unavailableMonitor = monitorPage(unavailablePage);
  await unavailablePage.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(unavailablePage, "Flow");
  await assertOnlineObserved(unavailablePage, "the page loads with speech unavailable", CONNECT_MS);
  const systemUnavailableButton = unavailablePage.locator(".speech-button");
  await settleSpeechBackend(unavailablePage);
  await systemUnavailableButton.click();
  await settleSpeechBackend(unavailablePage);
  const unavailable = await systemUnavailableButton.evaluate((button) => ({
    state: button.dataset.state,
    title: button.title,
    voices: speechSynthesis.getVoices().map((voice) => voice.name),
    log: window.__speechLog,
  }));
  assert.equal(unavailable.state, "error", `the missing system voice fails: ${JSON.stringify(unavailable)}`);
  assert.match(unavailable.title, /Zoe.*unavailable/);
  unavailableMonitor.assertClean();
  await unavailableContext.close();

  const variants = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  await installBrowserSpeech(variants);

  const disabled = await variants.newPage();
  const disabledMonitor = monitorPage(disabled);
  await routeSpeechBootstrap(disabled, { backend: "disabled" });
  await disabled.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(disabled, "Flow");
  await assertOnlineObserved(disabled, "the page loads with speech disabled", CONNECT_MS);
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
  await assertOnlineObserved(externalUnavailable, "no external backend is offered", CONNECT_MS);
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
  let speechRequest = 0;
  let releaseRender;
  await external.route("**/speech", async (route) => {
    speechRequest += 1;
    submitted = route.request().postDataJSON();
    if (speechRequest === 1) {
      await new Promise((resolve) => { releaseRender = resolve; });
    }
    await route.fulfill({
      status: 200,
      contentType: "audio/wav",
      body: Buffer.from("fixture-audio"),
    });
  });
  await external.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(external, "Flow");
  await assertOnlineObserved(external, "the page loads against the external backend", CONNECT_MS);
  const externalButton = external.locator(".speech-button");
  await settleSpeechBackend(external);
  await externalButton.click();
  await external.locator('.speech-button[data-state="loading"]').waitFor();
  await externalButton.click();
  await external.locator('.speech-button[data-state="idle"]').waitFor();
  const lateResponse = external.waitForResponse((response) =>
    response.url().endsWith("/speech") && response.request().method() === "POST");
  releaseRender();
  await (await lateResponse).finished();
  await external.evaluate(() => new Promise((resolve) =>
    requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.equal(
    await external.evaluate(() => window.__speechLog.some((row) => row.action === "audio")),
    false,
    "stopping an external render prevents its late bytes from creating media",
  );

  await external.evaluate(() => { window.__holdSpeechPlay = true; });
  await externalButton.click();
  await external.locator('.speech-button[data-state="playing"]').waitFor();
  assert.deepEqual(Object.keys(submitted), ["text"]);
  assert.match(submitted.text, /Flow/);
  await externalButton.click();
  await external.locator('.speech-button[data-state="idle"]').waitFor();
  await external.evaluate(() => {
    window.__speechPlayPending.splice(0).forEach(({ resolve }) => resolve());
  });
  await external.evaluate(() => new Promise((resolve) =>
    requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.equal(await externalButton.getAttribute("data-state"), "idle");
  const externalActions = await external.evaluate(() => window.__speechLog.map((row) => row.action));
  assert.ok(externalActions.includes("play"));
  assert.ok(externalActions.includes("pause"));

  await external.evaluate(() => { window.__holdSpeechPlay = false; });
  await externalButton.click();
  await external.locator('.speech-button[data-state="playing"]').waitFor();
  await chooseCanvasView(external, "Agents");
  const team = external.locator(".team-surface").first();
  await team.locator('.team-speech input[type="range"]').fill("1");
  const messageSpeech = team.getByLabel("Speak this message").first();
  await messageSpeech.waitFor();
  await messageSpeech.click();
  const requestsAtPause = speechRequest;
  await external.evaluate(() => {
    const audio = window.__speechAudios.at(-1);
    if (!audio?.onpause) throw new Error("external audio has no pause observer");
    audio.pause();
  });
  await chooseCanvasView(external, "Flow");
  await external.locator('.speech-button[data-state="idle"]').waitFor();
  await external.evaluate(() => new Promise((resolve) =>
    requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.equal(
    speechRequest,
    requestsAtPause,
    "an unmarked media pause clears the queued manual successor",
  );
  externalMonitor.assertClean();
  await external.close();

  const malformed = await variants.newPage();
  const malformedMonitor = monitorPage(malformed);
  await routeSpeechBootstrap(malformed, { backend: "system", voice: "Zoe (Premium)" });
  await malformed.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(malformed, "Chronology");
  await assertOfflineObserved(malformed, "a malformed bootstrap is rejected");
  assert.match(await malformed.locator(".status-copy").innerText(), /bootstrap response was invalid/);
  assert.equal(await malformed.locator(".speech-button").count(), 0);
  assert.match(await malformed.locator(".event.fault").innerText(), /bootstrap response was invalid/);
  malformedMonitor.assertClean();
  await malformed.close();

  await variants.close();
}
