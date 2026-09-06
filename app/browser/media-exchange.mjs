import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { readFile, rename, rm, writeFile } from "node:fs/promises";
import { join } from "node:path";

import { binary } from "./canvas-build.mjs";
import {
  chooseCanvasView,
  fixtureCommandEnv,
  reply,
  TRANSITION_MS,
  waitForPageFunction,
} from "./canvas-fixture.mjs";
import { pasteImages } from "./composer-image-indicators.mjs";
import { assertImageLifecycle } from "./image-lifecycle.mjs";
import {
  assertFullSizeTarget,
  assertImagePreview,
  openAndAssertIsolated,
} from "./mosaic-image-reference.mjs";

const PIXEL_PNG = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";
const READ_TRIGGER = ".spice-codex-read-trigger";
const READ_RESULT = ".spice-codex-read-result";
const RECORD_TRIGGER = ".spice-codex-records-trigger";

async function publishTrigger(worktree, name, body) {
  const target = join(worktree, name);
  const staged = `${target}.${process.pid}.pending`;
  await writeFile(staged, body);
  await rename(staged, target);
}

async function invokeHook(repository, fixture, lane) {
  return execFileSync(binary, ["hook", "codex"], {
    cwd: repository,
    encoding: "utf8",
    env: fixtureCommandEnv(lane, fixture),
    input: "",
    timeout: 20_000,
  });
}

function hookContext(document) {
  if (document.trim().length === 0) return "";
  const decoded = JSON.parse(document);
  return decoded.hookSpecificOutput.additionalContext;
}

function contextRecords(context, prefix) {
  return context
    .split("\n")
    .filter((line) => line.startsWith(prefix))
    .map((line) => JSON.parse(line.slice(prefix.length)));
}

async function waitForAgentRead(worktree) {
  const result = join(worktree, READ_RESULT);
  const deadline = Date.now() + TRANSITION_MS;
  while (Date.now() < deadline) {
    try {
      return await readFile(result);
    } catch (error) {
      if (error?.code !== "ENOENT") throw error;
      await new Promise((resolve) => setTimeout(resolve, 50));
    }
  }
  throw new Error(`Codex stand-in did not read ${result}`);
}

function codexMediaRecords(path, dataUrl) {
  const call = "call_canvas_media_exchange";
  const now = Date.now();
  const companion = {
    timestamp: new Date(now + 2).toISOString(),
    type: "event_msg",
    payload: {
      type: "item_completed",
      item: { type: "ImageView", id: "canvas-media-view", path },
    },
  };
  return [
    {
      timestamp: new Date(now).toISOString(),
      type: "response_item",
      payload: {
        type: "custom_tool_call",
        id: "canvas-media-call",
        status: "completed",
        call_id: call,
        name: "exec",
        input: "const r = await tools.view_image({ path: image }); image(r.image_url);",
      },
    },
    {
      timestamp: new Date(now + 1).toISOString(),
      type: "response_item",
      payload: {
        type: "custom_tool_call_output",
        id: "canvas-media-result",
        call_id: call,
        output: [
          { type: "input_text", text: "Script completed\nWall time 0.1 seconds" },
          { type: "input_image", detail: "original", image_url: dataUrl },
        ],
      },
    },
    companion,
    structuredClone(companion),
  ];
}

function newerProseRecords() {
  const start = Date.now() + 100;
  return Array.from({ length: 30 }, (_, index) => ({
    timestamp: new Date(start + index).toISOString(),
    type: "item.completed",
    item: {
      id: `canvas-media-newer-${index}`,
      type: "agent_message",
      text: `newer media history witness ${index + 1}`,
    },
  }));
}

async function triggerRecords(worktree, records) {
  await publishTrigger(
    worktree,
    RECORD_TRIGGER,
    `${records.map((record) => JSON.stringify(record)).join("\n")}\n`,
  );
}

async function mediaCard(surface, key) {
  const card = surface.locator(`[data-mosaic-card-key="${key}"]`);
  await card.waitFor({ state: "attached", timeout: TRANSITION_MS * 4 });
  await waitForPageFunction(
    surface.page(),
    "production Codex media reaches loaded Canvas state",
    TRANSITION_MS * 4,
    (expected) => document
      .querySelector(`[data-mosaic-card-key="${expected}"] img[alt="tool-image"]`)
      ?.dataset.mosaicImageState === "loaded",
    key,
  );
  return card;
}

async function cardIdentity(card) {
  return card.evaluate((article) => ({
    key: article.dataset.mosaicCardKey,
    activity: article.dataset.agentActivity,
    aria: article.getAttribute("aria-label"),
    text: article.querySelector(".message-body")?.textContent?.trim(),
    tool: article.querySelector(".message-tool-label")?.textContent,
    agent: article.querySelector(".message-agent-name")?.getAttribute("aria-label"),
    source: article.querySelector('img[alt="tool-image"]')?.getAttribute("src"),
    media: article.querySelector('img[alt="tool-image"]')?.dataset.mediaIdentity,
  }));
}

