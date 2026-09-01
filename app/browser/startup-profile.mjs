import assert from "node:assert/strict";
import { Buffer } from "node:buffer";
import { mkdtemp, realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

import { seatAgentTeam } from "./agent-conversation.mjs";
import { longWorktreeBasename } from "./agent-conversation.mjs";
import { buildResident, chooseCanvasView, configureFixture, seedAuthority, startResident } from "./canvas-fixture.mjs";
import { ownFixture } from "./canvas-lifecycle.mjs";
import { installSocketControl } from "./canvas-speech.mjs";
import { launchBrowser } from "./headless-launch.mjs";

const PROFILE_TIMEOUT_MS = 60_000;

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
      attributeFilter: ["data-plane-commits"],
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
  await console.getByRole("tab", { name: /^Compose/ }).click();
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
  try {
    await page.goto(endpoint, { waitUntil: "networkidle" });
    await chooseCanvasView(page, "Agents");
    if (seat) await seatAgentTeam(page, fixture, worktrees);
    await page.locator('[data-pane="agent-console"]').getByRole("tab", { name: /^Compose/ }).click();
    await page.locator(".lane-mosaic-plane").waitFor();
    const profile = {
      cards75: await profileSnapshot(page, route, 25),
      cards768: await profileSnapshot(page, route, 256),
      tabSwitch: await profileTabSwitch(page, route),
    };
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
} finally {
  await owner.cleanup();
}
