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

/// Room for Canvas's own five-second wait on a browser that publishes no
/// voice list, plus the render that reports it.
const VOICE_DEADLINE_MS = 15_000;

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

export async function installBrowserSpeech(
  context,
  voices = ["Zoe (Premium)"],
  { deferVoices = false } = {},
) {
  await context.addInitScript(({ voiceNames, deferVoices }) => {
    window.__speechLog = [];
    window.__speechUtterances = [];
    window.__speechActive = 0;
    window.__speechMaxActive = 0;
    window.__holdSpeechPlay = false;
    window.__speechPlayPending = [];
    window.__speechAudios = [];
    // A real engine enumerates its voices asynchronously: until it has,
    // getVoices answers an empty list and the engine announces the published
    // list with voiceschanged. `deferVoices` reproduces that first-load
    // window, which is the state a page reaching for a configured voice
    // immediately after load actually observes.
    window.__speechVoicesPublished = !deferVoices;
    window.__publishSpeechVoices = () => {
      window.__speechVoicesPublished = true;
      window.speechSynthesis.onvoiceschanged?.(new Event("voiceschanged"));
    };
    // Report one utterance failure the way the engine does, through the
    // error member of the event handed to the utterance observer.
    window.__failSpeech = (code) => {
      const utterance = window.__speechUtterances.at(-1);
      if (!utterance?.onerror) throw new Error("no system utterance is speaking");
      const event = new Event("error");
      event.error = code;
      utterance.onerror(event);
    };
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
          // A closing page cannot be asked afterwards, so the cancel also
          // leaves a same-origin receipt the next page can read.
          try {
            window.localStorage.setItem(
              "spice-fixture:speech-cancels",
              String(Number(window.localStorage.getItem("spice-fixture:speech-cancels") ?? 0) + 1),
            );
          } catch {
            // Storage may be unavailable while the document unloads.
          }
        },
        onvoiceschanged: null,
        getVoices() {
          const published = window.__speechVoicesPublished;
          // Only the initialization fixture records reads: the log is a
          // playback log, every other scenario asserts it whole, and asking
          // for the voice list is not playback.
          if (deferVoices) window.__speechLog.push({ action: "voices", published });
          if (!published) return [];
          return voiceNames.map((name) => new FakeVoice(name));
        },
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
  }, { voiceNames: voices, deferVoices });
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

