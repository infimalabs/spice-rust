// Per-organization watchband and reference-viewport proofs for the real Canvas fixture.
import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import {
  agentNames,
  assertFourAgentDensity,
  assertMosaicLattice,
} from "./agent-conversation.mjs";
import { primaryDestination } from "./composer-shards.mjs";
import { assertPrimaryComposerDragging } from "./composer-dragging.mjs";
import {
  assertReferenceOrgActions,
  captureReferenceControls,
} from "./control-reference.mjs";
import { assertLineageResponsiveLayout } from "./lineage-responsive.mjs";
import { assertOrgNavigation, ORG_DESTINATION_LABELS } from "./organization-navigation.mjs";
import {
  chooseCanvasView,
  reply,
  replyAsLane,
  streamFixtureAssistantMessage,
  CONNECT_MS,
  TRANSITION_MS,
  waitForLocator as waitForElement,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

const RANGE_THUMB_INSET_PX = 8;
const RANGE_DRAG_STEPS = 24;

async function dragRangeToMinimum(page, range) {
  const bounds = await range.boundingBox();
  assert.ok(bounds, "the lifetime range has pointer geometry");
  const y = bounds.y + bounds.height / 2;
  await page.mouse.move(bounds.x + bounds.width - RANGE_THUMB_INSET_PX, y);
  await page.mouse.down();
  await page.mouse.move(bounds.x + RANGE_THUMB_INSET_PX, y, { steps: RANGE_DRAG_STEPS });
  await page.mouse.up();
}

async function clickRangeMaximum(range) {
  const bounds = await range.boundingBox();
  assert.ok(bounds, "the lifetime range has pointer geometry");
  await range.click({
    position: {
      x: bounds.width - RANGE_THUMB_INSET_PX,
      y: bounds.height / 2,
    },
  });
}

async function installLifetimeCommandProbe(page, missingLineage) {
  await page.evaluate((missingLineage) => {
    window.__canvasLifetimeCommands = [];
    window.__holdCanvasLifetimeCommand = false;
    window.__releaseCanvasLifetimeCommand = null;
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    const send = socket.send.bind(socket);
    let intercepted = false;
    const rewrite = (bytes) => {
      const command = JSON.parse(new TextDecoder().decode(bytes));
      const policy = command.kind === "lineage::configure"
        && Object.hasOwn(command.body, "life");
      if (policy) window.__canvasLifetimeCommands.push(command.body.life);
      if (!intercepted && policy) {
        intercepted = true;
        command.body.lineage_id = missingLineage;
        bytes = new TextEncoder().encode(JSON.stringify(command));
      }
      return { bytes, policy };
    };
    const transmit = (bytes, text) => {
      const rewritten = rewrite(bytes);
      const data = text ? new TextDecoder().decode(rewritten.bytes) : rewritten.bytes;
      if (rewritten.policy && window.__holdCanvasLifetimeCommand) {
        window.__holdCanvasLifetimeCommand = false;
        window.__releaseCanvasLifetimeCommand = () => {
          window.__releaseCanvasLifetimeCommand = null;
          send(data);
        };
        return;
      }
      send(data);
    };
    socket.send = (data) => {
      if (typeof data === "string") {
        transmit(new TextEncoder().encode(data), true);
      } else if (data instanceof Blob) {
        void data.arrayBuffer().then((buffer) => transmit(new Uint8Array(buffer), false));
      } else {
        transmit(new Uint8Array(data.buffer ?? data, data.byteOffset ?? 0, data.byteLength), false);
      }
    };
  }, missingLineage);
}

async function assertFinalLifetimePointerChoice(page, range, lineage) {
  const commandsBeforeGesture = await page.evaluate(() => window.__canvasLifetimeCommands.length);
  await page.evaluate(() => { window.__holdCanvasLifetimeCommand = true; });
  await dragRangeToMinimum(page, range);
  await waitForPage(
    page,
    "the final lifetime choice is held while pending",
    TRANSITION_MS,
    (lineage) => document
      .querySelector(`.organization-surface[data-lineage-id="${lineage}"] .organization-composer-controls`)
      ?.dataset.lifetimePending === "true"
      && typeof window.__releaseCanvasLifetimeCommand === "function",
    lineage,
  );
  assert.equal(await range.isDisabled(), true, "the pending final choice disables the range");
  assert.deepEqual(
    await page.evaluate((before) => window.__canvasLifetimeCommands.slice(before), commandsBeforeGesture),
    ["steer"],
    "the complete Drain-to-Steer pointer gesture emits only its final choice",
  );
  await page.evaluate(() => window.__releaseCanvasLifetimeCommand());
}

async function settleLatestSystemSpeech(page, event = "end") {
  await page.evaluate((event) => {
    const utterance = window.__speechUtterances.at(-1);
    if (!utterance) throw new Error("no system utterance is available to settle");
    const callback = event === "error" ? utterance.onerror : utterance.onend;
    if (!callback) throw new Error(`system utterance has no ${event} callback`);
    callback(new Event(event));
  }, event);
}

async function triggerAgentProse(worktree, cards) {
  const before = await cards.count();
  await writeFile(join(worktree, ".spice-prose-trigger"), "emit\n");
  await cards.nth(before).waitFor({ timeout: CONNECT_MS });
}

async function streamAgentProse(worktree, cards, text, { final = false } = {}) {
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
  await streamFixtureAssistantMessage(worktree, message);
  const card = cards.nth(before);
  await card.waitFor({ timeout: CONNECT_MS });
  return card;
}

/// Steer the narrator lane, answer with an ACK packet the resident
/// authenticates, retire its key the way the agent does, and return the
/// acknowledged card once its narration cursor has moved past it. The spoken
/// packet awaits retirement first; only the steer::acked fact credits it. An
/// acked acknowledgement is the second automatic-speech kind beside a final
/// answer, classified separately at the resident.
async function streamAcknowledgedAck(page, fixture, narrator, surface, cards, body) {
  const sent = await reply(fixture, ["steer", narrator.lane, `acknowledge ${crypto.randomUUID()}`]);
  const card = await streamAgentProse(narrator.worktree, cards, `ACK ${sent.body.key}: ${body}`);
  const key = await card.getAttribute("data-mosaic-card-key");
  await waitForPage(page, `the "${body}" packet awaits retirement as one pending ACK`, CONNECT_MS,
    (key) => document.querySelector(`[data-mosaic-card-key="${key}"]`)?.dataset.messagePendingCount
      === "1", key);
  await replyAsLane(narrator.worktree, narrator.lane, ["ack", sent.body.key]);
  await waitForPage(page, `the "${body}" packet is authenticated as one ACK`, CONNECT_MS,
    (key) => document.querySelector(`[data-mosaic-card-key="${key}"]`)?.dataset.messageAckCount
      === "1", key);
  await waitForNarrationCursor(surface, key);
  return card;
}

async function waitForNarrationCursor(surface, key) {
  await surface.locator(".lane-mosaic-plane").evaluate((plane, expected) => (
    new Promise((resolve, reject) => {
      const complete = () => {
        if (plane.dataset.narrationCursor !== expected.key) return;
        clearTimeout(deadline);
        observer.disconnect();
        resolve();
      };
      const observer = new MutationObserver(complete);
      observer.observe(plane, {
        attributes: true,
        attributeFilter: ["data-narration-cursor"],
      });
      const deadline = setTimeout(() => {
        observer.disconnect();
        reject(new Error(`narration cursor did not adopt ${expected.key}`));
      }, expected.deadlineMs);
      complete();
    })
  ), { key, deadlineMs: TRANSITION_MS });
}

export async function assertPerOrgWatchbands(
  page,
  fixture,
  worktrees,
  { artifacts, staleEpoch },
) {
  const before = await reply(fixture, ["lane"]);
  const primary = before.body.lanes.find((lane) => lane.worktree === worktrees[0]);
  assert.ok(primary, "the primary organization remains seated for watchband proof");
  const added = await reply(fixture, [
    "lane", "add", worktrees[4],
    "--life", "Drive",
    "--vendor", "claude",
  ]);
  const awakened = await reply(fixture, ["lane", "wake", added.body.lane]);
  assert.equal(awakened.body.to, "occupied", "the secondary organization starts a live vendor");
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Lanes");
  const console = page.locator('[data-pane="agent-console"]');
  await console.getByRole("tab", { name: /^Agents/ }).first().click();
  await waitForElement(page, "second organization renders after reload", CONNECT_MS,
    console.locator(".organization-surface").nth(1));

  const lanes = await reply(fixture, ["lane"]);
  const secondary = lanes.body.lanes.find((lane) => lane.worktree === worktrees[4]);
  assert.ok(secondary, "the second organization is independently seated");
  const primarySurface = console.locator(
    `.organization-surface[data-lineage-id="${primary.lineage_id}"]`,
  );
  const secondarySurface = console.locator(
    `.organization-surface[data-lineage-id="${secondary.lineage_id}"]`,
  );
  for (const [surface, label] of [
    [primarySurface, "primary"],
    [secondarySurface, "secondary"],
  ]) {
    await assertOrgNavigation(surface, `${label} populated organization`);
  }
  const primaryTabs = primarySurface.getByRole("tab");
  await primaryTabs.first().focus();
  for (const [index, label] of ORG_DESTINATION_LABELS.entries()) {
    assert.match(
      (await page.locator(":focus").innerText()).trim(),
      new RegExp(`^${label}`),
      `organization keyboard traversal follows visible order at ${label}`,
    );
    if (index < ORG_DESTINATION_LABELS.length - 1) await page.keyboard.press("Tab");
  }
  const selectedMode = (surface) => surface.locator(".organization-pane").getAttribute("data-organization-mode");
  assert.deepEqual(
    [await selectedMode(primarySurface), await selectedMode(secondarySurface)],
    ["Lanes", "Lanes"],
    "each newly rendered organization begins in Lanes",
  );
  await primarySurface.getByRole("tab", { name: /^Team/ }).click();
  assert.deepEqual(
    [await selectedMode(primarySurface), await selectedMode(secondarySurface)],
    ["Org", "Lanes"],
    "switching the primary organization to Org leaves its sibling in Lanes",
  );
  await secondarySurface.getByRole("tab", { name: /^Team/ }).click();
  assert.deepEqual(
    [await selectedMode(primarySurface), await selectedMode(secondarySurface)],
    ["Org", "Org"],
    "two organizations retain different selected views simultaneously",
  );
  await primarySurface.getByRole("tab", { name: /^Board/ }).click();
  assert.deepEqual(
    [await selectedMode(primarySurface), await selectedMode(secondarySurface)],
    ["Tasks", "Org"],
    "a subsequent primary switch preserves the secondary selection",
  );
  await assertLineageResponsiveLayout(page, console);
  await secondarySurface.getByRole("tab", { name: /^Agents/ }).click();
  assert.deepEqual(
    [await selectedMode(primarySurface), await selectedMode(secondarySurface)],
    ["Tasks", "Lanes"],
    "a subsequent secondary switch preserves the primary selection",
  );
  await primarySurface.getByRole("tab", { name: /^Agents/ }).click();
  const noCardDestination = await primaryDestination(secondarySurface, secondary.lane);
  assert.equal(
    noCardDestination.rendered.length,
    0,
    "the freshly seated secondary agent exercises the primary lineage default before any card",
  );
  const firstSecondaryDirection = "the lineage default precedes this first rendered card";
  await secondarySurface.getByRole("textbox").fill(firstSecondaryDirection);
  await secondarySurface.getByRole("button", { name: "Drive", exact: true }).click();
  const firstSecondaryInbox = await reply(fixture, ["inbox", secondary.lane]);
  assert.ok(
    firstSecondaryInbox.body.lanes[0].pending.some(
      (directive) => directive.text === firstSecondaryDirection,
    ),
    "the first secondary direction reaches its durable inbox",
  );
  assert.equal(
    await secondarySurface.locator(".mosaic-message-card")
      .filter({ hasText: firstSecondaryDirection }).count(),
    0,
    "unacknowledged secondary prose is absent from the conversation",
  );
  await secondarySurface.getByRole("tab", { name: /^Board/ }).click();
  assert.equal(
    await secondarySurface.locator(".organization-task-eyebrow").innerText(),
    "Team work",
    "the empty Board workspace uses the Team presentation name",
  );
  const queuedDirection = secondarySurface.locator(
    `.organization-direction-row[data-agent-lane="${secondary.lane}"]`,
  );
  await queuedDirection.waitFor({ timeout: TRANSITION_MS });
  assert.equal(
    (await queuedDirection.locator(".organization-direction-preview").innerText()).trim(),
    firstSecondaryDirection,
    "Board shows the admitted newest direction preview without parsing authored text",
  );
  assert.equal(
    (await queuedDirection.locator(".organization-direction-count").innerText()).trim(),
    "1 pending",
    "Board keeps queued direction separate from active task count",
  );
  assert.equal(
    await secondarySurface.locator(".organization-task-row").count(),
    0,
    "queued direction is not presented as allocator-eligible work",
  );
  const secondaryTaskSections = secondarySurface.locator(
    ".organization-task-workspace > .organization-task-section",
  );
  assert.deepEqual(
    await secondaryTaskSections.getByRole("heading", { level: 3 }).allTextContents(),
    ["Active tasks", "Direction queue"],
    "active ownership remains ahead of queued direction",
  );
  const emptyActive = secondaryTaskSections.first().locator(".organization-task-empty");
  assert.equal(
    (await emptyActive.innerText()).trim(),
    "No active tasks for this organization.",
    "the production empty active state remains explicit",
  );
  assert.equal(
    await emptyActive.getAttribute("role"),
    null,
    "ordinary emptiness is not announced as an unavailable snapshot",
  );
  await secondarySurface.getByRole("tab", { name: /^Agents/ }).click();
  assert.equal(await console.locator(".organization-surface").count(), 2);
  assert.equal(await console.locator(".organization-speech").count(), 2);
  assert.equal(await console.locator(".organization-lifetime").count(), 2);
  const weightedWidths = await console.locator(".organization-surface").evaluateAll((surfaces) =>
    surfaces.map((surface) => ({
      weight: Number(surface.dataset.lineageWeight),
      width: surface.getBoundingClientRect().width,
    })),
  );
  const widthsByWeight = weightedWidths.toSorted((left, right) => left.weight - right.weight);
  assert.deepEqual(
    widthsByWeight.map(({ weight }) => weight),
    [1, 4],
    "lineage weights remain the admitted member counts",
  );
  assert.ok(
    widthsByWeight[1].width > widthsByWeight[0].width,
    "the four-agent lineage remains wider than its one-agent sibling",
  );
  const modeGeometry = await console.locator(".organization-surface").evaluateAll((surfaces) =>
    surfaces.map((surface) => ({
      width: surface.getBoundingClientRect().width,
      labels: [...surface.querySelectorAll(".organization-mode > span:first-child")].map((label) => {
        const button = label.parentElement.getBoundingClientRect();
        const rect = label.getBoundingClientRect();
        return {
          text: label.textContent,
          left: rect.left,
          right: rect.right,
          contained: rect.left >= button.left && rect.right <= button.right,
        };
      }),
    })),
  );
  assert.ok(
    modeGeometry.every(({ labels }) => labels.every((label, index) =>
      label.contained && (index === 0 || labels[index - 1].right <= label.left))),
    `organization-local tabs fit without overlapping at every lineage width: ${JSON.stringify(modeGeometry)}`,
  );

  const primaryLife = primarySurface.locator('.organization-lifetime input[type="range"]');
  const primarySeatLives = new Map(
    lanes.body.lanes
      .filter((lane) => lane.lineage_id === primary.lineage_id)
      .map((lane) => [lane.lane, lane.life]),
  );
  const initialPrimaryPolicy = lanes.body.subscriptions.find(
    (subscription) => subscription.lineage_id === primary.lineage_id,
  )?.policy;
  assert.ok(initialPrimaryPolicy, "the primary organization has one authoritative task policy");
  const initialPrimaryLabel = `${initialPrimaryPolicy[0].toUpperCase()}${initialPrimaryPolicy.slice(1)}`;
  const initialPrimaryIndex = { steer: "0", drive: "1", drain: "2" }[initialPrimaryPolicy];
  await installLifetimeCommandProbe(page, staleEpoch);
  await primaryLife.press("End");
  await waitForPage(
    page,
    "rejected lifetime command settles",
    TRANSITION_MS,
    (lineage) => document
      .querySelector(`.organization-surface[data-lineage-id="${lineage}"] .organization-composer-controls`)
      ?.dataset.lifetimePending === "false",
    primary.lineage_id,
  );
  await primarySurface
    .getByRole("button", { name: initialPrimaryLabel, exact: true })
    .waitFor();
  assert.equal(await primaryLife.inputValue(), initialPrimaryIndex);
  assert.match(await primarySurface.locator(".organization-control-issue").innerText(), /lineage::missing/);

  await primaryLife.press("End");
  await waitForPage(
    page,
    "accepted lifetime command settles",
    TRANSITION_MS,
    (lineage) => document
      .querySelector(`.organization-surface[data-lineage-id="${lineage}"] .organization-composer-controls`)
      ?.dataset.lifetimePending === "false",
    primary.lineage_id,
  );
  await primarySurface.getByRole("button", { name: "Drain", exact: true }).waitFor();
  const configured = await reply(fixture, ["lane"]);
  const primaryRows = configured.body.lanes.filter(
    (lane) => lane.lineage_id === primary.lineage_id,
  );
  const primarySubscription = configured.body.subscriptions.find(
    (subscription) => subscription.lineage_id === primary.lineage_id,
  );
  assert.equal(primaryRows.length, 4, "the first organization retains all four worktrees");
  assert.equal(primarySubscription?.policy, "drain");
  const primaryNarrator = primaryRows.find((lane) => lane.worktree === worktrees[0]);
  assert.ok(primaryNarrator, "the first organization exposes a real narrator worktree");
  const primaryNarrationCards = primarySurface.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${primaryNarrator.lane}"]`,
  );
  assert.ok(
    primaryRows.every((lane) => lane.life === primarySeatLives.get(lane.lane)),
    "watchband Drain never changes terminal lane life",
  );
  assert.ok(
    primaryRows.every((lane) => lane.occupant && lane.disposition === "occupied"),
    "watchband Drain changes task policy and never tears down occupants",
  );
  await primarySurface.getByRole("tab", { name: /^Board/ }).click();
  assert.equal(
    await primarySurface.locator('.organization-task-policy[aria-label="Task policy: Drain"]').count(),
    1,
    "Board reports the authoritative organization task policy once",
  );
  assert.equal(
    await primarySurface.getByText("Life", { exact: true }).count(),
    0,
    "Board does not expose the ambiguous per-seat Life label",
  );
  for (const seatLife of new Set(primaryRows.map((lane) => lane.life))) {
    assert.equal(
      await primarySurface.getByText(seatLife, { exact: true }).count(),
      0,
      `Board does not present seat policy ${seatLife} as organization task state`,
    );
  }
  const taskWorkspace = primarySurface.locator(".organization-task-workspace");
  const taskGeometry = async () => taskWorkspace.evaluate((workspace) => {
    const bounds = workspace.getBoundingClientRect();
    const rows = [...workspace.querySelectorAll(".organization-task-row, .organization-direction-row")]
      .map((row) => {
        const rect = row.getBoundingClientRect();
        const title = row.querySelector(".organization-task-title");
        return {
          card: row.classList.contains("organization-task-row"),
          borderRadius: getComputedStyle(row).borderRadius,
          left: rect.left,
          right: rect.right,
          width: rect.width,
          titleWhiteSpace: title ? getComputedStyle(title).whiteSpace : null,
          textOverflow: row.scrollWidth - row.clientWidth,
        };
      });
    return {
      clientWidth: workspace.clientWidth,
      eyebrow: workspace.querySelector(".organization-task-eyebrow").textContent,
      scrollWidth: workspace.scrollWidth,
      left: bounds.left,
      right: bounds.right,
      rows,
      lists: [...workspace.querySelectorAll(".organization-task-list, .organization-direction-list")]
        .map((list) => ({ className: list.className, tagName: list.tagName })),
      sections: [...workspace.querySelectorAll(":scope > .organization-task-section h3")]
        .map((heading) => heading.textContent),
    };
  });
  const assertTaskGeometry = (geometry, viewport) => {
    assert.equal(geometry.eyebrow, "Team work", `${viewport} Board uses the Team presentation name`);
    assert.ok(
      geometry.clientWidth > 0 && geometry.scrollWidth <= geometry.clientWidth,
      `${viewport} Board workspace has visible width without horizontal overflow: ${
        JSON.stringify(geometry)
      }`,
    );
    assert.ok(
      geometry.rows.every((row) => row.width > 0
        && row.left >= geometry.left
        && row.right <= geometry.right
        && row.textOverflow <= 0),
      `${viewport} task and direction rows stay inside the workspace: ${JSON.stringify(geometry)}`,
    );
    assert.deepEqual(geometry.sections, ["Active tasks", "Direction queue"]);
    assert.ok(
      geometry.lists.length > 0 && geometry.lists.every((list) => list.tagName === "OL"),
      `${viewport} populated sections retain ordered lists: ${JSON.stringify(geometry)}`,
    );
    assert.ok(
      geometry.rows.filter((row) => row.card).every((row) => (
        row.titleWhiteSpace === "normal" && Number.parseFloat(row.borderRadius) > 0
      )),
      `${viewport} active tasks retain wrapped semantic cards: ${JSON.stringify(geometry)}`,
    );
  };
  assertTaskGeometry(await taskGeometry(), "desktop");
  await page.screenshot({
    path: join(artifacts, "canvas-tasks-organization-desktop.png"),
    fullPage: true,
  });
  await page.setViewportSize({ width: 390, height: 844 });
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(resolve)));
  assertTaskGeometry(await taskGeometry(), "narrow");
  await page.screenshot({
    path: join(artifacts, "canvas-tasks-organization-narrow.png"),
    fullPage: true,
  });
  await page.setViewportSize({ width: 1440, height: 900 });
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(resolve)));
  await primarySurface.getByRole("tab", { name: /^Agents/ }).click();
  assert.equal(
    await secondarySurface.getByRole("button", { name: "Drive", exact: true }).count(),
    1,
    "the sibling organization keeps its own authoritative lifetime",
  );
  const secondaryCards = secondarySurface.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${secondary.lane}"]`,
  );
  await triggerAgentProse(secondary.worktree, secondaryCards);

  const primarySpeech = primarySurface.locator('.organization-speech input[type="range"]');
  const secondarySpeech = secondarySurface.locator('.organization-speech input[type="range"]');
  assert.deepEqual(
    [await primarySpeech.inputValue(), await secondarySpeech.inputValue()],
    ["1", "1"],
    "new organizations default to Speak",
  );
  await primarySpeech.fill("0");
  await secondarySpeech.fill("2");
  assert.equal(await primarySpeech.inputValue(), "0");
  assert.equal(await secondarySpeech.inputValue(), "2");
  assert.equal(await primarySurface.locator(".organization-speech span").innerText(), "quiet");
  assert.equal(await secondarySurface.locator(".organization-speech span").innerText(), "narrate");
  await console.getByRole("tab", { name: /^Team/ }).first().click();
  await console.getByRole("tab", { name: /^Agents/ }).first().click();
  assert.equal(await primarySpeech.inputValue(), "0", "Quiet survives an organization redraw");
  assert.equal(await secondarySpeech.inputValue(), "2", "Narrate survives an organization redraw");
  assert.equal(await primarySurface.locator(".organization-speech span").innerText(), "quiet");
  assert.equal(await secondarySurface.locator(".organization-speech span").innerText(), "narrate");

  await page.evaluate(() => {
    window.__speechLog = [];
    window.__speechUtterances = [];
  });
  const quietFinal = await streamAgentProse(
    primaryNarrator.worktree,
    primaryNarrationCards,
    "quiet final answer",
    { final: true },
  );
  await waitForNarrationCursor(
    primarySurface,
    await quietFinal.getAttribute("data-mosaic-card-key"),
  );
  assert.deepEqual(
    await page.evaluate(() => window.__speechLog),
    [],
    "Quiet hands a final answer to neither speechSynthesis nor an audio element",
  );
  await streamAcknowledgedAck(
    page,
    fixture,
    primaryNarrator,
    primarySurface,
    primaryNarrationCards,
    "quiet acknowledgement",
  );
  assert.deepEqual(
    await page.evaluate(() => window.__speechLog),
    [],
    "Quiet hands an acked ACK to neither speechSynthesis nor an audio element",
  );
  await primarySpeech.fill("1");
  assert.equal(await primarySurface.locator(".organization-speech span").innerText(), "speak");
  const spokenFinal = await streamAgentProse(
    primaryNarrator.worktree,
    primaryNarrationCards,
    "spoken final answer",
    { final: true },
  );
  const spokenFinalKey = await spokenFinal.getAttribute("data-mosaic-card-key");
  await waitForNarrationCursor(primarySurface, spokenFinalKey);
  await waitForPage(page, "Speak dispatches the final answer it is bound to", TRANSITION_MS,
    () => window.__speechLog.filter((row) => row.action === "speak").length === 1);
  assert.deepEqual(
    await page.evaluate(() => window.__speechLog.map((row) => [row.action, row.text])),
    [["speak", "spoken final answer"]],
    "returning to Speak speaks only the new final answer, never the Quiet-era one",
  );
  assert.equal(
    await spokenFinal.getAttribute("data-speech-active"),
    "true",
    "the automatic utterance marks its message card while the page is visible",
  );
  await page.evaluate(() => window.dispatchEvent(new Event("pagehide")));
  await waitForPage(page, "pagehide cancels the in-flight system utterance", TRANSITION_MS,
    () => window.__speechLog.at(-1)?.action === "cancel" && window.__speechActive === 0);
  await waitForPage(page, "pagehide releases the message card", TRANSITION_MS,
    (key) => document.querySelector(`[data-mosaic-card-key="${key}"]`)?.dataset.speechActive
      === "false", spokenFinalKey);
  await page.evaluate(() => { window.__speechLog = []; });
  await streamAcknowledgedAck(
    page,
    fixture,
    primaryNarrator,
    primarySurface,
    primaryNarrationCards,
    "spoken acknowledgement",
  );
  await waitForPage(page, "Speak dispatches the acked ACK it is bound to", TRANSITION_MS,
    () => window.__speechLog.filter((row) => row.action === "speak").length === 1);
  assert.deepEqual(
    await page.evaluate(() => window.__speechLog.map((row) => [row.action, row.text])),
    [["speak", "spoken acknowledgement"]],
    "Speak speaks the acked packet body once, so the Quiet ACK negative is bound to a stimulus that speaks",
  );
  await settleLatestSystemSpeech(page);
  await waitForPage(page, "the spoken acknowledgement releases the global head", TRANSITION_MS,
    () => window.__speechActive === 0);
  await primarySpeech.fill("0");
  assert.equal(await primarySurface.locator(".organization-speech span").innerText(), "quiet");

  await page.evaluate(() => { window.__speechLog = []; });
  const quietManual = primarySurface.getByLabel("Speak this message").first();
  await quietManual.click();
  await waitForPage(page, "Quiet retains explicit manual playback", TRANSITION_MS,
    () => window.__speechLog.some((row) => row.action === "speak"));
  await primarySurface.getByLabel("Stop playback").first().click();
  await waitForPage(page, "Quiet retains explicit manual stop", TRANSITION_MS,
    () => window.__speechActive === 0);
  await page.evaluate(() => {
    window.__speechLog = [];
    window.__speechUtterances = [];
  });
  await secondarySurface.getByLabel("Speak this message").first().click();
  await waitForPage(page, "Narrate dispatches speech", TRANSITION_MS,
    () => window.__speechLog.some((row) => row.action === "speak"));
  await settleLatestSystemSpeech(page);
  await waitForPage(page, "manual speech releases the global head", TRANSITION_MS,
    () => window.__speechActive === 0);

  await primarySpeech.fill("2");
  await page.evaluate(() => {
    window.__speechLog = [];
    window.__speechUtterances = [];
    window.__speechActive = 0;
    window.__speechMaxActive = 0;
  });
  const secondaryNarrationCards = secondarySurface.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${secondary.lane}"]`,
  );
  const secondaryCardsBeforeBurst = await secondaryNarrationCards.count();
  await triggerAgentProse(primaryNarrator.worktree, primaryNarrationCards);
  await waitForPage(page, "the first lineage owns the global narration head", CONNECT_MS,
    () => window.__speechLog.filter((row) => row.action === "speak").length === 1);
  await streamAgentProse(secondary.worktree, secondaryNarrationCards, "older queued narration");
  const newestNarration = await streamAgentProse(
    secondary.worktree,
    secondaryNarrationCards,
    "newest queued narration",
  );
  const newestNarrationKey = await newestNarration.getAttribute("data-mosaic-card-key");
  assert.ok(newestNarrationKey, "the newest queued narration has a stable message key");
  await waitForNarrationCursor(
    secondarySurface,
    newestNarrationKey,
  );
  const messageBackedDestination = await primaryDestination(secondarySurface, secondary.lane);
  assert.equal(messageBackedDestination.rendered.length, secondaryCardsBeforeBurst + 2);
  assert.equal(
    messageBackedDestination.href,
    `#${messageBackedDestination.rendered.at(-1)}`,
    "the former dash retargets to that agent's latest message card",
  );
  assert.equal(
    await page.evaluate(() => window.__speechLog.filter((row) => row.action === "speak").length),
    1,
    "a two-lineage burst preserves the active head and queues its tail",
  );
  const transcriptCards = await console.locator('.mosaic-message-card[data-card-kind="agent-message"]')
    .count();
  await settleLatestSystemSpeech(page, "error");
  await waitForPage(page, "a failed head advances to the coalesced lineage tail", TRANSITION_MS,
    () => window.__speechLog.filter((row) => row.action === "speak").length === 2);
  assert.equal(
    await page.evaluate(() => window.__speechLog.filter((row) => row.action === "speak").at(-1)?.text),
    "newest queued narration",
    "the rapid same-lineage burst advances to its newest tail",
  );
  assert.equal(
    await console.locator('.mosaic-message-card[data-card-kind="agent-message"]').count(),
    transcriptCards,
    "speech failure neither removes nor rewrites the admitted transcript",
  );
  assert.equal(
    await page.evaluate(() => window.__speechMaxActive),
    1,
    "the browser-global narration arbiter never overlaps two clips",
  );
  const storedNarrationCursors = await page.evaluate(() => JSON.parse(
    localStorage.getItem("spice:narration-cursors:v1") ?? "[]",
  ));
  assert.ok(
    storedNarrationCursors.length > 0 && storedNarrationCursors.length <= 256,
    "durable narration cursors are present and bounded in browser storage",
  );
  await settleLatestSystemSpeech(page);
  await waitForPage(page, "the coalesced narration tail drains", TRANSITION_MS,
    () => window.__speechActive === 0);
  assert.equal(
    await page.evaluate(() => window.__speechLog.filter((row) => row.action === "speak").length),
    2,
    "two stale entries from one queued lineage coalesce to one successor",
  );
  await primarySpeech.fill("0");
  await assertReferenceOrgActions(page, primarySurface);

  const policyMessage = "drain lifetime is task policy, not occupant teardown";
  await primarySurface.locator(".organization-agent").first().getByRole("textbox").fill(policyMessage);
  await primarySurface.getByRole("button", { name: "Drain", exact: true }).click();
  const inbox = await reply(fixture, ["inbox", primaryRows[0].lane]);
  assert.ok(
    inbox.body.lanes[0].pending.some((directive) => directive.text === policyMessage),
    "the Drain-labelled submit follows the configured organization policy into real steering",
  );
  assert.equal(
    await primarySurface.locator(".mosaic-message-card").filter({ hasText: policyMessage }).count(),
    0,
    "unacknowledged Drain prose is absent from the conversation",
  );
  assert.equal(await primarySpeech.inputValue(), "0", "Quiet survives a streamed refresh");
  assert.equal(await secondarySpeech.inputValue(), "2", "Narrate survives a streamed refresh");
  assert.equal(await primarySurface.locator(".organization-speech span").innerText(), "quiet");
  assert.equal(await secondarySurface.locator(".organization-speech span").innerText(), "narrate");

  const renderedPrimaryMembers = await primarySurface.locator(".organization-agent").evaluateAll(
    (members) => members.map((member) => member.dataset.agentLane),
  );
  assert.equal(renderedPrimaryMembers.length, 4, "the quote fixture has four ordered members");
  const lifecycleLane = primaryRows.find((lane) => lane.lane === renderedPrimaryMembers[1]);
  assert.ok(lifecycleLane, "the quote fixture resolves the card's second rendered member");
  assert.notEqual(
    lifecycleLane.lane,
    renderedPrimaryMembers[0],
    "the departed-author card is not the row-order member",
  );
  const departedAuthor = lifecycleLane.occupant;
  const departedCards = primarySurface.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${lifecycleLane.lane}"]`,
  );
  await departedCards.first().waitFor({ timeout: 10_000 });

  // Two lifecycle authorities can consume a vacancy: Drain assigns eligible
  // task work, and any pending directive wakes its target regardless of task
  // policy. Hold the lineage in Steer and retire only this lane's test-owned
  // pending directions while proving the exact manual transition, then restore
  // Drain after the existing explicit wake. This makes vacancy observable
  // without weakening either the drain reply or the durable lane snapshot.
  await assertFinalLifetimePointerChoice(page, primaryLife, primary.lineage_id);
  await waitForPage(
    page,
    "temporary Steer policy settles before manual drain",
    TRANSITION_MS,
    (lineage) => document
      .querySelector(`.organization-surface[data-lineage-id="${lineage}"] .organization-composer-controls`)
      ?.dataset.lifetimePending === "false",
    primary.lineage_id,
  );
  await primarySurface.getByRole("button", { name: "Steer", exact: true }).waitFor();
  const steered = await reply(fixture, ["lane"]);
  assert.equal(
    steered.body.subscriptions.find(
      (subscription) => subscription.lineage_id === primary.lineage_id,
    )?.policy,
    "steer",
    "the task allocator is paused before vacancy is created",
  );
  const pendingBeforeDrain = await reply(fixture, ["inbox", lifecycleLane.lane]);
  for (const directive of pendingBeforeDrain.body.lanes[0].pending) {
    await reply(fixture, ["steer", "cancel", directive.directive]);
  }
  const quietInbox = await reply(fixture, ["inbox", lifecycleLane.lane]);
  assert.deepEqual(
    quietInbox.body.lanes[0].pending,
    [],
    "pending direction cannot automatically consume the vacancy under test",
  );
  const drained = await reply(fixture, ["lane", "drain", lifecycleLane.lane]);
  assert.equal(drained.body.lane, lifecycleLane.lane, "the drain reply names the lifecycle lane");
  assert.equal(drained.body.from, "occupied", "the drain begins at the observed occupant");
  assert.equal(drained.body.to, "vacant", "the drain itself commits the vacancy under test");
  const vacantSnapshot = await reply(fixture, ["lane"]);
  const vacant = vacantSnapshot.body.lanes.find((lane) => lane.lane === lifecycleLane.lane);
  assert.ok(vacant, "the configured member remains durable after leaving");
  assert.equal(vacant.disposition, "vacant");
  assert.equal(vacant.occupant, undefined);
  assert.equal(vacant.life, primarySeatLives.get(vacant.lane));
  assert.equal(
    vacantSnapshot.body.lanes.some((lane) => lane.occupant === departedAuthor),
    false,
    "the author of the second member's retained cards is seated in no lane",
  );
  await assertOrgNavigation(primarySurface, "the organization with a vacant member");
  await waitForPage(page, "departed author cards retain their lane", TRANSITION_MS,
    ({ lineage, lane }) => {
    const cards = [...document.querySelectorAll(
      `.organization-surface[data-lineage-id="${lineage}"] `
        + `[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
    )];
    return cards.length > 0 && cards.every((card) => card.dataset.producerTargetId === lane);
  }, { lineage: primary.lineage_id, lane: lifecycleLane.lane });
  assert.deepEqual(
    new Set(await departedCards.evaluateAll(
      (cards) => cards.map((card) => card.dataset.producerTargetId),
    )),
    new Set([lifecycleLane.lane]),
    "an unseated author routes every retained card to its own lane, never the first member",
  );
  assert.equal(
    vacantSnapshot.body.subscriptions.find(
      (subscription) => subscription.lineage_id === primary.lineage_id,
    )?.policy,
    "steer",
    "Steer keeps the proved vacancy stable until the explicit wake",
  );
  await reply(fixture, ["lane", "wake", lifecycleLane.lane]);
  const wokenSnapshot = await reply(fixture, ["lane"]);
  const woken = wokenSnapshot.body.lanes.find((lane) => lane.lane === lifecycleLane.lane);
  assert.ok(woken?.occupant, "watchband Drain leaves the member eligible for a normal wake");
  assert.equal(woken.disposition, "occupied");
  assert.equal(woken.life, primarySeatLives.get(woken.lane));
  assert.equal(
    await primarySurface.locator(
      '[data-card-kind="lifecycle"][data-lifecycle-cause="drained gracefully"]',
    ).count(),
    0,
    "a successful manual drain leaves no lifecycle divider in the activity plane",
  );
  await clickRangeMaximum(primaryLife);
  await waitForPage(
    page,
    "configured Drain policy is restored after manual wake",
    TRANSITION_MS,
    (lineage) => document
      .querySelector(`.organization-surface[data-lineage-id="${lineage}"] .organization-composer-controls`)
      ?.dataset.lifetimePending === "false",
    primary.lineage_id,
  );
  await primarySurface.getByRole("button", { name: "Drain", exact: true }).waitFor();
  const restored = await reply(fixture, ["lane"]);
  assert.equal(
    restored.body.subscriptions.find(
      (subscription) => subscription.lineage_id === primary.lineage_id,
    )?.policy,
    "drain",
    "the vacancy proof restores the configured organization policy",
  );
  await assertPrimaryComposerDragging(
    page,
    fixture,
    console,
    primary.lineage_id,
    secondary.lineage_id,
    join(artifacts, "canvas-primary-composer-dragging.png"),
  );
  await page.screenshot({
    path: join(artifacts, "canvas-light-watchband-organizations.png"),
    fullPage: true,
  });
  await secondarySpeech.fill("1");
  const defaultSpeakStarted = Date.now();
  const speechesBeforeDefault = await page.evaluate(() =>
    window.__speechLog.filter((row) => row.action === "speak").length);
  await streamAgentProse(
    secondary.worktree,
    secondaryNarrationCards,
    "default Speak consumes the resident-prepared final utterance",
    { final: true },
  );
  await waitForPage(page, "default Speak dispatches the prepared final utterance", CONNECT_MS,
    (before) => window.__speechLog.filter((row) => row.action === "speak").length === before + 1,
    speechesBeforeDefault);
  assert.equal(
    await page.evaluate(() => window.__speechLog.filter((row) => row.action === "speak").at(-1)?.text),
    "default Speak consumes the resident-prepared final utterance",
  );
  await settleLatestSystemSpeech(page);
  process.stdout.write(
    `canvas retained default-Speak proof elapsed_ms=${Date.now() - defaultSpeakStarted}\n`,
  );
  await secondarySpeech.fill("2");
  await page.evaluate(() => {
    window.__speechLog = [];
    window.__speechUtterances = [];
  });
  const reloadSpeechStarted = Date.now();
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Lanes");
  const reconnected = page.locator('[data-pane="agent-console"]');
  await reconnected.getByRole("tab", { name: /^Agents/ }).first().click();
  const restoredPrimary = reconnected.locator(
    `.organization-surface[data-lineage-id="${primary.lineage_id}"]`,
  );
  const restoredSecondary = reconnected.locator(
    `.organization-surface[data-lineage-id="${secondary.lineage_id}"]`,
  );
  await assertOrgNavigation(reconnected, "the reloaded organization selection");
  const restoredPrimarySpeech = restoredPrimary.locator('.organization-speech input[type="range"]');
  const restoredSecondarySpeech = restoredSecondary.locator('.organization-speech input[type="range"]');
  assert.deepEqual(
    [await restoredPrimarySpeech.inputValue(), await restoredSecondarySpeech.inputValue()],
    ["0", "2"],
    "a full reload restores each organization's browser-local speech mode",
  );
  process.stdout.write(
    `canvas retained speech-mode reload proof elapsed_ms=${Date.now() - reloadSpeechStarted}\n`,
  );
  await restoredPrimarySpeech.fill("2");
  await restoredSecondarySpeech.fill("2");
  await reconnected.getByRole("tab", { name: /^Team/ }).first().click();
  await reconnected.getByRole("tab", { name: /^Agents/ }).first().click();
  await page.evaluate(() => new Promise((resolve) =>
    requestAnimationFrame(() => requestAnimationFrame(resolve))));
  assert.deepEqual(
    await page.evaluate(() => window.__speechLog.filter((row) => row.action === "speak")),
    [],
    "reconnect and rematerialization never narrate retained history",
  );
  const restoredNarrationCards = restoredPrimary.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${primaryNarrator.lane}"]`,
  );
  await triggerAgentProse(primaryNarrator.worktree, restoredNarrationCards);
  await waitForPage(page, "live narration resumes after reconnect history suppression", CONNECT_MS,
    () => window.__speechLog.filter((row) => row.action === "speak").length === 1);
  await settleLatestSystemSpeech(page);
  await waitForPage(page, "post-reconnect narration drains", TRANSITION_MS,
    () => window.__speechActive === 0);
  return { lineage: primary.lineage_id, policy: "drain" };
}

export async function captureReferenceViewport(page, artifacts) {
  await chooseCanvasView(page, "Lanes");
  const organization = page.locator('[data-pane="agent-console"]');
  await organization.getByRole("tab", { name: /^Team/ }).click();
  for (const name of agentNames) {
    await organization.locator(".organization-detail-card").getByText(name, { exact: true }).waitFor();
  }
  await organization.getByText("0 pending", { exact: true }).waitFor();
  await organization.getByText("running", { exact: true }).first().waitFor();
  await page.screenshot({
    path: join(artifacts, "canvas-reference-agent-details-dark.png"),
    fullPage: false,
  });
  await organization.getByRole("tab", { name: /^Agents/ }).click();
  await organization.locator(".mosaic-message-card").nth(3).waitFor({ timeout: 10_000 });
  assert.equal(
    await page.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches),
    true,
    "the reference capture explicitly exercises dark mode",
  );
  await assertFourAgentDensity(organization);
  assert.equal(
    await organization.locator(".organization-surface").getAttribute("data-lineage-weight"),
    "4",
    "the reference viewport retains the four-agent lineage weight",
  );
  await assertMosaicLattice(page, false);
  const pixels = await page.evaluate(() => ({
    width: innerWidth,
    height: innerHeight,
    background: getComputedStyle(document.body).backgroundColor,
    bodyWidth: document.body.scrollWidth,
  }));
  assert.deepEqual([pixels.width, pixels.height], [1000, 568]);
  assert.equal(
    await page.locator(".mosaic-workspace > .topbar").count(),
    0,
    "the live lineage has no redundant operator control bar",
  );
  assert.equal(pixels.bodyWidth, 1000, "the reference viewport has no horizontal overflow");
  assert.equal(pixels.background, "rgb(16, 20, 17)", "the captured surface uses the dark palette");
  await captureReferenceControls(
    page,
    organization,
    join(artifacts, "canvas-reference-four-agent-dark.png"),
  );
}
