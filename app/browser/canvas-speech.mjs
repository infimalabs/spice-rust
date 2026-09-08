// Speech installation and backend scenarios for the Canvas browser suite.
import assert from "node:assert/strict";

import {
  assertOfflineObserved,
  assertOnlineObserved,
  chooseCanvasView,
  CONNECT_MS,
  monitorPage,
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


export async function assertSpeechVariants(browser, endpoint) {
  const unavailableContext = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  await installBrowserSpeech(unavailableContext, []);
  const unavailablePage = await unavailableContext.newPage();
  const unavailableMonitor = monitorPage(unavailablePage);
  await unavailablePage.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(unavailablePage, "Flow");
  await assertOnlineObserved(unavailablePage, "the page loads with speech unavailable", CONNECT_MS);
  await chooseCanvasView(unavailablePage, "Lanes");
  const unavailableCard = unavailablePage.locator('[data-card-kind="agent-message"]').first();
  await unavailableCard.waitFor();
  await unavailableCard.getByLabel("Speak this message").click();
  await settleSpeechBackend(unavailablePage);
  assert.equal(await unavailableCard.getAttribute("data-speech-active"), "false");
  assert.equal(
    await unavailablePage.evaluate(() => window.__speechLog.some((row) => row.action === "speak")),
    false,
    "a missing configured voice cannot start a message",
  );
  assert.equal(await unavailablePage.locator(".speech-button").count(), 0,
    "Canvas exposes no general-purpose page reader");
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
  await chooseCanvasView(disabled, "Lanes");
  const disabledCard = disabled.locator('[data-card-kind="agent-message"]').first();
  await disabledCard.waitFor();
  await disabledCard.getByLabel("Speak this message").click();
  await settleSpeechBackend(disabled);
  assert.equal(await disabledCard.getAttribute("data-speech-active"), "false");
  assert.deepEqual(await disabled.evaluate(() => window.__speechLog), []);
  assert.equal(await disabled.locator(".speech-button").count(), 0);
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
  await chooseCanvasView(externalUnavailable, "Lanes");
  let unavailableRequests = 0;
  await externalUnavailable.route("**/speech", async (route) => {
    unavailableRequests += 1;
    await route.abort();
  });
  const externalUnavailableCard = externalUnavailable
    .locator('[data-card-kind="agent-message"]').first();
  await externalUnavailableCard.waitFor();
  await externalUnavailableCard.getByLabel("Speak this message").click();
  await settleSpeechBackend(externalUnavailable);
  assert.equal(unavailableRequests, 0, "an unavailable backend receives no message render request");
  assert.equal(await externalUnavailableCard.getAttribute("data-speech-active"), "false");
  assert.equal(await externalUnavailable.locator(".speech-button").count(), 0);
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
  let signalRenderStarted;
  const renderStarted = new Promise((resolve) => { signalRenderStarted = resolve; });
  await external.route("**/speech", async (route) => {
    speechRequest += 1;
    submitted = route.request().postDataJSON();
    if (speechRequest === 1) {
      signalRenderStarted();
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
  await chooseCanvasView(external, "Lanes");
  const card = external.locator('[data-card-kind="agent-message"]').first();
  await card.waitFor();
  const cardKey = await card.getAttribute("data-mosaic-card-key");
  const waitForCardState = (active) => external.waitForFunction(({ key, active }) => (
    document.querySelector(`[data-mosaic-card-key="${key}"]`)?.dataset.speechActive
      === String(active)
  ), { key: cardKey, active });
  await card.getByLabel("Speak this message").click();
  await waitForCardState(true);
  await renderStarted;
  const stopIcon = card.locator('[data-card-action-icon="stop"]');
  assert.equal(await stopIcon.count(), 1);
  assert.deepEqual(
    await stopIcon.evaluate((icon) => {
      const box = icon.getBoundingClientRect();
      return [box.width, box.height];
    }),
    [15, 15],
    "the stop icon retains the predecessor's 15px viewport",
  );
  await card.getByLabel("Stop playback").click();
  await waitForCardState(false);
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
  await card.getByLabel("Speak this message").click();
  await waitForCardState(true);
  await external.waitForFunction(() => window.__speechPlayPending.length === 1);
  assert.deepEqual(Object.keys(submitted), ["text"]);
  const normalizeSpeechText = (text) => text.replaceAll("`", "").replace(/\s+/g, " ").trim();
  assert.equal(
    normalizeSpeechText(submitted.text),
    normalizeSpeechText(await card.locator(".message-body").innerText()),
    "the backend receives this message card rather than page text",
  );
  await card.getByLabel("Stop playback").click();
  await waitForCardState(false);
  await external.evaluate(() => {
    window.__speechPlayPending.splice(0).forEach(({ resolve }) => resolve());
  });
  await external.evaluate(() => new Promise((resolve) =>
    requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.equal(await card.getAttribute("data-speech-active"), "false");
  const playIcon = card.locator('[data-card-action-icon="play"]');
  assert.equal(await playIcon.count(), 1);
  assert.deepEqual(
    await playIcon.evaluate((icon) => {
      const box = icon.getBoundingClientRect();
      return [box.width, box.height];
    }),
    [15, 15],
    "the play icon retains the predecessor's 15px viewport",
  );
  const externalActions = await external.evaluate(() => window.__speechLog.map((row) => row.action));
  assert.ok(externalActions.includes("play"));
  assert.ok(externalActions.includes("pause"));

  await external.evaluate(() => { window.__holdSpeechPlay = false; });
  await card.getByLabel("Speak this message").click();
  await waitForCardState(true);
  await external.waitForFunction(() => typeof window.__speechAudios.at(-1)?.onpause === "function");
  const speaking = await card.evaluate((article) => {
    const probe = document.createElement("span");
    probe.style.color = "color-mix(in srgb, var(--warning) 70%, var(--line))";
    article.append(probe);
    const expectedBorder = getComputedStyle(probe).color;
    probe.style.color = "var(--warning)";
    const expectedAccent = getComputedStyle(probe).color;
    probe.remove();
    const style = getComputedStyle(article);
    return {
      active: article.dataset.speechActive,
      borderLeft: style.borderLeftColor,
      boxShadow: style.boxShadow,
      expectedAccent,
      expectedBorder,
    };
  });
  assert.equal(speaking.active, "true", "the active queue head marks its exact message card");
  assert.equal(speaking.borderLeft, speaking.expectedBorder, "message speech uses the warm border");
  assert.ok(speaking.boxShadow.includes(speaking.expectedAccent), "message speech paints the warm inset rule");
  const requestsAtPause = speechRequest;
  await external.evaluate(() => {
    const audio = window.__speechAudios.at(-1);
    if (!audio?.onpause) throw new Error("message audio has no pause observer");
    audio.pause();
  });
  await waitForCardState(false);
  assert.equal(
    await card.getAttribute("data-speech-active"),
    "false",
    "an external stop clears the active message marker",
  );
  assert.equal(speechRequest, requestsAtPause, "an external stop starts no successor");
  assert.equal(await external.locator(".speech-button").count(), 0,
    "the external backend remains card-only");
  externalMonitor.assertClean();
  await external.close();

  const malformed = await variants.newPage();
  const malformedMonitor = monitorPage(malformed);
  await routeSpeechBootstrap(malformed, { backend: "system", voice: "Zoe (Premium)" });
  await malformed.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(malformed, "Flow");
  await assertOfflineObserved(malformed, "a malformed bootstrap is rejected");
  assert.match(await malformed.locator(".status-copy").innerText(), /bootstrap response was invalid/);
  assert.equal(await malformed.locator(".speech-button").count(), 0);
  malformedMonitor.assertClean();
  await malformed.close();

  await variants.close();
}
