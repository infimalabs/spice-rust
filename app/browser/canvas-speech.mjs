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

export async function assertAutomaticSpeechEngines(endpoint, fixture, worktree, lane, artifacts) {
  for (const engine of ["chromium", "firefox"]) {
    await assertAutomaticSpeechEngine(engine, endpoint, fixture, worktree, lane, artifacts);
  }
}

async function assertAutomaticSpeechEngine(engine, endpoint, fixture, worktree, lane, artifacts) {
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

    const marker = engine === "chromium" ? "C" : "F";
    const finalText = [
      `Opened [the ${marker} final](https://example.test).`,
      "middle stays silent",
      "Landed 7a12bc99.",
    ].join("\n\n");
    const finalCard = await emitAssistantRecord(worktree, page, lane, finalText, { final: true });
    await waitForSpeech(page, 1, `${engine} final answer`);
    assert.equal(
      await latestSpeechText(page),
      `Opened the ${marker} final. Landed commit 7 a 1 2 b c 9.`,
    );
    await assertFinalCardPresentation(finalCard, `${engine} playing final`, true);
    await settleLatestSpeech(page);
    await assertFinalCardMatrix(page, surface, finalCard, engine, artifacts);

    await page.reload({ waitUntil: "networkidle" });
    await chooseCanvasView(page, "Agents");
    assert.equal(await speechCount(page), 0, `${engine} reload replays no speech`);
    assert.equal(await mode.inputValue(), "1", `${engine} retains default Speak after reload`);
    const replayedFinal = page.locator(
      `[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
      { hasText: `Opened the ${marker} final` },
    );
    await assertFinalCardPresentation(replayedFinal, `${engine} replayed final`, false);

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
  return cards.nth(before);
}

async function assertFinalCardPresentation(card, label, nowPlaying) {
  const presentation = await card.evaluate((element) => {
    const style = getComputedStyle(element);
    const bounds = element.getBoundingClientRect();
    const painted = [...element.querySelectorAll(".message-body > *, .message-footer")]
      .map((child) => child.getBoundingClientRect())
      .filter((child) => child.width > 0 || child.height > 0);
    const probe = document.createElement("span");
    element.append(probe);
    const color = (value) => {
      probe.style.background = value;
      const resolved = getComputedStyle(probe).backgroundColor;
      probe.style.background = "";
      return resolved;
    };
    const measured = {
      final: element.dataset.messageFinal,
      finalClass: element.classList.contains("final"),
      nowPlaying: element.classList.contains("now-playing"),
      background: style.backgroundColor,
      expectedBackground: color("color-mix(in srgb, var(--accent-1) 8%, var(--panel))"),
      panel: color("var(--panel)"),
      borderTop: style.borderTopColor,
      accent: color("var(--accent-1)"),
      borderRight: style.borderRightColor,
      producer: color("var(--accent-slot, var(--accent))"),
      shadow: style.boxShadow,
      warning: color("var(--warning)"),
      playingBorder: color("color-mix(in srgb, var(--warning) 70%, var(--line))"),
      contentBottom: painted.length > 0 ? Math.max(...painted.map((child) => child.bottom)) : null,
      innerBottom: bounds.bottom - Number.parseFloat(style.borderBottomWidth),
    };
    probe.remove();
    return measured;
  });
  assert.equal(presentation.final, "true", `${label} exposes final state`);
  assert.equal(presentation.finalClass, true, `${label} carries the final class`);
  assert.equal(presentation.nowPlaying, nowPlaying, `${label} composes playback state`);
  assert.equal(
    presentation.background,
    presentation.expectedBackground,
    `${label} uses the accent-tinted final surface`,
  );
  assert.notEqual(presentation.background, presentation.panel, `${label} is not a neutral card`);
  assert.ok(
    presentation.contentBottom === null || presentation.contentBottom <= presentation.innerBottom + 0.5,
    `${label} contains its painted content: ${JSON.stringify(presentation)}`,
  );
  assert.equal(
    presentation.borderTop,
    nowPlaying ? presentation.playingBorder : presentation.accent,
    `${label} keeps the expected border state`,
  );
  assert.equal(
    presentation.borderRight,
    presentation.producer,
    `${label} preserves the producer accent on the right edge`,
  );
  assert.ok(
    presentation.shadow.includes(presentation.accent),
    `${label} retains the inset final top rule: ${presentation.shadow}`,
  );
  if (nowPlaying) {
    assert.ok(
      presentation.shadow.includes(presentation.warning),
      `${label} retains the playback edge beside the final top rule: ${presentation.shadow}`,
    );
  }
}

async function waitForCommittedResize(page, surface, viewport, label) {
  const current = page.viewportSize();
  if (current?.width === viewport.width && current?.height === viewport.height) {
    await settleSpeechBackend(page);
    return;
  }
  const priorWidths = await surface.locator(".lane-mosaic-plane").evaluateAll(
    (planes) => planes.map((plane) => Number(plane.dataset.committedWidth)),
  );
  await page.setViewportSize(viewport);
  await waitForPage(page, `${label} mosaic width commits`, TRANSITION_MS, (before) => {
    const planes = [...document.querySelectorAll(
      '[data-pane="agent-console"] .lane-mosaic-plane',
    )];
    return planes.length === before.length && planes.every((plane, index) => (
      plane.dataset.measurementPhase === "ready"
      && Number(plane.dataset.committedWidth) !== before[index]
      && Math.abs(Number(plane.dataset.committedWidth) - plane.clientWidth) <= 1
    ));
  }, priorWidths);
}

async function assertFinalCardMatrix(page, surface, card, engine, artifacts) {
  const originalViewport = page.viewportSize();
  for (const mode of ["light", "dark"]) {
    await page.emulateMedia({ colorScheme: mode });
    for (const [width, height, size] of [[1000, 720, "wide"], [390, 720, "narrow"]]) {
      await waitForCommittedResize(page, surface, { width, height }, `${engine} ${mode} ${size}`);
      await assertFinalCardPresentation(card, `${engine} ${mode} ${size} final`, false);
      await card.screenshot({
        path: join(artifacts, `canvas-final-${engine}-${mode}-${size}.png`),
      });
    }
  }
  await page.emulateMedia({ colorScheme: "light" });
  await waitForCommittedResize(page, surface, originalViewport, `${engine} restored`);
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
  await chooseCanvasView(unavailablePage, "Chronology");
  await assertOnlineObserved(unavailablePage, "the page loads with speech unavailable", CONNECT_MS);
  await chooseCanvasView(unavailablePage, "Agents");
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
  await chooseCanvasView(disabled, "Chronology");
  await assertOnlineObserved(disabled, "the page loads with speech disabled", CONNECT_MS);
  await chooseCanvasView(disabled, "Agents");
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
  await chooseCanvasView(externalUnavailable, "Chronology");
  await assertOnlineObserved(externalUnavailable, "no external backend is offered", CONNECT_MS);
  await chooseCanvasView(externalUnavailable, "Agents");
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
  await chooseCanvasView(external, "Chronology");
  await assertOnlineObserved(external, "the page loads against the external backend", CONNECT_MS);
  await chooseCanvasView(external, "Agents");
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
  await chooseCanvasView(malformed, "Chronology");
  await assertOfflineObserved(malformed, "a malformed bootstrap is rejected");
  assert.match(await malformed.locator(".status-copy").innerText(), /bootstrap response was invalid/);
  assert.equal(await malformed.locator(".speech-button").count(), 0);
  assert.match(await malformed.locator(".event.fault").innerText(), /bootstrap response was invalid/);
  malformedMonitor.assertClean();
  await malformed.close();

  await variants.close();
}
