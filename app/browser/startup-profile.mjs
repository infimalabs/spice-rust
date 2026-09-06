import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { mkdtemp, realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { seatAgentTeam } from "./agent-conversation.mjs";
import { longWorktreeBasename } from "./agent-conversation.mjs";
import { settleMosaicActivity } from "./agent-conversation-mosaic.mjs";
import {
  buildResident,
  chooseCanvasView,
  configureFixture,
  monitorPage,
  seedAuthority,
  startResident,
} from "./canvas-fixture.mjs";
import { ownFixture } from "./canvas-lifecycle.mjs";
import { describeLoad } from "./canvas-load.mjs";
import { installSocketControl } from "./canvas-speech.mjs";
import { launchBrowser } from "./headless-launch.mjs";

const PROFILE_TIMEOUT_MS = 60_000;
const RESIZE_FRAME_BUDGET_MS = 2_000;
const RESIZE_VIEWPORTS = Object.freeze([
  { label: "reported-1", width: 480, height: 468 },
  { label: "wide-1", width: 1440, height: 900 },
  { label: "narrow-1", width: 390, height: 720 },
  { label: "wide-2", width: 1440, height: 900 },
  { label: "reported-2", width: 480, height: 468 },
  { label: "wide-3", width: 1440, height: 900 },
]);

function decode(message) {
  return JSON.parse(typeof message === "string" ? message : message.toString("utf8"));
}

async function installSnapshotRoute(context) {
  let activeSocket;
  let sourceFrame;
  const prose = new Map();
  let inboundFrames = 0;
  await context.routeWebSocket(/.*/, (socket) => {
    activeSocket = socket;
    const server = socket.connectToServer();
    server.onMessage((message) => {
      inboundFrames += 1;
      const frame = decode(message);
      sourceFrame ??= frame.stream === "messages" ? frame : sourceFrame;
      if (frame.stream === "messages") {
        for (const message of frame.messages) {
          if (message.activity !== "tool") prose.set(message.lane, message);
        }
      }
      if (frame.stream === "message" && frame.message?.activity !== "tool") {
        prose.set(frame.message.lane, frame.message);
      }
      socket.send(message);
    });
  });
  return {
    frameCount: () => inboundFrames,
    makeSnapshot(perLane) {
      const sources = [...prose.values()].slice(0, 3);
      assert.equal(sources.length, 3, "three live agents supplied profile message shapes");
      const base = Date.parse("2034-01-01T00:00:00.000Z");
      const messages = [];
      for (let index = 0; index < perLane; index += 1) {
        for (const [laneIndex, source] of sources.entries()) {
          const message = structuredClone(source);
          const ordinal = index * sources.length + laneIndex;
          message.text = `Profile lane ${laneIndex + 1} retained record ${index + 1}`;
          message.activity = "prose";
          message.media = [];
          message.authored = new Date(base + ordinal).toISOString();
          message.authored_synthetic = false;
          message.observed = new Date(base + ordinal).toISOString();
          message.locus = {
            member: source.locus.member,
            offset: 100_000 + ordinal * 100,
            length: message.text.length + 1,
            event: 90_000 + ordinal,
          };
          messages.push(message);
        }
      }
      return messages;
    },
    send(messages) {
      assert.ok(activeSocket, "the browser has an active routed socket");
      assert.ok(sourceFrame, "the resident supplied the retained-snapshot envelope");
      activeSocket.send(Buffer.from(JSON.stringify({
        stream: "messages",
        v: sourceFrame.v,
        id: sourceFrame.id,
        epoch: sourceFrame.epoch,
        limit: 256,
        messages,
      })));
    },
  };
}

async function armProfile(page) {
  await page.evaluate(() => {
    const started = performance.now();
    const record = {
      started,
      frames: 0,
      maxFrameGap: 0,
      addedCards: 0,
      removedCards: 0,
      commitMutations: 0,
      placementWrites: 0,
      placementWriteCards: new Set(),
      longTasks: [],
    };
    let previous = started;
    const tick = (now) => {
      record.frames += 1;
      record.maxFrameGap = Math.max(record.maxFrameGap, now - previous);
      previous = now;
      record.frame = requestAnimationFrame(tick);
    };
    record.frame = requestAnimationFrame(tick);
    record.observer = new MutationObserver((mutations) => {
      for (const mutation of mutations) {
        if (mutation.type === "attributes" && mutation.attributeName === "data-plane-commits") {
          record.commitMutations += 1;
        }
        if (
          mutation.type === "attributes"
          && mutation.attributeName === "style"
          && mutation.target.matches(".mosaic-message-card")
        ) {
          record.placementWrites += 1;
          record.placementWriteCards.add(mutation.target.dataset.mosaicCardKey);
        }
        for (const node of mutation.addedNodes) {
          if (!(node instanceof Element)) continue;
          record.addedCards += Number(node.matches(".mosaic-message-card"));
          record.addedCards += node.querySelectorAll(".mosaic-message-card").length;
        }
        for (const node of mutation.removedNodes) {
          if (!(node instanceof Element)) continue;
          record.removedCards += Number(node.matches(".mosaic-message-card"));
          record.removedCards += node.querySelectorAll(".mosaic-message-card").length;
        }
      }
    });
    record.observer.observe(document.body, {
      attributes: true,
      attributeFilter: ["data-plane-commits", "style"],
      childList: true,
      subtree: true,
    });
    if (globalThis.PerformanceObserver?.supportedEntryTypes?.includes("longtask")) {
      record.longTaskObserver = new PerformanceObserver((list) => {
        record.longTasks.push(...list.getEntries()
          .filter((entry) => entry.startTime >= started)
          .map((entry) => entry.duration));
      });
      record.longTaskObserver.observe({ type: "longtask", buffered: true });
    }
    window.__startupProfile = record;
  });
}

async function finishProfile(page) {
  return page.evaluate(() => {
    const record = window.__startupProfile;
    cancelAnimationFrame(record.frame);
    record.observer.disconnect();
    record.longTaskObserver?.disconnect();
    const planes = [...document.querySelectorAll(".lane-mosaic-plane")];
    return {
      elapsed: performance.now() - record.started,
      frames: record.frames,
      maxFrameGap: record.maxFrameGap,
      addedCards: record.addedCards,
      removedCards: record.removedCards,
      commitMutations: record.commitMutations,
      placementWrites: record.placementWrites,
      placementWriteCards: record.placementWriteCards.size,
      longTasks: record.longTasks,
      cards: document.querySelectorAll(".mosaic-message-card").length,
      commits: planes.map((plane) => Number(plane.dataset.planeCommits)),
      reads: [...document.querySelectorAll(".mosaic-message-card")]
        .reduce((sum, card) => sum + Number(card.dataset.measurementReads), 0),
      keys: [...document.querySelectorAll(".mosaic-message-card")]
        .map((card) => card.dataset.mosaicCardKey),
    };
  });
}

async function profileResizeTransition(page, viewport) {
  const plane = page.locator(".lane-mosaic-plane");
  const before = await plane.evaluate((element) => ({
    commits: Number(element.dataset.planeCommits),
    committedWidth: Number(element.dataset.committedWidth),
    cards: [...element.querySelectorAll(".mosaic-message-card")].map((card) => {
      card.dataset.resizeProfileIdentity ??= crypto.randomUUID();
      return {
        identity: card.dataset.resizeProfileIdentity,
        key: card.dataset.mosaicCardKey,
      };
    }),
  }));
  const token = await plane.evaluate((element, config) => {
    const token = crypto.randomUUID();
    const started = performance.now();
    let resolve;
    const completed = new Promise((done) => { resolve = done; });
    const observer = new MutationObserver(() => {
      const committedWidth = Number(element.dataset.committedWidth);
      if (committedWidth === config.committedWidth) return;
      resolve({
        committedWidth,
        completionMs: performance.now() - started,
        observedWidth: Number(element.dataset.observedWidth),
      });
    });
    observer.observe(element, {
      attributes: true,
      attributeFilter: ["data-committed-width"],
    });
    element.__resizeProfile = { completed, observer, token };
    return token;
  }, before);
  await armProfile(page);
  await page.setViewportSize({ width: viewport.width, height: viewport.height });
  const completion = await plane.evaluate((element, config) => {
    const probe = element.__resizeProfile;
    if (!probe || probe.token !== config.token) {
      throw new Error("the armed resize profile changed before completion");
    }
    let deadline;
    return Promise.race([
      probe.completed,
      new Promise((_, reject) => {
        deadline = setTimeout(
          () => reject(new Error(`Mosaic did not commit ${config.label}`)),
          config.deadlineMs,
        );
      }),
    ]).finally(() => {
      clearTimeout(deadline);
      probe.observer.disconnect();
      delete element.__resizeProfile;
    });
  }, { deadlineMs: PROFILE_TIMEOUT_MS, label: viewport.label, token });
  await settleMosaicActivity(page, plane);
  const responsiveness = await finishProfile(page);
  const after = await plane.evaluate((element) => ({
    commits: Number(element.dataset.planeCommits),
    committedWidth: Number(element.dataset.committedWidth),
    measurementPending: Number(element.dataset.measurementPending),
    measurementPhase: element.dataset.measurementPhase,
    cards: [...element.querySelectorAll(".mosaic-message-card")].map((card) => ({
      identity: card.dataset.resizeProfileIdentity,
      key: card.dataset.mosaicCardKey,
      layoutPhase: card.dataset.layoutPhase,
    })),
  }));
  assert.equal(after.commits, before.commits + 1, `${viewport.label} commits once`);
  assert.notEqual(after.committedWidth, before.committedWidth, `${viewport.label} changes width`);
  assert.equal(after.measurementPending, 0, `${viewport.label} leaves no pending measurement`);
  assert.equal(after.measurementPhase, "ready", `${viewport.label} leaves Mosaic ready`);
  assert.deepEqual(
    after.cards.map(({ identity, key }) => ({ identity, key })),
    before.cards,
    `${viewport.label} retains card nodes and chronology`,
  );
  assert.ok(
    after.cards.every((card) => card.layoutPhase === "committed"),
    `${viewport.label} leaves every placement committed`,
  );
  assert.deepEqual(page.viewportSize(), {
    width: viewport.width,
    height: viewport.height,
  }, `${viewport.label} reaches its viewport`);
  assert.equal(responsiveness.commitMutations, 1, `${viewport.label} exposes one commit mutation`);
  assert.equal(responsiveness.addedCards, 0, `${viewport.label} adds no cards`);
  assert.equal(responsiveness.removedCards, 0, `${viewport.label} removes no cards`);
  delete responsiveness.keys;
  return {
    label: viewport.label,
    viewport: { width: viewport.width, height: viewport.height },
    ...completion,
    ...responsiveness,
    commitsBefore: before.commits,
    commitsAfter: after.commits,
  };
}

async function profileResizeTransitions(page) {
  const transitions = [];
  for (const viewport of RESIZE_VIEWPORTS) {
    transitions.push(await profileResizeTransition(page, viewport));
  }
  return transitions;
}

async function waitForSettled(page, count) {
  await page.waitForFunction((expected) => {
    const planes = [...document.querySelectorAll(".lane-mosaic-plane")];
    return document.querySelectorAll(".mosaic-message-card").length === expected
      && planes.length === 1
      && planes.every((plane) => plane.dataset.measurementPending === "0"
        && plane.dataset.measurementPhase === "ready");
  }, count, { timeout: PROFILE_TIMEOUT_MS });
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
}

async function profileSnapshot(page, route, perLane) {
  route.send([]);
  await page.waitForFunction(() => document.querySelectorAll(".mosaic-message-card").length === 0);
  const messages = route.makeSnapshot(perLane);
  await armProfile(page);
  route.send(messages);
  await waitForSettled(page, messages.length);
  const result = await finishProfile(page);
  const expected = messages.map((message) => [
    "message-agent",
    message.lane,
    message.locus.member,
    message.locus.offset,
    message.locus.length,
    message.locus.event,
  ].join("-"));
  assert.deepEqual(result.keys, expected, "the profile snapshot keeps transport chronology");
  result.firstKey = result.keys.at(0);
  result.lastKey = result.keys.at(-1);
  delete result.keys;
  return result;
}

async function profileTabSwitch(page, route) {
  const beforeFrames = route.frameCount();
  const token = await page.locator(".lane-mosaic-plane").evaluate((plane) => {
    plane.dataset.profileIdentity = crypto.randomUUID();
    return plane.dataset.profileIdentity;
  });
  await armProfile(page);
  const console = page.locator('[data-pane="agent-console"]');
  await console.getByRole("tab", { name: /^Tasks/ }).click();
  await console.getByRole("tab", { name: /^Agents/ }).click();
  await waitForSettled(page, 768);
  const result = await finishProfile(page);
  delete result.keys;
  return {
    ...result,
    samePlane: await page.locator(".lane-mosaic-plane").getAttribute("data-profile-identity") === token,
    inboundFrames: route.frameCount() - beforeFrames,
  };
}

async function runEngine(endpoint, fixture, worktrees, engine, seat) {
  const browser = await launchBrowser(engine);
  const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
  const route = await installSnapshotRoute(context);
  await installSocketControl(context);
  const page = await context.newPage();
  const monitor = monitorPage(page);
  const loadBefore = describeLoad();
  try {
    await page.goto(endpoint, { waitUntil: "networkidle" });
    await chooseCanvasView(page, "Lanes");
    if (seat) await seatAgentTeam(page, fixture, worktrees);
    await page.locator('[data-pane="agent-console"]').getByRole("tab", { name: /^Agents/ }).click();
    await page.locator(".lane-mosaic-plane").waitFor();
    const profile = {
      cards75: await profileSnapshot(page, route, 25),
      cards768: await profileSnapshot(page, route, 256),
      tabSwitch: await profileTabSwitch(page, route),
      resize: await profileResizeTransitions(page),
    };
    profile.diagnostics = monitor.evidence();
    profile.load = { before: loadBefore, after: describeLoad() };
    assert.equal(profile.cards75.cards, 75, `${engine} cold window is 25 cards per member`);
    assert.equal(profile.cards75.commitMutations, 1, `${engine} cold window commits once`);
    assert.equal(profile.cards75.reads, 75, `${engine} measures each cold card once`);
    assert.ok(profile.cards75.maxFrameGap < 200, `${engine} cold window remains responsive`);
    assert.equal(profile.tabSwitch.samePlane, true, `${engine} preserves the Mosaic plane`);
    assert.equal(profile.tabSwitch.inboundFrames, 0, `${engine} changes modes without transport`);
    assert.equal(profile.tabSwitch.addedCards, 0, `${engine} changes modes without rebuilding cards`);
    assert.equal(profile.tabSwitch.removedCards, 0, `${engine} changes modes without dropping cards`);
    assert.equal(profile.tabSwitch.commitMutations, 0, `${engine} changes modes without a commit`);
    assert.equal(profile.tabSwitch.reads, 768, `${engine} changes modes without remeasurement`);
    assert.ok(profile.tabSwitch.maxFrameGap < 250, `${engine} hydrated mode switch stays responsive`);
    monitor.assertClean();
    return profile;
  } finally {
    await context.close();
    await browser.close();
  }
}

await buildResident();
const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-startup-profile-")));
const linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-startup-profile-lanes-")));
const owner = ownFixture({ fixture, linkedRoot });
try {
  const worktrees = await configureFixture(fixture, linkedRoot, 4, {
    laneLease: "5m",
    worktreeNames: { 4: longWorktreeBasename },
  });
  const resident = owner.ownResident(await startResident(fixture));
  await seedAuthority(fixture);
  const firefox = await runEngine(resident.discovery.endpoint, fixture, worktrees, "firefox", true);
  const chromium = await runEngine(resident.discovery.endpoint, fixture, worktrees, "chromium", false);
  process.stdout.write(`${JSON.stringify({ firefox, chromium }, null, 2)}\n`);
  for (const [engine, profile] of Object.entries({ firefox, chromium })) {
    assert.ok(profile.resize.every(
      (transition) => transition.maxFrameGap < RESIZE_FRAME_BUDGET_MS,
    ),
      `${engine} resize transitions stay frame responsive: ${JSON.stringify({
        budgetMs: RESIZE_FRAME_BUDGET_MS,
        diagnostics: profile.diagnostics,
        load: profile.load,
        transitions: profile.resize.map((transition) => ({
          commitMutations: transition.commitMutations,
          completionMs: transition.completionMs,
          label: transition.label,
          longTasks: transition.longTasks,
          maxFrameGap: transition.maxFrameGap,
          placementWriteCards: transition.placementWriteCards,
          placementWrites: transition.placementWrites,
        })),
      })}`);
  }
} finally {
  await owner.cleanup();
}
