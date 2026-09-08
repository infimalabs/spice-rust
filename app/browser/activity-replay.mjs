import assert from "node:assert/strict";
import { Buffer } from "node:buffer";

import { TOOL_ACTIVITY_TEXT, TRANSITION_MS, waitForPageFunction } from "./canvas-fixture.mjs";
import { createMessageHistoryFixture } from "./message-history-fixture.mjs";

function decodeServerFrame(message) {
  return JSON.parse(typeof message === "string" ? message : message.toString("utf8"));
}

function agentMessageKey(message) {
  return [
    "message-agent",
    message.lane,
    message.locus.member,
    message.locus.offset,
    message.locus.length,
    message.locus.event,
  ].join("-");
}

function compareAgentMessages(left, right) {
  return Date.parse(left.observed ?? left.authored) - Date.parse(right.observed ?? right.authored)
    || left.lane.localeCompare(right.lane)
    || left.locus.member - right.locus.member
    || left.locus.offset - right.locus.offset
    || left.locus.length - right.locus.length
    || left.locus.event - right.locus.event;
}

/// Forward the production socket unchanged while retaining one real older tool
/// frame that the assertion can redeliver to the same browser connection.
export async function installActivityReplay(
  context,
  { retainInjectedSnapshots = false, withholdBootstrap = true } = {},
) {
  let captured;
  let activeSocket;
  let heldHistoryRequest;
  let replayed = false;
  let withheldBootstrap = false;
  let injectedEvent = 10_000;
  const knownMessages = new Map();
  const injectedKeys = new Set();
  const observedProse = [];
  const proseFrames = new Map();
  const proseWaiters = [];
  let canonicalProseFrames;
  let progressiveHistory;
  let socketGeneration = 0;
  const remember = (messages) => {
    for (const message of messages) knownMessages.set(agentMessageKey(message), message);
  };
  const rememberProseSources = (frame, messages) => {
    for (const message of messages) {
      if (message.activity === "tool") continue;
      proseFrames.set(message.lane, {
        stream: "message",
        v: frame.v,
        id: frame.id,
        epoch: frame.epoch,
        message,
      });
    }
  };
  await context.routeWebSocket(/.*/, (socket) => {
    socketGeneration += 1;
    const routedGeneration = socketGeneration;
    activeSocket = socket;
    progressiveHistory?.socketOpened(socket, routedGeneration);
    const server = socket.connectToServer();
    socket.onMessage((message) => {
      const frame = decodeServerFrame(message);
      if (progressiveHistory?.handleClientFrame(frame, socket, routedGeneration)) return;
      if (frame.stream === "message-history" && heldHistoryRequest?.lane === frame.lane) {
        const held = heldHistoryRequest;
        heldHistoryRequest = undefined;
        held.resolve(frame);
        return;
      }
      server.send(message);
    });
    server.onMessage((message) => {
      const frame = decodeServerFrame(message);
      if (progressiveHistory?.handleServerFrame(frame, socket, routedGeneration)) return;
      let outbound = message;
      if (retainInjectedSnapshots && frame.stream === "messages") {
        const merged = new Map(frame.messages.map((entry) => [agentMessageKey(entry), entry]));
        for (const key of injectedKeys) merged.set(key, knownMessages.get(key));
        frame.messages = [...merged.values()].sort(compareAgentMessages);
        outbound = Buffer.from(JSON.stringify(frame));
      }
      if (frame.stream === "message") {
        remember([frame.message]);
        rememberProseSources(frame, [frame.message]);
      }
      if (frame.stream === "messages" || frame.stream === "message-history") {
        remember(frame.messages);
        rememberProseSources(frame, frame.messages);
      }
      // Exercise the browser's bootstrap boundary with the initial retained
      // message page absent. A live prose frame can still follow on this same
      // socket, and it must remain visible while the capacity announcement is
      // outstanding rather than being truncated against an implicit zero.
      if (withholdBootstrap && !withheldBootstrap && frame.stream === "messages") {
        withheldBootstrap = true;
        return;
      }
      socket.send(outbound);
      if (socket === activeSocket
        && frame.stream === "message"
        && frame.message?.activity !== "tool") {
        observedProse.push({
          lane: frame.message?.lane,
          author: frame.message?.author,
          text: frame.message?.text,
        });
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

  const injectAgentMessage = async (
    page,
    lane,
    text,
    {
      activity = undefined,
      automaticSpeech = undefined,
      badges = undefined,
      media = undefined,
      history = false,
    } = {},
  ) => {
    const source = proseFrames.get(lane);
    assert.ok(source, `the route captured an agent prose frame for ${lane}`);
    assert.ok(activeSocket, "the route retains the current browser WebSocket");
    const frame = JSON.parse(JSON.stringify(source));
    frame.message.text = text;
    if (activity === undefined) delete frame.message.activity;
    else frame.message.activity = activity;
    if (automaticSpeech === undefined) delete frame.message.automatic_speech;
    else frame.message.automatic_speech = automaticSpeech;
    if (badges === undefined) delete frame.message.badges;
    else frame.message.badges = badges;
    delete frame.message.final_answer;
    if (media === undefined) delete frame.message.media;
    else frame.message.media = media;
    frame.message.authored = new Date().toISOString();
    frame.message.authored_synthetic = false;
    delete frame.message.observed;
    frame.message.locus.event += injectedEvent;
    injectedEvent += 1;
    const expectedKey = agentMessageKey(frame.message);
    let delivery = frame;
    if (history) {
      const visibleKeys = await page.locator(
        '[data-card-kind="agent-message"]',
      ).evaluateAll((cards) => cards.map((card) => card.dataset.mosaicCardKey));
      const visibleMessages = visibleKeys.map((key) => knownMessages.get(key)).filter(Boolean);
      assert.equal(
        visibleMessages.length,
        visibleKeys.length,
        "the history fixture can reconstruct every visible agent card",
      );
      const snapshotMessages = new Map(
        visibleMessages.map((message) => [agentMessageKey(message), message]),
      );
      for (const message of knownMessages.values()) {
        if (message.activity === "tool" && (message.media?.length ?? 0) === 0) {
          snapshotMessages.set(agentMessageKey(message), message);
        }
      }
      assert.equal(heldHistoryRequest, undefined, "only one history request is held at a time");
      const requested = new Promise((resolve) => { heldHistoryRequest = { lane, resolve }; });
      activeSocket.send(Buffer.from(JSON.stringify({
        stream: "messages",
        v: source.v,
        id: source.id,
        epoch: source.epoch,
        limit: 256,
        messages: [...snapshotMessages.values()].sort(compareAgentMessages),
        more: [lane],
      })));
      const request = await Promise.race([
        requested,
        new Promise((_, reject) => setTimeout(
          () => reject(new Error(`the browser did not request history for ${lane}`)),
          TRANSITION_MS,
        )),
      ]);
      delivery = {
        stream: "message-history",
        v: source.v,
        id: source.id,
        epoch: source.epoch,
        lane,
        before: request.before,
        more: false,
        messages: [frame.message],
      };
    }
    injectedKeys.add(expectedKey);
    remember([frame.message]);
    activeSocket.send(Buffer.from(JSON.stringify(delivery)));
    await waitForPageFunction(
      page,
      `deterministic ${history ? "history" : activity ?? "prose"} reaches the browser`,
      TRANSITION_MS,
      (key) => [...document.querySelectorAll(".mosaic-message-card")]
        .some((card) => card.dataset.mosaicCardKey === key),
      expectedKey,
      {
        observed: async () => ({
          delivery,
          currentSource: source === proseFrames.get(lane),
          cards: await page.locator(".mosaic-message-card").evaluateAll((cards) => (
            cards.map((card) => ({
              key: card.dataset.mosaicCardKey,
              kind: card.dataset.cardKind,
              text: card.textContent,
            }))
          )),
        }),
      },
    );
    return { key: expectedKey, message: JSON.parse(JSON.stringify(frame.message)) };
  };

  return {
    nextAgentProse(lane) {
      return new Promise((resolve) => proseWaiters.push({ lane, resolve }));
    },
    injectAgentProse(page, lane, text, media = undefined) {
      return injectAgentMessage(page, lane, text, { media });
    },
    injectClassifiedAgentProse(page, lane, text, badges) {
      return injectAgentMessage(page, lane, text, { badges });
    },
    injectAgentToolMedia(page, lane, media, text = "") {
      return injectAgentMessage(page, lane, text, { activity: "tool", media });
    },
    injectAgentToolMediaHistory(page, lane, media, text) {
      return injectAgentMessage(page, lane, text, { activity: "tool", media, history: true });
    },
    async replayAgentMessage(page, key) {
      const current = knownMessages.get(key);
      assert.ok(current, `the route retained message ${key}`);
      const source = proseFrames.get(current.lane);
      assert.ok(source, `the route captured an agent prose frame for ${current.lane}`);
      assert.ok(activeSocket, "the route retains the current browser WebSocket");
      const frame = JSON.parse(JSON.stringify(source));
      frame.message = JSON.parse(JSON.stringify(current));
      activeSocket.send(Buffer.from(JSON.stringify(frame)));
      await page.evaluate(() => new Promise((resolve) => {
        requestAnimationFrame(() => requestAnimationFrame(resolve));
      }));
      return { key };
    },
    async reviseAgentFinal(page, key) {
      const current = knownMessages.get(key);
      assert.ok(current, `the route retained message ${key}`);
      const source = proseFrames.get(current.lane);
      assert.ok(source, `the route captured an agent prose frame for ${current.lane}`);
      assert.ok(activeSocket, "the route retains the current browser WebSocket");
      const frame = JSON.parse(JSON.stringify(source));
      frame.message = JSON.parse(JSON.stringify(current));
      frame.message.final_answer = true;
      remember([frame.message]);
      activeSocket.send(Buffer.from(JSON.stringify(frame)));
      await waitForPageFunction(
        page,
        "same-identity final revision reaches the browser",
        TRANSITION_MS,
        (expectedKey) => {
          const card = document.querySelector(`[data-mosaic-card-key="${expectedKey}"]`);
          return card?.dataset.messageFinal === "true" && card.classList.contains("final");
        },
        key,
      );
      return { key };
    },
    injectAgentHistory(page, lane, text) {
      return injectAgentMessage(page, lane, text, { history: true });
    },
    injectCompaction(page, lane, text = "") {
      return injectAgentMessage(page, lane, text, {
        activity: "compaction",
        media: [],
      });
    },
    progressiveHistory() {
      assert.equal(progressiveHistory, undefined, "one progressive history fixture owns the route");
      assert.ok(activeSocket, "progressive history retains the current browser WebSocket");
      progressiveHistory = createMessageHistoryFixture({
        generation: socketGeneration,
        remember,
        socket: activeSocket,
        sources: [...proseFrames.values()],
      });
      return progressiveHistory;
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
      remember([message]);
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
    async restoreProseSnapshot(page, targetSocket = activeSocket) {
      canonicalProseFrames ??= [...proseFrames.values()];
      const current = [...proseFrames.values()];
      assert.ok(canonicalProseFrames.length > 1, "the route retained the lineage prose snapshot");
      assert.ok(current.length > 1, "the route retained current message transport authority");
      assert.ok(targetSocket, "the route retains the requested browser WebSocket");
      activeSocket = targetSocket;
      const source = current[0];
      targetSocket.send(Buffer.from(JSON.stringify({
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
        TRANSITION_MS * 4,
        (expected) => document.querySelectorAll(
          '[data-card-kind="agent-message"]',
        ).length === expected,
        canonicalProseFrames.length,
      );
      return canonicalProseFrames.map((frame) => ({
        key: agentMessageKey(frame.message),
        lane: frame.message.lane,
        prose: frame.message.text,
      }));
    },
    async assertBootstrapLiveMessages(console) {
      assert.equal(withheldBootstrap, true, "the route withheld the initial message snapshot");
      await console.getByRole("tab", { name: /^Threads/ }).click();
      const cards = console.locator('[data-card-kind="agent-message"]');
      try {
        await cards.nth(3).waitFor({ timeout: 10_000 });
      } catch (error) {
        const observed = {
          prose: observedProse,
          cards: await cards.count(),
          tickers: await console.locator(".ticker").allTextContents(),
          members: await console.locator(".organization-agent").evaluateAll((nodes) => nodes.map(
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
    async assertOlderToolIgnored(page, organization) {
      assert.ok(captured, "the route captured the production tool activity frame");
      assert.equal(replayed, false, "the older tool activity is replayed exactly once");
      replayed = true;
      const identity = {
        lane: captured.frame.message.lane,
        locus: captured.frame.message.locus,
      };
      const ticker = organization.locator(".ticker");
      const before = await ticker.evaluate(tickerIdentity);
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
      assert.deepEqual(
        await ticker.evaluate(tickerIdentity),
        before,
        "a replayed older activity does not walk the status strip backwards",
      );
    },
  };
}

function tickerIdentity(node) {
  return {
    lane: node.dataset.agentLane,
    authored: node.querySelector("[data-age-authored]")?.dataset.ageAuthored,
    prefix: node.querySelector(".ticker-prefix")?.textContent,
    detail: node.querySelector(".ticker-detail")?.textContent,
  };
}
