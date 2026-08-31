import assert from "node:assert/strict";
import { Buffer } from "node:buffer";

import { TOOL_ACTIVITY_TEXT, TRANSITION_MS, waitForPageFunction } from "./canvas-fixture.mjs";

function decodeServerFrame(message) {
  return JSON.parse(typeof message === "string" ? message : message.toString("utf8"));
}

/// Forward the production socket unchanged while retaining one real older tool
/// frame that the assertion can redeliver to the same browser connection.
export async function installActivityReplay(context) {
  let captured;
  let activeSocket;
  let replayed = false;
  let withheldBootstrap = false;
  let injectedEvent = 10_000;
  const observedProse = [];
  const proseFrames = new Map();
  const proseWaiters = [];
  let canonicalProseFrames;
  await context.routeWebSocket(/.*/, (socket) => {
    activeSocket = socket;
    const server = socket.connectToServer();
    server.onMessage((message) => {
      const frame = decodeServerFrame(message);
      // Exercise the browser's bootstrap boundary with the initial retained
      // message page absent. A live prose frame can still follow on this same
      // socket, and it must remain visible while the capacity announcement is
      // outstanding rather than being truncated against an implicit zero.
      if (!withheldBootstrap && frame.stream === "messages") {
        withheldBootstrap = true;
        return;
      }
      socket.send(message);
      if (socket === activeSocket
        && frame.stream === "message"
        && frame.message?.activity !== "tool") {
        observedProse.push({
          lane: frame.message?.lane,
          author: frame.message?.author,
          text: frame.message?.text,
        });
        proseFrames.set(frame.message?.lane, frame);
        for (let index = proseWaiters.length - 1; index >= 0; index -= 1) {
          if (proseWaiters[index].lane === frame.message?.lane) {
            proseWaiters.splice(index, 1)[0].resolve(frame.message);
          }
        }
      }
      if (captured) return;
      if (
        frame.stream === "message" &&
        frame.message?.activity === "tool" &&
        frame.message.text === TOOL_ACTIVITY_TEXT
      ) {
        captured = { frame, message, socket };
      }
    });
  });

  return {
    nextAgentProse(lane) {
      return new Promise((resolve) => proseWaiters.push({ lane, resolve }));
    },
    async injectAgentProse(page, lane, text, media) {
      const source = proseFrames.get(lane);
      assert.ok(source, `the route captured an agent prose frame for ${lane}`);
      assert.ok(activeSocket, "the route retains the current browser WebSocket");
      const frame = JSON.parse(JSON.stringify(source));
      frame.message.text = text;
      if (media !== undefined) frame.message.media = media;
      frame.message.authored = new Date().toISOString();
      frame.message.authored_synthetic = false;
      delete frame.message.observed;
      frame.message.locus.event += injectedEvent;
      injectedEvent += 1;
      const expectedKey = [
        "message-agent",
        frame.message.lane,
        frame.message.locus.member,
        frame.message.locus.offset,
        frame.message.locus.length,
        frame.message.locus.event,
      ].join("-");
      activeSocket.send(Buffer.from(JSON.stringify(frame)));
      await waitForPageFunction(
        page,
        "deterministic agent prose reaches the browser",
        TRANSITION_MS,
        (key) => [...document.querySelectorAll('[data-card-kind="agent-message"]')]
          .some((card) => card.dataset.mosaicCardKey === key),
        expectedKey,
        {
          observed: async () => ({
            frame,
            currentSource: source === proseFrames.get(lane),
            cards: await page.locator(
              `[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
            ).evaluateAll((cards) => cards.map((card) => ({
              key: card.dataset.mosaicCardKey,
              text: card.textContent,
            }))),
          }),
        },
      );
      return { key: expectedKey };
    },
    async injectSyntheticSnapshot(page, lane, text) {
      const source = proseFrames.get(lane);
      assert.ok(source, `the route captured an agent prose frame for ${lane}`);
      assert.ok(activeSocket, "the route retains the current browser WebSocket");
      const message = JSON.parse(JSON.stringify(source.message));
      message.text = text;
      message.authored = new Date().toISOString();
      message.authored_synthetic = true;
      delete message.observed;
      message.locus.event += 20_000;
      assert.equal(
        Object.hasOwn(message, "observed"),
        false,
        "the synthetic browser fixture carries no Spice observation time",
      );
      activeSocket.send(Buffer.from(JSON.stringify({
        stream: "messages",
        v: source.v,
        id: source.id,
        epoch: source.epoch,
        limit: 256,
        messages: [message],
      })));
      await waitForPageFunction(
        page,
        "the timestamp-free synthetic snapshot reaches the browser",
        TRANSITION_MS,
        ({ expectedLane, expectedText }) => {
          const cards = [...document.querySelectorAll('[data-card-kind="agent-message"]')];
          return cards.length === 1
            && cards[0].dataset.agentLane === expectedLane
            && cards[0].textContent?.includes(expectedText);
        },
        { expectedLane: lane, expectedText: text },
      );
      return { authored: message.authored, text };
    },
    async restoreProseSnapshot(page) {
      canonicalProseFrames ??= [...proseFrames.values()];
      const current = [...proseFrames.values()];
      assert.ok(canonicalProseFrames.length > 1, "the route retained the lineage prose snapshot");
      assert.ok(current.length > 1, "the route retained current message transport authority");
      assert.ok(activeSocket, "the route retains the current browser WebSocket");
      const source = current[0];
      activeSocket.send(Buffer.from(JSON.stringify({
        stream: "messages",
        v: source.v,
        id: source.id,
        epoch: source.epoch,
        limit: 256,
        messages: canonicalProseFrames.map((frame) => frame.message),
      })));
      await waitForPageFunction(
        page,
        "the original lineage prose snapshot is restored",
        TRANSITION_MS,
        (expected) => document.querySelectorAll(
          '[data-card-kind="agent-message"]',
        ).length === expected,
        canonicalProseFrames.length,
      );
      return canonicalProseFrames.map((frame) => ({
        lane: frame.message.lane,
        prose: frame.message.text,
      }));
    },
    async assertBootstrapLiveMessages(console) {
      assert.equal(withheldBootstrap, true, "the route withheld the initial message snapshot");
      await console.getByRole("tab", { name: /^Compose/ }).click();
      const cards = console.locator('[data-card-kind="agent-message"]');
      try {
        await cards.nth(3).waitFor({ timeout: 10_000 });
      } catch (error) {
        const observed = {
          prose: observedProse,
          cards: await cards.count(),
          tickers: await console.locator(".ticker").allTextContents(),
          members: await console.locator(".team-agent").evaluateAll((nodes) => nodes.map(
            (node) => ({ lane: node.dataset.agentLane, text: node.textContent }),
          )),
        };
        throw new Error(`bootstrap live messages were absent: ${JSON.stringify(observed)}`, {
          cause: error,
        });
      }
      assert.equal(
        await cards.count(),
        4,
        "four live agent messages remain visible before the capacity snapshot arrives",
      );
    },
    async assertOlderToolIgnored(page, team) {
      assert.ok(captured, "the route captured the production tool activity frame");
      assert.equal(replayed, false, "the older tool activity is replayed exactly once");
      replayed = true;
      const identity = {
        lane: captured.frame.message.lane,
        locus: captured.frame.message.locus,
      };
      const before = await team.locator(".ticker").innerText();
      await page.evaluate((expected) => {
        const socket = window.__canvasSockets.at(-1);
        if (!socket) throw new Error("canvas WebSocket was not captured");
        window.__activityReplayObserved = false;
        const observe = async (event) => {
          const text = typeof event.data === "string"
            ? event.data
            : event.data instanceof Blob
              ? await event.data.text()
              : new TextDecoder().decode(event.data);
          const frame = JSON.parse(text);
          if (
            frame.stream === "message" &&
            frame.message?.lane === expected.lane &&
            JSON.stringify(frame.message.locus) === JSON.stringify(expected.locus)
          ) {
            socket.removeEventListener("message", observe);
            window.__activityReplayObserved = true;
          }
        };
        socket.addEventListener("message", observe);
      }, identity);
      captured.socket.send(captured.message);
      await waitForPageFunction(
        page,
        "the browser receives the replayed older activity",
        TRANSITION_MS,
        () => window.__activityReplayObserved === true,
      );
      await page.evaluate(() => new Promise((resolve) => {
        requestAnimationFrame(() => requestAnimationFrame(resolve));
      }));
      assert.equal(
        await team.locator(".ticker").innerText(),
        before,
        "a replayed older activity does not walk the status strip backwards",
      );
    },
  };
}