/// The evidenced cause of intermittent configured-voice unavailability.
///
/// A page that reads `getVoices` once, immediately after load, reads it
/// before the engine has enumerated its voices: the list is empty and the
/// installed configured voice looks absent. The names arrive with
/// `voiceschanged`, so the held request is the one that speaks, and it
/// speaks exactly once with the configured voice. That is a different cause
/// from a published list that omits the name, which `assertSpeechVariants`
/// covers and which refuses immediately.
export async function assertDeferredVoicePublication(browser, endpoint) {
  const context = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  await installBrowserSpeech(context, ["Zoe (Premium)"], { deferVoices: true });

  const page = await context.newPage();
  const monitor = monitorPage(page);
  await page.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(page, "Work");
  await assertOnlineObserved(page, "the page loads before voices are published", CONNECT_MS);
  await chooseCanvasView(page, "Chat");
  const card = page.locator('[data-card-kind="agent-message"]').first();
  await card.waitFor();
  await card.getByLabel("Speak this message").click();
  await settleSpeechBackend(page);
  assert.deepEqual(
    await page.evaluate(() => window.__speechLog),
    [{ action: "voices", published: false }],
    "the request reads an unpublished list and starts nothing on it",
  );
  assert.equal(await page.locator(".form-refusal").count(), 0,
    "an unpublished voice list refuses nothing");
  assert.equal(await card.getAttribute("data-speech-active"), "true",
    "the held request remains the marked queue head");

  await page.evaluate(() => window.__publishSpeechVoices());
  await waitForPage(page, "the published list starts the held utterance", CONNECT_MS,
    () => window.__speechLog.filter((row) => row.action === "speak").length === 1);
  assert.deepEqual(
    await page.evaluate(() => window.__speechLog
      .filter((row) => row.action === "speak")
      .map((row) => row.voice)),
    ["Zoe (Premium)"],
    "the held request speaks once, with the configured voice",
  );
  assert.deepEqual(
    await page.evaluate(() => window.__speechLog
      .filter((row) => row.action === "voices")
      .map((row) => row.published)),
    [false, true],
    "the same request resolves its voice on the published list it first read empty",
  );
  const voiceLine = page.locator("[data-speech-voice]");
  await voiceLine.waitFor();
  assert.equal(await voiceLine.getAttribute("data-speech-voice"), "Zoe (Premium)",
    "Canvas names the voice the browser reported for the utterance");
  assert.equal(await voiceLine.innerText(), "Speaking as Zoe (Premium)");

  await page.evaluate(() => window.__publishSpeechVoices());
  await settleSpeechBackend(page);
  assert.equal(
    await page.evaluate(() => window.__speechLog.filter((row) => row.action === "speak").length),
    1,
    "a repeated publication speaks nothing further",
  );

  await page.evaluate(() => window.__failSpeech("synthesis-failed"));
  await page.locator(".form-refusal").waitFor();
  assert.equal(
    await page.locator(".form-refusal").innerText(),
    "system speech playback failed: synthesis-failed",
    "the browser's own error code reaches the operator",
  );
  assert.equal(await page.locator("[data-speech-voice]").count(), 0,
    "a finished utterance names no speaking voice");
  monitor.assertClean();
  await page.close();

  const cancelled = await context.newPage();
  const cancelledMonitor = monitorPage(cancelled);
  await cancelled.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(cancelled, "Work");
  await assertOnlineObserved(cancelled, "a second page loads before voices", CONNECT_MS);
  await chooseCanvasView(cancelled, "Chat");
  const cancelledCard = cancelled.locator('[data-card-kind="agent-message"]').first();
  await cancelledCard.waitFor();
  await cancelledCard.getByLabel("Speak this message").click();
  await settleSpeechBackend(cancelled);
  await cancelledCard.getByLabel("Stop playback").click();
  await settleSpeechBackend(cancelled);
  await cancelled.evaluate(() => window.__publishSpeechVoices());
  await settleSpeechBackend(cancelled);
  assert.equal(
    await cancelled.evaluate(() => window.__speechLog.some((row) => row.action === "speak")),
    false,
    "a stopped request cannot be started by a later publication",
  );
  assert.equal(await cancelledCard.getAttribute("data-speech-active"), "false");
  cancelledMonitor.assertClean();
  await cancelled.close();

  await context.close();
}



