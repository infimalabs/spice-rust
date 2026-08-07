import assert from "node:assert/strict";

import { TRANSITION_MS, waitForPageFunction } from "./canvas-fixture.mjs";
import { TOOL_ACTIVITY_TEXT } from "./canvas-transcript-records.mjs";

const newerProse = "assistant: newest lane prose witness";

function decodeServerFrame(message) {
  return JSON.parse(typeof message === "string" ? message : message.toString("utf8"));
}

/// Forward the production socket unchanged while retaining one real older tool
/// frame that the assertion can redeliver to the same browser connection.
export async function installActivityReplay(context) {
  let captured;
  let replayed = false;
  await context.routeWebSocket(/.*/, (socket) => {
    const server = socket.connectToServer();
    server.onMessage((message) => {
      socket.send(message);
      if (captured) return;
      const frame = decodeServerFrame(message);
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
    async assertOlderToolIgnored(page, team) {
      assert.ok(captured, "the route captured the production tool activity frame");
      assert.equal(replayed, false, "the older tool activity is replayed exactly once");
      replayed = true;
      const identity = {
        lane: captured.frame.message.lane,
        locus: captured.frame.message.locus,
      };
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
        newerProse,
        "a replayed older activity does not walk the status strip backwards",
      );
    },
  };
}
