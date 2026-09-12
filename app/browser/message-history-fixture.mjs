import assert from "node:assert/strict";
import { Buffer } from "node:buffer";

import { TRANSITION_MS } from "./canvas-fixture.mjs";

function messageKey(message) {
  return [
    "message-agent",
    message.lane,
    message.locus.member,
    message.locus.offset,
    message.locus.length,
    message.locus.event,
  ].join("-");
}

function locusKey(locus) {
  return [locus.member, locus.offset, locus.length, locus.event].join(":");
}

function clone(value) {
  return JSON.parse(JSON.stringify(value));
}

function waitFor(waiters, label, inspect, timeout = TRANSITION_MS) {
  const current = inspect();
  if (current !== undefined) return Promise.resolve(current);
  return new Promise((resolve, reject) => {
    const waiter = { inspect, resolve, timer: undefined };
    waiter.timer = setTimeout(() => {
      const index = waiters.indexOf(waiter);
      if (index >= 0) waiters.splice(index, 1);
      reject(new Error(`timed out waiting for ${typeof label === "function" ? label() : label}`));
    }, timeout);
    waiters.push(waiter);
  });
}

function notify(waiters) {
  for (let index = waiters.length - 1; index >= 0; index -= 1) {
    const waiter = waiters[index];
    const value = waiter.inspect();
    if (value === undefined) continue;
    waiters.splice(index, 1);
    clearTimeout(waiter.timer);
    waiter.resolve(value);
  }
}