export async function assertSpeechVariants(browser, endpoint) {
  // A published list that omits the configured voice is a naming mismatch:
  // the request is refused at once, no other voice is substituted, and the
  // refusal names what this browser does offer.
  const mismatchContext = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  await installBrowserSpeech(mismatchContext, ["Daniel", "Karen", "Alex"]);
  const mismatchPage = await mismatchContext.newPage();
  const mismatchMonitor = monitorPage(mismatchPage);
  await mismatchPage.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(mismatchPage, "Work");
  await assertOnlineObserved(mismatchPage, "the page loads with another voice", CONNECT_MS);
  await chooseCanvasView(mismatchPage, "Chat");
  const mismatchCard = mismatchPage.locator('[data-card-kind="agent-message"]').first();
  await mismatchCard.waitFor();
  await mismatchCard.getByLabel("Speak this message").click();
  await settleSpeechBackend(mismatchPage);
  assert.equal(await mismatchCard.getAttribute("data-speech-active"), "false");
  assert.equal(
    await mismatchPage.evaluate(() => window.__speechLog.some((row) => row.action === "speak")),
    false,
    "a missing configured voice cannot start a message",
  );
  assert.equal(
    await mismatchPage.locator(".form-refusal").innerText(),
    'system voice "Zoe (Premium)" is unavailable; this browser offers Daniel, Karen, Alex',
    "the refusal names the voices this browser published",
  );
  assert.equal(await mismatchPage.locator(".speech-button").count(), 0,
    "Canvas exposes no general-purpose page reader");
  mismatchMonitor.assertClean();
  await mismatchContext.close();

  // An empty list is the pre-publication state, so the request waits for
  // voiceschanged rather than refusing. An engine carrying no voices never
  // announces one, and the bounded wait is what still ends that request.
  const unavailableContext = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  await installBrowserSpeech(unavailableContext, []);
  const unavailablePage = await unavailableContext.newPage();
  const unavailableMonitor = monitorPage(unavailablePage);
  await unavailablePage.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(unavailablePage, "Work");
  await assertOnlineObserved(unavailablePage, "the page loads with speech unavailable", CONNECT_MS);
  await chooseCanvasView(unavailablePage, "Chat");
  const unavailableCard = unavailablePage.locator('[data-card-kind="agent-message"]').first();
  await unavailableCard.waitFor();
  await unavailableCard.getByLabel("Speak this message").click();
  await settleSpeechBackend(unavailablePage);
  assert.equal(
    await unavailableCard.getAttribute("data-speech-active"),
    "true",
    "an unpublished voice list holds the request rather than refusing it",
  );
  await unavailablePage.locator(".form-refusal").waitFor({ timeout: VOICE_DEADLINE_MS });
  assert.equal(
    await unavailablePage.locator(".form-refusal").innerText(),
    "this browser published no system voices",
  );
  assert.equal(await unavailableCard.getAttribute("data-speech-active"), "false");
  assert.equal(
    await unavailablePage.evaluate(() => window.__speechLog.some((row) => row.action === "speak")),
    false,
    "a browser with no voices speaks nothing",
  );
  unavailableMonitor.assertClean();
  await unavailableContext.close();

  const variants = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  await installBrowserSpeech(variants);

  const disabled = await variants.newPage();
  const disabledMonitor = monitorPage(disabled);
  await routeSpeechBootstrap(disabled, { backend: "disabled" });
  await disabled.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(disabled, "Work");
  await assertOnlineObserved(disabled, "the page loads with speech disabled", CONNECT_MS);
  await chooseCanvasView(disabled, "Chat");
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
  await chooseCanvasView(externalUnavailable, "Work");
  await assertOnlineObserved(externalUnavailable, "no external backend is offered", CONNECT_MS);
  await chooseCanvasView(externalUnavailable, "Chat");
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
  await chooseCanvasView(external, "Work");
  await assertOnlineObserved(external, "the page loads against the external backend", CONNECT_MS);
  await chooseCanvasView(external, "Chat");
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

  await assertSpeechStopsWithThePage(variants, endpoint);

  const malformed = await variants.newPage();
  const malformedMonitor = monitorPage(malformed);
  await routeSpeechBootstrap(malformed, { backend: "system", voice: "Zoe (Premium)" });
  await malformed.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(malformed, "Work");
  await assertOfflineObserved(malformed, "a malformed bootstrap is rejected");
  assert.match(await malformed.locator(".status-copy").innerText(), /bootstrap response was invalid/);
  assert.equal(await malformed.locator(".speech-button").count(), 0);
  malformedMonitor.assertClean();
  await malformed.close();

  await variants.close();
}

/// Closing the page that queued a system utterance cancels it, so speech
/// never continues out of the operating system speech server after the
/// Canvas window is gone.
async function assertSpeechStopsWithThePage(context, endpoint) {
  const closing = await context.newPage();
  const closingMonitor = monitorPage(closing);
  await closing.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(closing, "Work");
  await assertOnlineObserved(closing, "the page loads with system speech", CONNECT_MS);
  await chooseCanvasView(closing, "Chat");
  const card = closing.locator('[data-card-kind="agent-message"]').first();
  await card.waitFor();
  await card.getByLabel("Speak this message").click();
  await waitForPage(closing, "system speech starts before the page closes", CONNECT_MS,
    () => window.__speechActive === 1);
  await closing.evaluate(() => {
    window.localStorage.removeItem("spice-fixture:speech-cancels");
  });
  assert.equal(
    await closing.evaluate(() => window.__speechLog.filter((row) => row.action === "cancel").length),
    0,
    "starting one utterance cancels nothing",
  );
  closingMonitor.assertClean();
  await closing.close();

  const successor = await context.newPage();
  const successorMonitor = monitorPage(successor);
  await successor.goto(endpoint, { waitUntil: "networkidle" });
  assert.equal(
    await successor.evaluate(() => window.localStorage.getItem("spice-fixture:speech-cancels")),
    "1",
    "closing the page cancelled its in-flight system utterance exactly once",
  );
  await successor.evaluate(() => {
    window.localStorage.removeItem("spice-fixture:speech-cancels");
  });
  successorMonitor.assertClean();
  await successor.close();
}