async function reopenLane(page, lineage, lane) {
  await chooseCanvasView(page, "Lanes");
  const surface = page.locator(`.team-surface[data-lineage-id="${lineage}"]`);
  await surface.waitFor({ timeout: TRANSITION_MS * 4 });
  await surface.getByRole("tab", { name: /^(Lanes|Agents)/ }).click();
  await surface.locator(`.team-agent[data-agent-lane="${lane}"]`).waitFor();
  return surface;
}

async function recordHistoryRequestsAfterReload(page) {
  await page.context().addInitScript(() => {
    const ParentWebSocket = window.WebSocket;
    window.__mediaHistoryRequests = [];
    window.WebSocket = class MediaHistoryWebSocket extends ParentWebSocket {
      send(data) {
        try {
          const text = typeof data === "string"
            ? data
            : new TextDecoder().decode(
              ArrayBuffer.isView(data)
                ? new Uint8Array(data.buffer, data.byteOffset, data.byteLength)
                : new Uint8Array(data),
            );
          const frame = JSON.parse(text);
          if (frame.stream === "message-history") window.__mediaHistoryRequests.push(frame);
        } catch {
          // The production writer owns validation; this recorder observes only JSON requests.
        }
        return super.send(data);
      }
    };
  });
}

export async function assertBidirectionalMediaExchange(page, fixture, worktree) {
  await reply(fixture, [
    "lane", "add", worktree, "--name", "Media", "--life", "Steer", "--vendor", "codex",
  ]);
  const snapshot = await reply(fixture, ["lane"]);
  const member = snapshot.body.lanes.find((lane) => lane.worktree === worktree);
  assert.ok(member, "the production media fixture seats one Codex lane");
  const wake = await reply(fixture, ["lane", "wake", member.lane]);
  assert.equal(wake.body.to, "occupied", "the Codex media stand-in becomes live");
  let surface = await reopenLane(page, member.lineage_id, member.lane);
  const composer = surface.locator(
    `.team-agent[data-agent-lane="${member.lane}"] .composer-shard--primary`,
  );
  await composer.getByRole("textbox").fill("Inspect the attached operator pixel.");
  await pasteImages(composer, ["operator-pixel.png"]);
  await composer.getByRole("img", { name: "operator-pixel.png" }).waitFor();
  await composer.getByRole("textbox").press("Control+Enter");
  await waitForPageFunction(
    page,
    "the real Canvas attachment submission clears its composer",
    TRANSITION_MS,
    (lane) => document
      .querySelector(`.team-agent[data-agent-lane="${lane}"] textarea`)
      ?.value === "",
    member.lane,
  );

  const inbox = await reply(fixture, ["inbox", member.lane]);
  const directive = inbox.body.lanes[0].pending.find(
    (entry) => entry.text.includes("Inspect the attached operator pixel."),
  );
  assert.ok(directive, "the Canvas attachment reaches the authoritative lane inbox");
  assert.equal(directive.attachments.length, 1);
  assert.equal(directive.attachments[0].name, "operator-pixel.png");
  const targetContext = hookContext(await invokeHook(worktree, fixture, member.lane));
  assert.doesNotMatch(targetContext, /data:image|iVBORw0KG/, "agent context contains no payload bytes");
  const projections = contextRecords(targetContext, "media-projection=");
  const references = contextRecords(targetContext, "media-reference=");
  const projection = projections.find((entry) => entry.directive === directive.directive);
  const reference = references.find((entry) => entry.directive === directive.directive);
  assert.ok(projection && reference, "the addressed lane receives reference and projection authority");
  assert.deepEqual(
    {
      lane: projection.lane,
      directive: projection.directive,
      ordinal: projection.ordinal,
      digest: projection.digest,
      bytes: projection.bytes,
      content_type: projection.content_type,
      standing: projection.standing,
    },
    {
      lane: member.lane,
      directive: directive.directive,
      ordinal: reference.ordinal,
      digest: reference.digest,
      bytes: reference.bytes,
      content_type: reference.content_type,
      standing: "live",
    },
  );
  assert.ok(
    projection.path.includes(join(".spice-rs", "steering-media", member.lane)),
    "the projection stays beneath the admitted lane-private root",
  );

  const other = snapshot.body.lanes.find((lane) => lane.lane !== member.lane);
  assert.ok(other, "the fixture retains a wrong-lane falsification target");
  const otherContext = hookContext(await invokeHook(other.worktree, fixture, other.lane));
  assert.ok(
    !otherContext.includes(projection.path) && !otherContext.includes(directive.directive),
    "another lane cannot resolve the target projection through its hook surface",
  );

  await rm(join(worktree, READ_RESULT), { force: true });
  await publishTrigger(worktree, READ_TRIGGER, projection.path);
  const agentBytes = await waitForAgentRead(worktree);
  const expectedBytes = Buffer.from(PIXEL_PNG, "base64");
  assert.deepEqual(agentBytes, expectedBytes, "the running Codex stand-in reads exact projected bytes");
  assert.deepEqual(
    await readFile(projection.path),
    expectedBytes,
    "the projection itself retains exact decoded attachment bytes",
  );

  const dataUrl = `data:image/png;base64,${agentBytes.toString("base64")}`;
  await triggerRecords(worktree, codexMediaRecords(projection.path, dataUrl));
  await waitForPageFunction(
    page,
    "one production Codex image result reaches Canvas",
    TRANSITION_MS * 4,
    (lane) => document.querySelectorAll(
      `.team-surface .mosaic-message-card[data-agent-lane="${lane}"] img[alt="tool-image"]`,
    ).length === 1,
    member.lane,
  );
  let card = surface.locator(
    `.mosaic-message-card[data-agent-lane="${member.lane}"]:has(img[alt="tool-image"])`,
  );
  assert.equal(await card.count(), 1, "call, result, and duplicate companions render one image card");
  const key = await card.getAttribute("data-mosaic-card-key");
  assert.ok(key, "the production media card has durable source identity");
  card = await mediaCard(surface, key);
  const identity = await cardIdentity(card);
  assert.equal(identity.activity, "tool");
  assert.match(identity.aria, /^Tool result from /);
  assert.match(identity.text, /^exec: .*view_image.* completed$/);
  assert.equal(identity.tool, "Tool");
  assert.match(identity.agent, /^Copy agent id for /);
  assert.equal(identity.source, dataUrl);
  assert.match(identity.media, /^[0-9a-f]{64}$/);
  const preview = card.locator('.message-image:has(img[alt="tool-image"])');
  await assertImageLifecycle(preview, "production Codex tool image", "loaded");
  await assertImagePreview(preview, "production Codex tool image", 140);
  await assertFullSizeTarget(preview, "production Codex tool image", dataUrl);
  await openAndAssertIsolated(page, preview, "production Codex tool image", /^blob:/);

  const canceled = await reply(fixture, ["steer", "cancel", directive.directive]);
  assert.equal(canceled.body.disposition, "canceled");
  const retiredContext = hookContext(await invokeHook(worktree, fixture, member.lane));
  assert.ok(!retiredContext.includes(projection.path), "retired projection is absent from the hook");
  await assert.rejects(readFile(projection.path), { code: "ENOENT" });
  await triggerRecords(worktree, [codexMediaRecords(projection.path, dataUrl)[2]]);
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  assert.equal(
    await surface.locator(
      `.mosaic-message-card[data-agent-lane="${member.lane}"]:has(img[alt="tool-image"])`,
    ).count(),
    1,
    "a stale ImageView companion creates no second card",
  );

  await page.reload({ waitUntil: "networkidle" });
  surface = await reopenLane(page, member.lineage_id, member.lane);
  card = await mediaCard(surface, key);
  assert.deepEqual(await cardIdentity(card), identity, "reconnect preserves the exact media card");

  const newer = newerProseRecords();
  await triggerRecords(worktree, newer);
  await surface.locator(
    `.mosaic-message-card[data-agent-lane="${member.lane}"]`,
  ).filter({ hasText: "newer media history witness 30" })
    .waitFor({ timeout: TRANSITION_MS * 4 });
  await recordHistoryRequestsAfterReload(page);
  await page.reload({ waitUntil: "networkidle" });
  surface = await reopenLane(page, member.lineage_id, member.lane);
  await waitForPageFunction(
    page,
    "the bounded reconnect requests older member history",
    TRANSITION_MS * 4,
    (lane) => window.__mediaHistoryRequests.some((request) => request.lane === lane),
    member.lane,
  );
  const historyRequests = await page.evaluate(() => window.__mediaHistoryRequests);
  assert.ok(
    historyRequests.every((request) => request.stream === "message-history"),
    "the reload recorder retains only typed history requests",
  );
  const plane = surface.locator(".lane-mosaic-plane");
  await plane.evaluate((element) => {
    element.scrollTop = 0;
    element.dispatchEvent(new Event("scroll"));
  });
  card = await mediaCard(surface, key);
  assert.deepEqual(
    await cardIdentity(card),
    identity,
    "bounded older-history paging restores the exact media card",
  );
  assert.equal(
    await surface.locator(
      `.mosaic-message-card[data-agent-lane="${member.lane}"]:has(img[alt="tool-image"])`,
    ).count(),
    1,
    "history restoration retains one tool-image card",
  );
}