/// Build deterministic retained history from three real lane-authority frames.
/// Only the current head of each lane travels in a `messages` snapshot. Every
/// older record is available exclusively through a member-local history page.
export function createMessageHistoryFixture({
  generation,
  remember,
  socket,
  sources,
}) {
  const sorted = [...sources].sort(
    (left, right) => left.message.lane.localeCompare(right.message.lane),
  );
  assert.ok(sorted.length >= 3, "progressive history owns at least three real lane authorities");
  const frames = sorted.length === 3 ? sorted : [sorted[0], sorted[1], sorted.at(-1)];

  let event = 80_000;
  const make = (frame, text, observed, offset, activity = undefined) => {
    const message = clone(frame.message);
    message.text = text;
    message.media = [];
    if (activity === undefined) delete message.activity;
    else message.activity = activity;
    message.authored = observed;
    message.authored_synthetic = false;
    message.observed = observed;
    message.locus = {
      member: message.locus.member,
      offset,
      length: Math.max(1, text.length + 1),
      event,
    };
    event += 1;
    return message;
  };

  const lanes = frames.map((frame) => frame.message.lane);
  const [first, second, third] = frames;
  const firstWarm = Array.from({ length: 20 }, (_, index) => make(
    first,
    `history first bounded ${index + 1}`,
    `2031-05-17T12:00:00.${String((index + 1) * 40).padStart(3, "0")}Z`,
    70_060 + index,
  ));
  const records = {
    firstDeep: make(first, "history first deep", "2031-05-17T11:59:58.000Z", 70_000),
    secondOlder: make(second, "history second exhausted", "2031-05-17T11:59:59.000Z", 71_000),
    firstOlder: make(first, "history first older", "2031-05-17T12:00:00.000Z", 70_050),
    firstHead: make(first, "history first head", "2031-05-17T12:00:01.000Z", 70_100),
    thirdBeforeBoundary: make(
      third, "history third before boundary", "2031-05-17T12:00:02.000Z", 72_250,
    ),
    boundary: make(second, "", "2031-05-17T12:00:03.000Z", 71_200, "compaction"),
    thirdAfterBoundary: make(
      third, "history third after boundary", "2031-05-17T12:00:04.000Z", 72_275,
    ),
    thirdHead: make(third, "history third head", "2031-05-17T12:00:05.000Z", 72_300),
    live: make(first, "history concurrent live", "2031-05-17T12:00:06.000Z", 70_150),
    stale: make(third, "history stale response", "2031-05-17T12:00:01.500Z", 72_260),
  };
  const bulk = Array.from({ length: 75 }, (_, index) => {
    const frame = frames[index % frames.length];
    const text = index === 17
      ? [
        "## Measured lattice witness",
        ...Array.from({ length: 8 }, (_, paragraph) => [
          "",
          `Integrated hydration paragraph ${paragraph + 1} preserves deterministic chronology`,
          "while enough ordinary words wrap naturally across the narrow and wide lattice candidates.",
        ].join(" ")),
      ].join("\n")
      : `history retained record ${index + 1}`;
    return make(
      frame,
      text,
      index === 17
        ? "2031-05-17T11:59:59.900Z"
        : new Date(Date.parse("2031-05-17T11:50:00.000Z") + index * 1_000).toISOString(),
      60_000 + (index % frames.length) * 1_000 + Math.floor(index / frames.length) * 10,
    );
  });
  const bulkByLane = new Map(lanes.map((lane) => [lane, bulk.filter((row) => row.lane === lane)]));
  const initialHeads = [records.firstHead, records.boundary, records.thirdHead];
  const currentHeads = [records.live, records.boundary, records.thirdHead];
  const pages = {
    empty: { messages: [], more: true },
    first: { messages: [records.firstOlder, ...firstWarm], more: true },
    firstReplay: { messages: [records.firstOlder, ...firstWarm, records.firstHead], more: true },
    firstFinal: { messages: [...bulkByLane.get(lanes[0]), records.firstDeep], more: false },
    second: { messages: [...bulkByLane.get(lanes[1]), records.secondOlder], more: false },
    third: {
      messages: [records.thirdAfterBoundary, records.thirdBeforeBoundary],
      more: true,
    },
    thirdFinal: {
      messages: [records.firstDeep, ...bulkByLane.get(lanes[2]), records.thirdBeforeBoundary],
      more: false,
    },
  };
  // The third final page deliberately carries one foreign-lane record. The
  // runtime must ignore it while exhausting only the requested third lane.

  let active = true;
  let activeGeneration = generation;
  let activeSocket = socket;
  const sockets = new Map([[generation, socket]]);
  const suppressedSnapshots = new Map();
  const metadata = new Map();
  const requests = [];
  const snapshots = [];
  const deliveries = [];
  const waiters = [];
  const awaitEvent = (label, inspect, timeout = TRANSITION_MS) => (
    waitFor(waiters, label, inspect, timeout)
  );

  const send = (target, frame) => {
    assert.ok(target, "history fixture has a browser WebSocket");
    target.send(Buffer.from(JSON.stringify(frame)));
  };
  const frameMetadata = (targetGeneration) => {
    const observed = metadata.get(targetGeneration);
    const source = frames[0];
    return observed ?? { v: source.v, id: source.id, epoch: source.epoch };
  };

  const api = {
    lanes,
    records,
    bulk,
    pages,
    keys: Object.fromEntries(Object.entries(records).map(([name, message]) => (
      [name, messageKey(message)]
    ))),
    get generation() {
      return activeGeneration;
    },
    socketOpened(nextSocket, nextGeneration) {
      if (!active) return;
      activeSocket = nextSocket;
      activeGeneration = nextGeneration;
      sockets.set(nextGeneration, nextSocket);
      notify(waiters);
    },
    handleClientFrame(frame, frameSocket, frameGeneration) {
      if (!active) return false;
      if (frame.stream !== "message-history") return false;
      requests.push({
        ...clone(frame),
        answered: false,
        generation: frameGeneration,
        ordinal: requests.length,
        socket: frameSocket,
      });
      notify(waiters);
      return true;
    },
    handleServerFrame(frame, frameSocket, frameGeneration) {
      if (!active) return false;
      if (frame.stream !== "messages") return false;
      activeSocket = frameSocket;
      activeGeneration = frameGeneration;
      metadata.set(frameGeneration, { v: frame.v, id: frame.id, epoch: frame.epoch });
      suppressedSnapshots.set(
        frameGeneration,
        (suppressedSnapshots.get(frameGeneration) ?? 0) + 1,
      );
      notify(waiters);
      return true;
    },
    async waitForSocketAfter(previous) {
      return awaitEvent("the next routed Canvas socket", () => (
        activeGeneration > previous ? activeGeneration : undefined
      ));
    },
    async waitForServerSnapshot(targetGeneration) {
      return awaitEvent(`the generation ${targetGeneration} retained head`, () => (
        suppressedSnapshots.has(targetGeneration) ? targetGeneration : undefined
      ));
    },
    async waitForRequests(expectedLanes, targetGeneration = activeGeneration, after = 0) {
      return awaitEvent(`${expectedLanes.length} member-local history requests`, () => {
        const found = new Map();
        for (const request of requests.slice(after)) {
          if (request.generation === targetGeneration
            && expectedLanes.includes(request.lane)
            && !found.has(request.lane)) {
            found.set(request.lane, request);
          }
        }
        return found.size === expectedLanes.length ? found : undefined;
      }, TRANSITION_MS * 4);
    },
    async waitForRequest(lane, before, targetGeneration = activeGeneration, after = 0) {
      const expected = locusKey(before);
      return awaitEvent(
        () => {
          const observed = requests.slice(after)
            .filter((request) => request.lane === lane)
            .map((request) => ({
              answered: request.answered,
              before: locusKey(request.before),
              generation: request.generation,
            }));
          return `history request ${lane} before ${expected} in generation ${targetGeneration}; `
            + `observed ${JSON.stringify(observed)}`;
        },
        () => requests.slice(after).find(
          (request) => request.generation === targetGeneration
            && request.lane === lane
            && locusKey(request.before) === expected,
        ),
        TRANSITION_MS * 4,
      );
    },
    requestCount() {
      return requests.length;
    },
    socketForGeneration(targetGeneration) {
      const target = sockets.get(targetGeneration);
      assert.ok(target, `history fixture retains socket generation ${targetGeneration}`);
      return target;
    },
    requestsForGeneration(targetGeneration) {
      return requests.filter((request) => request.generation === targetGeneration);
    },
    sendHeads(reason, more = lanes, targetGeneration = activeGeneration) {
      assert.ok(["initial", "reconnect", "reload"].includes(reason),
        `head snapshot reason is controlled: ${reason}`);
      const target = sockets.get(targetGeneration) ?? activeSocket;
      const meta = frameMetadata(targetGeneration);
      const heads = reason === "initial" ? initialHeads : currentHeads;
      const frame = {
        stream: "messages",
        ...meta,
        limit: 256,
        messages: heads,
        more,
      };
      snapshots.push({
        reason,
        generation: targetGeneration,
        keys: heads.map(messageKey),
        more: [...more],
      });
      remember(heads);
      send(target, frame);
    },
    sendLive() {
      const meta = frameMetadata(activeGeneration);
      remember([records.live]);
      deliveries.push({ kind: "live", key: messageKey(records.live) });
      send(activeSocket, { stream: "message", ...meta, message: records.live });
    },
    answer(request, page) {
      assert.equal(request.answered, false, "one held request receives one accepted answer");
      request.answered = true;
      const messages = page.messages.map(clone);
      remember(messages);
      deliveries.push({
        kind: "history",
        lane: request.lane,
        before: clone(request.before),
        keys: messages.map(messageKey),
        more: page.more,
      });
      send(request.socket, {
        stream: "message-history",
        v: request.v,
        id: request.id,
        epoch: request.epoch,
        lane: request.lane,
        before: request.before,
        more: page.more,
        messages,
      });
    },
    sendStale(request) {
      const message = clone(records.stale);
      deliveries.push({ kind: "stale", lane: request.lane, key: messageKey(message) });
      send(request.socket, {
        stream: "message-history",
        v: request.v,
        id: request.id,
        epoch: request.epoch,
        lane: request.lane,
        before: request.before,
        more: false,
        messages: [message],
      });
    },
    expectedKeys() {
      return [
        ...bulk,
        ...firstWarm,
        records.firstDeep,
        records.secondOlder,
        records.firstOlder,
        records.firstHead,
        records.thirdBeforeBoundary,
        records.boundary,
        records.thirdAfterBoundary,
        records.thirdHead,
        records.live,
      ].sort((left, right) => left.observed.localeCompare(right.observed)
        || left.lane.localeCompare(right.lane)
        || left.locus.member - right.locus.member
        || left.locus.offset - right.locus.offset
        || left.locus.length - right.locus.length
        || left.locus.event - right.locus.event)
        .map(messageKey);
    },
    audit() {
      assert.ok(snapshots.length >= 3, "initial, reconnect, and reload each seed one retained head");
      assert.ok(snapshots.every((entry) => entry.keys.length === lanes.length),
        "every injected messages snapshot contains one head per lane, never a history page");
      assert.ok(deliveries.filter((entry) => entry.kind === "history").length >= 8,
        "older records arrive through repeated member-local pages");
      return {
        deliveries: deliveries.map(clone),
        requests: requests.map(({ socket: _socket, ...request }) => clone(request)),
        snapshots: snapshots.map(clone),
      };
    },
    stop() {
      active = false;
    },
  };
  return api;
}
