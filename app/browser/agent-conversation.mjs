import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { basename, join } from "node:path";

import {
  assertModeSwitchPreservesDeferredMosaic,
  assertMosaicAnchorPreserved,
  assertMosaicLattice,
  assertMosaicLifecycleAdvanced,
  assertMosaicScrollInputs,
  assertPersistentMosaic,
  assertRetainedMosaicIdentity,
  mosaicScrollSnapshot,
  scrollMosaicAwayFromTop,
  waitForMosaicReady,
} from "./agent-conversation-mosaic.mjs";
import { assertReferenceControlStack } from "./control-stack.mjs";
import { assertActivityIdentityLabels } from "./activity-identity.mjs";
import { assertReferenceMessageCard } from "./message-card-reference.mjs";
import { assertSyntheticComposerObservation } from "./composer-shards.mjs";
import {
  chooseCanvasView,
  reply,
  replyAsLane,
  CONNECT_MS,
  TRANSITION_MS,
  waitForLocator as waitForElement,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";
import { assertAgentWorkspaceGeometry } from "./canvas-readiness.mjs";
import { assertCandidateMeasurementPaintAtomic } from "./mosaic-candidate-paint.mjs";
import { assertLaneHeaderPipMatrix } from "./lane-header-pips.mjs";
import { captureMessageStreamBoundaries } from "./message-stream-boundaries.mjs";
import {
  assertGlobalNavigation,
  assertOrgNavigation,
} from "./organization-navigation.mjs";
import {
  assertQuoteBlockDensity,
  captureQuoteBlockDensity,
} from "./quote-block-density.mjs";

const AGENT_SEAT_MS = 15_000;

export { assertMosaicLattice };
export const longAgentName = "Bexley Integration Reliability Operator";
export const longWorktreeBasename = "lane-4-release-observability-and-follow-through";
export const agentNames = ["Ava", longAgentName, "Marco"];
const organizationSeatNames = [...agentNames, undefined];

function canonicalAgentAttribution(member) {
  assert.ok(member.occupant, `lane ${member.lane} has an occupant identity`);
  const identity = `${member.occupant.slice(0, 8)}/${member.lane.slice(0, 8)}`
    + ` · ${basename(member.worktree)}`;
  return member.name ? `${identity} · ${member.name}` : identity;
}

function messageFooterLabel(member) {
  return member.name ?? basename(member.worktree);
}

/// Fixed lane identities keep the browser fixture deterministic. Accent slots
/// come from canonical member order, independently of these UUID values.
const organizationLaneIds = [
  "00000000-0000-4000-8000-000000000000", "00000000-0000-4000-8000-000000000001",
  "00000000-0000-4000-8000-000000000002", "00000000-0000-4000-8000-000000000006",
];
export const nestedQuotePreview = "The author words belong in the composer band.";
export const nestedQuoteMessage = [
  "> > The final review found no alternate execution path.", "> >",
  "> > Installed Spice `dev xtask -- verify --package spice` is green.",
  ">",
  "> Why it matters: nested context remains visible.",
  "",
  nestedQuotePreview,
].join("\n");
const quoteDensityPreview = "The authored conclusion remains outside the quote stack.";
const quoteDensityMessage = [
  "> > > Deep context advances by one compact rail.",
  "> >",
  "> > The first nested paragraph stays close to its successor.",
  "> >",
  "> > The second keeps **inline emphasis** and `code` inside the quote.",
  ">",
  "> - Nested lists retain compact spacing.",
  "> - A deliberately long list item demonstrates that useful line width remains available instead of disappearing into progressively wider indentation.",
  "",
  quoteDensityPreview,
].join("\n");
export const entirelyQuotedMessage = [
  "> > Borrowed context has no authored preview.",
  "> > A second borrowed line stays reachable.",
].join("\n");
const tallMosaicMessage = [
  "## Measured lattice witness",
  "",
  "This directive is intentionally tall at the base span.",
  "",
  "- measurement comes from the rendered DOM",
  "- placement comes from the Rust lattice",
  "- the wide tier remains content-class agnostic",
  "- track bounds stay inside twelve columns",
  "- row spans cover the natural card height",
  "- creation order stays deterministic",
  "- independent lineage controls remain mounted",
  "- attachment presence cannot choose a span",
  "- agent identity cannot choose a span",
  "- CSS class cannot choose a span",
  "- one base-height read is mandatory",
  "- only a tall base measurement admits one more read",
  "- no third candidate is measured",
  "- no card overlaps another card",
  "- exact viewport geometry stays observable",
  "- the browser test reads the instrumentation",
  "- the short path remains one read",
  "- the tall path remains two reads",
].join("\n");

async function assignOrgLaneIds(page) {
  await page.evaluate((laneIds) => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    const send = socket.send.bind(socket);
    let assignment = 0;
    const rewrite = (bytes) => {
      const command = JSON.parse(new TextDecoder().decode(bytes));
      if (command.kind !== "lane::seat") return bytes;
      const lane = laneIds[assignment];
      if (!lane) throw new Error("fixture received an unexpected fifth lane::seat command");
      assignment += 1;
      command.body.lane = lane;
      command.body.lineage_id = laneIds[0];
      return new TextEncoder().encode(JSON.stringify(command));
    };
    socket.send = (data) => {
      if (typeof data === "string") {
        const rewritten = rewrite(new TextEncoder().encode(data));
        send(new TextDecoder().decode(rewritten));
      } else if (data instanceof Blob) {
        void data.arrayBuffer().then((buffer) => send(rewrite(new Uint8Array(buffer))));
      } else {
        send(rewrite(new Uint8Array(data.buffer ?? data, data.byteOffset ?? 0, data.byteLength)));
      }
    };
  }, organizationLaneIds);
}

async function measurePips(locator, laneKey, statusKey) {
  return locator.evaluateAll((nodes, keys) => nodes.map((pip) => {
    const box = pip.getBoundingClientRect();
    return {
      lane: pip.dataset[keys.laneKey],
      status: pip.dataset[keys.statusKey],
      column: pip.dataset.pipColumn,
      row: pip.dataset.pipRow,
      left: box.left,
      top: box.top,
    };
  }), { laneKey, statusKey });
}

async function assertThreeAgentOrgPips(page, console) {
  const lanes = organizationLaneIds.slice(0, 3);
  const menu = page.locator(".canvas-menu");
  await menu.locator(":scope > summary").click();
  const menuOrg = menu.locator(".canvas-organization");
  await menuOrg.locator(".canvas-organization-member").nth(2).waitFor({ timeout: TRANSITION_MS });
  assert.deepEqual(
    await menuOrg.locator(":scope > .canvas-organization-members > .canvas-organization-member")
      .evaluateAll((members) => members.map((member) => [
        member.dataset.organizationMember,
        member.dataset.accent,
      ])),
    lanes.map((lane, index) => [lane, String(lanes.length - index - 1)]),
    "the visible control order establishes the green right-edge agent",
  );

  const organizationPips = await measurePips(
    console.locator(".organization-light"),
    "laneLightTargetId",
    "laneStatus",
  );
  const organizationByLane = new Map(organizationPips.map((pip) => [pip.lane, pip]));
  assert.ok(organizationPips.every((pip) => pip.status), "every three-agent organization pip has a live status");
  assert.deepEqual(
    lanes.map((lane) => {
      const pip = organizationByLane.get(lane);
      return [pip?.column, pip?.row];
    }),
    [["1", "2"], ["2", "1"], ["2", "2"]],
    "three organization pip identities keep the established stack fill order",
  );

  await chooseCanvasView(page, "Lanes");
  await console.getByRole("tab", { name: /^Team/ }).click();
}

/// Seat the canonical four-member fixture organization without running its scenarios.
///
/// Other independently runnable browser shards need a real lineage but must
/// not replay the conversation assertions or their deliberate quiet window.
export async function seatAgentOrg(
  page,
  fixture,
  worktrees,
  assertInvalidVendor = false,
  beforeReload = null,
  artifacts = null,
) {
  const organizationWorktrees = worktrees.slice(0, 4);
  const console = page.locator('[data-pane="agent-console"]');
  await chooseCanvasView(page, "Lanes");
  await console.waitFor();
  assert.equal(
    await console.locator(".organization-pane").getAttribute("data-organization-mode"),
    "Lanes",
    "the empty organization begins in the right-edge Lanes mode",
  );
  await page.getByLabel("Open Spice Canvas menu").click();
  await assertGlobalNavigation(page, "the empty fleet");
  await chooseCanvasView(page, "Lanes");
  await assertOrgNavigation(console, "the empty fleet");
  await console.getByRole("tab", { name: /^Team/ }).click();
  await console.getByLabel("Name").fill(agentNames[0]);
  await console.getByLabel("Worktree").fill(organizationWorktrees[0]);
  await console.getByLabel("Life").fill("steer");

  const refusal = console.locator(".form-refusal");
  if (assertInvalidVendor) {
    // A refused submission has to say why. The vendor field is free text with
    // no datalist, so a proper name is the trip an operator actually takes.
    await console.getByLabel("Vendor", { exact: true }).fill("My Agent");
    await console.getByRole("button", { name: "Seat agent" }).click();
    await refusal.waitFor({ timeout: 10_000 });
    assert.match(await refusal.innerText(), /Vendor must be one lower-case word/);
  }

  await assignOrgLaneIds(page);
  await console.getByLabel("Vendor", { exact: true }).fill("Claude");
  await console.getByRole("button", { name: "Seat agent" }).click();
  await refusal.waitFor({ state: "detached", timeout: 10_000 });
  await waitForElement(page, "first external agent seat renders", AGENT_SEAT_MS,
    console.locator(".organization-detail-card").nth(0));

  const firstSnapshot = await reply(fixture, ["lane"]);
  const host = firstSnapshot.body.lanes.find((lane) => lane.worktree === organizationWorktrees[0]);
  assert.ok(host, "the Canvas seat control creates the lineage host");
  assert.equal(host.lane, organizationLaneIds[0], "the fixture fixes the lineage host accent slot");
  const hostSurface = console.locator(
    `.organization-surface[data-lineage-id="${host.lineage_id}"]`,
  );
  assert.equal(
    await hostSurface.locator(".organization-pane").getAttribute("data-organization-mode"),
    "Org",
    "the first seated lineage inherits the empty Canvas Org selection",
  );
  assert.equal(
    await hostSurface.locator(".organization-detail-card").count(),
    1,
    "the inherited Org surface keeps the first seat details visible",
  );
  for (const [index, worktree] of organizationWorktrees.slice(1).entries()) {
    await console.getByLabel("Name").fill(organizationSeatNames[index + 1] ?? "");
    await console.getByLabel("Worktree").fill(worktree);
    await console.getByLabel("Life").fill("steer");
    await console.getByLabel("Vendor", { exact: true }).fill("Claude");
    await console.getByRole("button", { name: "Seat agent" }).click();
    await waitForElement(page, "next external agent seat renders", AGENT_SEAT_MS,
      console.locator(".organization-detail-card").nth(index + 1));
    if (index === 1) await assertThreeAgentOrgPips(page, console);
  }
  for (const lane of organizationLaneIds) {
    const wake = await reply(fixture, ["lane", "wake", lane]);
    assert.equal(wake.body.lane, lane, "the fixture wakes each canonical organization member");
    assert.equal(wake.body.to, "occupied", "the fixture starts an agent before prose assertions");
  }
  if (beforeReload) await beforeReload(console);
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Lanes");
  await console.getByRole("tab", { name: /^Team/ }).click();
  await waitForElement(page, "seated agents render after reload", CONNECT_MS,
    console.locator(".organization-detail-card").nth(3));
  await waitForPage(page, "seated agents publish their session identities", CONNECT_MS, () => {
    const cards = [...document.querySelectorAll(".organization-detail-card")];
    return cards.length >= 4 && cards.every((card) => !card.title.includes("thread: unbound"));
  }, undefined, {
    observed: () => console.locator(".organization-detail-card").evaluateAll((cards) => (
      cards.map((card) => card.title)
    )),
  });

  const lanes = await reply(fixture, ["lane"]);
  const seated = organizationWorktrees.map(
    (worktree) => lanes.body.lanes.find((lane) => lane.worktree === worktree),
  );
  assert.ok(seated.every(Boolean), "one lineage admits all four linked worktrees");
  assert.deepEqual(seated.map((lane) => lane.lane), organizationLaneIds, "four fixed lane identities render");
  assert.ok(seated.every((lane) => lane.lineage_id === host.lineage_id), "one lineage renders");
  assert.deepEqual(seated.map((lane) => lane.name), organizationSeatNames);
  assert.equal(seated[1].name, longAgentName, "the fixture admits a long configured agent name");
  assert.equal(
    messageFooterLabel(seated[3]),
    longWorktreeBasename,
    "the unnamed fixture lane exposes a long worktree basename",
  );
  assert.ok(seated.every((lane) => lane.vendor === "claude"));
  assert.ok(seated.every((lane) => lane.life === "steer"), "the organization shares one lifetime");
  assert.equal(
    await console.locator(".organization-detail-card").first().getAttribute("title"),
    `Claude vendor; model: claude-opus-5; effort: high; thread: ${seated[0].occupant}; `
      + "session: 018f7777-7777-7444-8888-999999999999",
    "the agent card tooltip carries the admitted launch and session identity",
  );

  return {
    console,
    host,
    seated,
    organizationWorktrees,
    lane: seated[0].lane,
    quoteLane: seated[3].lane,
    agentName: agentNames[0],
    agentAttribution: canonicalAgentAttribution(seated[0]),
    quoteProse: agentProse(organizationWorktrees[3]),
    primaryHeadings: seated.map((lane, index) => ({
      lane: lane.lane,
      prose: index === 0
        ? "newest lane prose witness"
        : agentProse(organizationWorktrees[index]),
    })),
  };
}

async function assertOperatorProseHidden(console, text, label) {
  assert.equal(
    await console.locator('[data-card-kind="operator-message"]').count(),
    0,
    `${label}: Canvas renders no standalone operator cards`,
  );
  assert.equal(
    await console.locator('.mosaic-message-card').filter({ hasText: text }).count(),
    0,
    `${label}: unacknowledged operator prose is absent from the conversation`,
  );
}

async function assertStableOperatorAuthorship(page, fixture, console, lane, activityReplay) {
  const text = `stable operator authorship ${crypto.randomUUID()}`;
  const composer = console.locator(`.organization-agent[data-agent-lane="${lane}"]`);
  await composer.getByRole("textbox").fill(text);
  await console.getByRole("button", { name: "Steer", exact: true }).click();
  const inbox = await reply(fixture, ["inbox", lane]);
  const directive = inbox.body.lanes[0].pending.find((row) => row.text === text);
  assert.ok(directive, "the focused operator record has durable steering identity");
  await assertOperatorProseHidden(console, text, "live steering");
  assert.notEqual(
    (await composer.locator(".composer-shard-prose").innerText()).trim(),
    text,
    "operator prose never occupies the agent header before acknowledgement",
  );

  const beforeAgents = await console.locator('[data-card-kind="agent-message"]').count();
  const agentText = `agent acknowledgement ${crypto.randomUUID()}`;
  await activityReplay.injectAgentProse(page, lane, agentText);
  assert.equal(
    await console.locator('[data-card-kind="agent-message"]').count(),
    beforeAgents + 1,
    "one deterministic agent record follows the operator record",
  );
  const response = console.locator('[data-card-kind="agent-message"]')
    .filter({ hasText: agentText }).last();
  await response.waitFor({ timeout: TRANSITION_MS });
  assert.equal(await response.getAttribute("data-message-role"), "agent");
  await reply(fixture, ["steer", "cancel", directive.directive]);
  await assertOperatorProseHidden(console, text, "canceled steering");
}

async function assertExternalOperatorReplay(page, fixture, console, lane, worktree) {
  const quotedText = `external operator premise ${crypto.randomUUID()}`;
  const replyText = `approved ${crypto.randomUUID()}`;
  const text = `> ${quotedText}\n\n${replyText}`;
  await page.evaluate(() => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    window.__externalOperatorFacts = [];
    socket.addEventListener("message", async (event) => {
      const text = typeof event.data === "string"
        ? event.data
        : event.data instanceof Blob
          ? await event.data.text()
          : new TextDecoder().decode(event.data);
      const frame = JSON.parse(text);
      if (frame.stream === "fact" && frame.fact?.fact?.kind === "steer::sent") {
        window.__externalOperatorFacts.push(frame.fact);
      }
    });
  });
  const sent = await reply(fixture, ["steer", lane, text]);
  await waitForPage(page, "external steer fact reaches Canvas", TRANSITION_MS, (directive) => (
    window.__externalOperatorFacts.some((fact) => fact.fact.directive === directive)
  ), sent.body.directive, {
    observed: () => page.evaluate(() => window.__externalOperatorFacts),
  });
  const streamed = await page.evaluate((directive) => (
    window.__externalOperatorFacts.find((fact) => fact.fact.directive === directive)
  ), sent.body.directive);
  assert.equal(streamed.actor, "operator", `external steer retains operator authorship: ${JSON.stringify(streamed)}`);
  assert.equal(streamed.fact.lane, lane, "external steer fact retains its target lane");
  await assertOperatorProseHidden(console, replyText, "external live steering");

  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Lanes");
  await console.getByRole("tab", { name: /^Agents/ }).click();
  await assertOperatorProseHidden(console, replyText, "pre-acknowledgement replay");

  await writeFile(join(worktree, ".spice-ack-trigger"), `${sent.body.key}\n`);
  const acknowledgedCard = console.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
  ).filter({ hasText: replyText });
  await waitForPage(page, "acknowledgement evidence reaches its assistant card", CONNECT_MS,
    ({ lane, replyText }) => [...document.querySelectorAll(
      `[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
    )].some((card) => (
      card.textContent?.includes(replyText)
        && card.dataset.messageRole === "agent"
        && card.dataset.messageAckCount === "1"
        && card.dataset.messageTasteCount === "0"
        && [...card.querySelectorAll(".message-badge")]
          .map((badge) => badge.textContent).join(",") === "ACK1"
    )), { lane, replyText });
  assert.equal(await acknowledgedCard.count(), 1, "acknowledgement renders one assistant card");
  assert.equal(await acknowledgedCard.getAttribute("data-message-role"), "agent");
  assert.equal(await acknowledgedCard.getAttribute("data-message-ack-count"), "1");
  assert.equal(await acknowledgedCard.getAttribute("data-message-taste-count"), "0");
  assert.deepEqual(
    await acknowledgedCard.locator(".message-badge").allTextContents(),
    ["ACK1"],
    "authenticated acknowledgement evidence renders one counted ACK badge",
  );
  const outerQuote = acknowledgedCard.locator("blockquote").first();
  assert.equal(
    (await outerQuote.locator("blockquote").innerText()).trim(),
    quotedText,
    "the acknowledged operator quote remains nested",
  );
  assert.match(
    (await outerQuote.innerText()).trim(),
    new RegExp(`${replyText.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}$`),
    "the operator reply follows its nested quote",
  );
  assert.doesNotMatch(
    (await acknowledgedCard.locator(".message-body").innerText()).trim(),
    new RegExp(`(?:ACK|NACK)\\s+${sent.body.key}`),
    "recognized protocol framing is absent from visible prose",
  );
  assert.match(
    (await acknowledgedCard.locator(".message-body").innerText()).trim(),
    /\nApplied$/,
    "the assistant response follows its acknowledgement quote",
  );
  assert.equal(await console.locator('[data-card-kind="operator-message"]').count(), 0);
  await waitForPage(page, "external acknowledgement Mosaic settles", TRANSITION_MS, () => {
    const element = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    return element?.dataset.measurementPhase === "ready"
      && element.dataset.measurementPending === "0";
  });
  const acknowledged = await replyAsLane(worktree, lane, ["ack", sent.body.key]);
  await waitForPage(page, "external acknowledgement reaches Canvas", TRANSITION_MS, (cursor) => (
    document.querySelector(".bar-meta")?.textContent === cursor
  ), `cursor ${acknowledged.seq.last}`);
  assert.equal(await acknowledgedCard.count(), 1, "acknowledgement refresh retains one assistant card");
  assert.equal(await console.locator('[data-card-kind="operator-message"]').count(), 0);
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Lanes");
  await console.getByRole("tab", { name: /^Agents/ }).click();
  await acknowledgedCard.waitFor({ timeout: CONNECT_MS });
  assert.equal(await acknowledgedCard.count(), 1, "bootstrap restores one acknowledged assistant card");
  assert.deepEqual(
    await acknowledgedCard.locator(".message-badge").allTextContents(),
    ["ACK1"],
    "durable replay restores the typed ACK badge",
  );
  assert.equal(
    (await acknowledgedCard.locator("blockquote blockquote").innerText()).trim(),
    quotedText,
    "durable replay restores the nested operator quote",
  );
  assert.doesNotMatch(
    (await acknowledgedCard.locator(".message-body").innerText()).trim(),
    new RegExp(`(?:ACK|NACK)\\s+${sent.body.key}`),
    "durable replay keeps protocol framing hidden",
  );
  assert.equal(await console.locator('[data-card-kind="operator-message"]').count(), 0);
  return replyText;
}

async function assertInlineTastePills(page, console, lane, activityReplay) {
  const replyText = "ACK: presentation remains unchanged.";
  const text = [
    "> [TASTE] compatibility: Keep one tested path.",
    "",
    "> [TASTE] polling: Wait for the completion event.",
    "",
    replyText,
  ].join("\n");
  const { key } = await activityReplay.injectClassifiedAgentProse(page, lane, text, {
    acknowledgements: 0,
    tastes: 2,
  });
  const card = console.locator(`[data-mosaic-card-key="${key}"]`);
  await card.waitFor({ timeout: TRANSITION_MS });
  assert.equal(await card.getAttribute("data-message-ack-count"), "0");
  assert.equal(await card.getAttribute("data-message-taste-count"), "2");
  assert.deepEqual(
    await card.locator(".message-taste-pill").allTextContents(),
    ["TASTE", "TASTE"],
    "each quoted taste receives its own unnumbered inline pill",
  );
  assert.equal(
    await card.locator(".message-taste-pill .message-badge-count").count(),
    0,
    "inline taste pills carry no ordinal or count",
  );
  assert.equal(
    await card.locator(".message-footer .message-badge").count(),
    0,
    "taste classification is not repeated in the message footer",
  );
  const body = await card.locator(".message-body").innerText();
  assert.doesNotMatch(body, /\[TASTE\]/, "quoted taste markers are replaced in presentation");
  assert.doesNotMatch(body, /compatibility:|polling:/);
  assert.match(body, /Keep one tested path\./);
  assert.match(body, /Wait for the completion event\./);
  assert.match(body, /ACK: presentation remains unchanged\.$/);
  return replyText;
}

async function assertInlineBroadcastPill(page, console, lane, activityReplay) {
  const replyText = "ACK: every occupied agent received the same message.";
  const text = [
    "> [BROADCAST] Send this to every occupied agent.",
    "",
    replyText,
  ].join("\n");
  const { key } = await activityReplay.injectClassifiedAgentProse(page, lane, text, {
    acknowledgements: 0,
    tastes: 0,
    broadcasts: 1,
  });
  const card = console.locator(`[data-mosaic-card-key="${key}"]`);
  await card.waitFor({ timeout: TRANSITION_MS });
  assert.equal(await card.getAttribute("data-message-broadcast-count"), "1");
  const pill = card.locator(".message-broadcast-pill");
  assert.equal(await pill.innerText(), "BROADCAST");
  assert.equal(await pill.getAttribute("data-message-classification"), "broadcast");
  assert.equal(await pill.getAttribute("aria-label"), "Broadcast classification");
  assert.doesNotMatch(await card.locator(".message-body").innerText(), /\[BROADCAST\]/u);
  for (const colorScheme of ["light", "dark"]) {
    await emulateColorScheme(page, colorScheme);
    const colors = await card.evaluate((element) => {
      const quote = element.querySelector("blockquote");
      const pill = element.querySelector(".message-broadcast-pill");
      if (!quote || !pill) throw new Error("broadcast presentation is incomplete");
      const probe = document.createElement("span");
      probe.style.color = "var(--accent-5)";
      element.append(probe);
      const accent = getComputedStyle(probe).color;
      probe.remove();
      return {
        accent,
        quote: getComputedStyle(quote).borderLeftColor,
        pill: getComputedStyle(pill).color,
        fontFamily: getComputedStyle(pill).fontFamily,
      };
    });
    assert.deepEqual(
      [colors.quote, colors.pill],
      [colors.accent, colors.accent],
      `${colorScheme} broadcast quote and pill use the plum accent`,
    );
    assert.equal(
      colors.fontFamily,
      "ui-monospace, SFMono-Regular, Menlo, monospace",
      `${colorScheme} broadcast pill uses the shared semantic monospace stack`,
    );
  }
  await emulateColorScheme(page, "light");
  return replyText;
}

async function emulateColorScheme(page, colorScheme) {
  await page.emulateMedia({ colorScheme });
  await waitForPage(
    page,
    `${colorScheme} system theme applies before palette inspection`,
    TRANSITION_MS,
    (expected) => document.documentElement.dataset.canvasEffectiveTheme === expected,
    colorScheme,
  );
}

async function assertImmediateFinalCard(page, console, lane, activityReplay) {
  let latestText;
  for (const reducedMotion of ["no-preference", "reduce"]) {
    await page.emulateMedia({ reducedMotion });
    const text = `Immediate final card with ${reducedMotion} motion.`;
    latestText = text;
    await page.evaluate(({ lane, text }) => {
      const witness = { states: [] };
      window.__canvasFirstFinalFrame = witness;
      const observer = new MutationObserver(() => {
        const card = [...document.querySelectorAll(
          `[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
        )].find((candidate) => candidate.textContent?.includes(text));
        if (!card) return;
        const style = getComputedStyle(card);
        const box = card.getBoundingClientRect();
        witness.states.push({
          messageFinal: card.dataset.messageFinal,
          finalClass: card.classList.contains("final"),
          layoutPhase: card.dataset.layoutPhase,
          visibility: style.visibility,
          borderTopColor: style.borderTopColor,
          backgroundColor: style.backgroundColor,
          box: [box.x, box.y, box.width, box.height],
        });
        observer.disconnect();
      });
      observer.observe(document.body, { childList: true, subtree: true });
    }, { lane, text });
    const injected = await activityReplay.injectAgentFinal(page, lane, text);
    await waitForPage(
      page,
      "the first terminal card frame is observed",
      TRANSITION_MS,
      () => window.__canvasFirstFinalFrame.states.length > 0,
    );
    const [first] = await page.evaluate(() => window.__canvasFirstFinalFrame.states);
    assert.equal(first.messageFinal, "true");
    assert.equal(first.finalClass, true);
    const colors = await console.evaluate((element) => {
      const probe = document.createElement("span");
      probe.style.color = "var(--accent-1)";
      probe.style.background = "color-mix(in srgb, var(--accent-1) 8%, var(--panel))";
      element.append(probe);
      const style = getComputedStyle(probe);
      const result = { accent: style.color, tint: style.backgroundColor };
      probe.remove();
      return result;
    });
    assert.equal(first.borderTopColor, colors.accent);
    assert.equal(first.backgroundColor, colors.tint);
    const card = console.locator(`[data-mosaic-card-key="${injected.key}"]`);
    assert.equal(await card.count(), 1, "the terminal record creates one card identity");
    assert.equal(await card.getAttribute("data-message-final"), "true");
    await waitForPage(
      page,
      "the immediate final card settles into its mosaic placement",
      TRANSITION_MS,
      (key) => document.querySelector(`[data-mosaic-card-key="${key}"]`)
        ?.dataset.layoutPhase === "committed",
      injected.key,
    );
    const settledBox = await card.evaluate((element) => {
      const box = element.getBoundingClientRect();
      return [box.x, box.y, box.width, box.height];
    });
    if (first.visibility === "hidden") {
      assert.equal(first.layoutPhase, "measuring");
    } else {
      assert.deepEqual(settledBox, first.box, "a visible final card does not shift after insertion");
    }
  }
  await page.emulateMedia({ reducedMotion: "no-preference" });
  return latestText;
}

export async function assertAgentConversation(
  page,
  fixture,
  worktrees,
  activityReplay,
  artifacts,
) {
  const organization = await seatAgentOrg(
    page,
    fixture,
    worktrees,
    true,
    (console) => activityReplay.assertBootstrapLiveMessages(console),
    artifacts,
  );
  const {
    console,
    host,
    seated,
    organizationWorktrees,
  } = organization;

  await console.getByRole("tab", { name: /^Agents/ }).click();
  await console.locator(".organization-agent").nth(3).waitFor();
  assert.equal(await console.locator(".organization-surface").count(), 1, "one lineage renders one surface");
  assert.equal(
    await console.locator(".organization-activity-grid").count(),
    1,
    "all lineage members share exactly one packed activity plane",
  );
  const authored = console.locator('[data-card-kind="agent-message"]');
  await authored.nth(3).waitFor({ timeout: 10_000 });
  const producerIds = new Set(await authored.evaluateAll((cards) =>
    cards.map((card) => card.getAttribute("data-producer-target-id")),
  ));
  const producerNames = new Set(await authored.locator(".message-agent-name").allTextContents());
  assert.ok(producerIds.size > 1, "the packed plane contains more than one producing agent");
  assert.ok(producerNames.size > 1, "each authored card footer names its producing agent");
  await assertSyntheticComposerObservation(
    page,
    console,
    activityReplay,
    seated[0].lane,
    seated[1].lane,
  );
  await assertStableOperatorAuthorship(
    page,
    fixture,
    console,
    seated[0].lane,
    activityReplay,
  );
  const acknowledgementReply = await assertExternalOperatorReplay(
    page,
    fixture,
    console,
    seated[0].lane,
    organizationWorktrees[0],
  );
  const currentProse = seated.map((member) => activityReplay.nextAgentProse(member.lane));
  await Promise.all([
    writeFile(join(organizationWorktrees[0], ".spice-tool-trigger"), "emit\n"),
    ...organizationWorktrees.slice(0, seated.length).map((worktree) => (
      writeFile(join(worktree, ".spice-prose-trigger"), "emit\n")
    )),
  ]);
  await Promise.all(currentProse);
  await assertActivityIdentityLabels(
    page,
    console,
    activityReplay,
    seated[0],
    seated[3],
  );
  await assertRosterPips(page, console, seated, artifacts);
  const accessibility = await console.locator(".organization-activity-grid").ariaSnapshot();
  for (const name of producerNames) {
    assert.match(accessibility, new RegExp(name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  }
  assert.equal(await console.getByRole("button", { name: "Steer", exact: true }).count(), 1);
  const lifetime = console.locator('.organization-lifetime input[type="range"]');
  await lifetime.fill("2");
  await console.getByRole("button", { name: "Drain", exact: true }).waitFor();
  await lifetime.fill("0");
  await console.getByRole("button", { name: "Steer", exact: true }).waitFor();
  await assertReferenceControlStack(page, console, artifacts);
  const messages = [
    "browser direction survives streaming",
    "## Gate contract\n\n- green on the integrated tip\n- exact checks retained",
    "> The reference viewport keeps **all agent status** visible.\n>\n> Density remains readable.\n\n`1000×568` stays bounded.",
    nestedQuoteMessage,
  ];
  const messageLabels = [
    messages[0],
    "Gate contract",
    "reference viewport keeps",
    "final review found",
  ];
  for (const [index, lane] of seated.entries()) {
    const column = console.locator(`.organization-agent[data-agent-lane="${lane.lane}"]`);
    await column.getByRole("textbox").fill(messages[index]);
    await console.getByRole("button", { name: "Steer", exact: true }).click();
    if (index === 0) await assertQueuedRosterPip(page, console, seated[0]);
  }
  const quoteLane = console.locator(`.organization-agent[data-agent-lane="${seated[3].lane}"]`);
  await quoteLane.getByRole("textbox").fill(entirelyQuotedMessage);
  await console.getByRole("button", { name: "Steer", exact: true }).click();
  const tallLane = console.locator(`.organization-agent[data-agent-lane="${seated[1].lane}"]`);
  await tallLane.getByRole("textbox").fill(tallMosaicMessage);
  await console.getByRole("button", { name: "Steer", exact: true }).click();
  const refreshedProse = seated.map((member) => activityReplay.nextAgentProse(member.lane));
  await Promise.all(organizationWorktrees.slice(0, seated.length).map((worktree) => (
    writeFile(join(worktree, ".spice-prose-trigger"), "emit\n")
  )));
  await Promise.all(refreshedProse);
  for (const [index, lane] of seated.entries()) {
    await activityReplay.injectAgentProse(page, lane.lane, messages[index]);
    await console
      .locator(`.mosaic-message-card[data-agent-lane="${lane.lane}"]`)
      .filter({ hasText: messageLabels[index] })
      .waitFor({ timeout: 10_000 });
  }
  await activityReplay.injectAgentProse(page, seated[3].lane, entirelyQuotedMessage);
  await console
    .locator(`.mosaic-message-card[data-agent-lane="${seated[3].lane}"]`)
    .filter({ hasText: "Borrowed context has no authored preview." })
    .waitFor({ timeout: 10_000 });
  await activityReplay.injectAgentProse(page, seated[1].lane, tallMosaicMessage);
  await console
    .locator(`.mosaic-message-card[data-agent-lane="${seated[1].lane}"]`)
    .filter({ hasText: "Measured lattice witness" })
    .waitFor({ timeout: 10_000 });
  assert.equal(
    await console.locator('[data-card-kind="operator-message"]').count(),
    0,
    "pending directions never join assistant-authored Mosaic history",
  );
  assert.ok(await console.locator(".mosaic-message-card h2").count(), "cards render headings");
  assert.ok(await console.locator(".mosaic-message-card ul li").count(), "cards render lists");
  assert.ok(
    await console.locator(".mosaic-message-card blockquote blockquote").count(),
    "cards preserve nested quote depth",
  );
  const cardHeights = await console.locator(".mosaic-message-card").evaluateAll((cards) =>
    cards.map((card) => Math.round(card.getBoundingClientRect().height)),
  );
  assert.ok(new Set(cardHeights).size >= 3, "Markdown produces nonuniform Mosaic card heights");
  const planes = console.locator(".lane-mosaic-plane");
  assert.equal(await planes.count(), 1, "one seated lineage owns one Mosaic plane");
  const plane = console.locator(
    `.lane-mosaic-plane[data-agent-plane="${host.lineage_id}"]`,
  );
  await plane.waitFor();
  const placements = await assertMosaicLattice(page, true);
  assert.ok(placements.length > seated.length, "the lane packs assistant-authored cards");
  assert.ok(placements.every((card) => card.span >= 1 && card.track + card.span <= 12));
  assert.ok(placements.every((card) => card.rows >= 3));
  assert.ok(placements.every((card) => card.columnStart === card.track + 1));
  assert.ok(placements.every((card) => card.row === card.base));
  assert.equal(Math.min(...placements.map((card) => card.base)), 0, "the lane owns its row floor");
  const packedMembers = new Set(placements.map((card) => card.lane));
  for (const member of seated) {
    assert.ok(packedMembers.has(member.lane), "every worktree member reaches the lane plane");
  }
  await assertPersistentMosaic(
    page,
    fixture,
    console,
    plane,
    seated[0],
    activityReplay,
    artifacts,
  );
  await assertCandidateMeasurementPaintAtomic(page, "chromium");

  // Every message projection uses the producer's current canonical member
  // position. Identity remains explicit in the accessible attribution rather
  // than being inferred from that decorative slot.
  const messageChrome = await console.locator('[data-card-kind="agent-message"]').evaluateAll(
    (cards) => cards.map((card) => ({
      lane: card.dataset.agentLane,
      accent: card.dataset.accent,
      accentSlot: card.dataset.accentSlot,
      producer: card.querySelector(".message-agent-name")?.textContent?.trim() ?? "",
      producerAria: card.querySelector(".message-agent-name")?.getAttribute("aria-label") ?? "",
    })),
  );
  for (const [position, member] of seated.entries()) {
    const expected = messageFooterLabel(member);
    const attribution = canonicalAgentAttribution(member);
    const accent = (seated.length - position - 1) % 6;
    assert.ok(
      messageChrome.some((card) => card.lane === member.lane && card.producer === expected),
      `lane ${member.lane} renders its configured name or worktree basename`,
    );
    assert.ok(
      messageChrome.some((card) => (
        card.lane === member.lane
          && card.producerAria === `Copy agent id for ${attribution}`
      )),
      `lane ${member.lane} preserves canonical attribution for its copy control`,
    );
    assert.ok(
      messageChrome.filter((card) => card.lane === member.lane)
        .every((card) => (
          card.accent === String(accent) && card.accentSlot === String(accent)
        )),
      `lane ${member.lane} message cards use right-edge accent slot ${accent}`,
    );
  }
  const separated = [seated[0].lane, seated[3].lane].map(
    (lane) => messageChrome.find((card) => card.lane === lane),
  );
  assert.ok(separated.every(Boolean), "the first and fourth lanes render message cards");
  assert.notEqual(separated[0].accent, separated[1].accent);
  assert.notEqual(separated[0].lane, separated[1].lane);
  assert.notEqual(
    separated[0].producer, separated[1].producer,
    "producer names remain distinct independently of color",
  );
  const expectedActions = [
    { label: "Speak this message", icon: "", iconKind: "play" },
    { label: "Quote this message", icon: "❝", iconKind: null },
    { label: "Copy this message", icon: "⧉", iconKind: null },
  ];
  const actionCard = console.locator('[data-card-kind="agent-message"]').first();
  assert.ok((await actionCard.locator(".message-agent-name").innerText()).trim());
  assert.deepEqual(
    await actionCard.locator(".message-actions button").evaluateAll(
      (buttons) => buttons.map((button) => ({
        label: button.getAttribute("aria-label"),
        icon: button.querySelector(".card-action-icon")?.textContent,
        iconKind: button.querySelector(".card-action-icon")?.dataset.cardActionIcon ?? null,
        iconHidden: button.querySelector(".card-action-icon")?.getAttribute("aria-hidden"),
        title: button.title,
      })),
    ),
    expectedActions.map(({ label, icon, iconKind }) => ({
      label,
      icon,
      iconKind,
      iconHidden: "true",
      title: label,
    })),
    "assistant cards expose the reference speak, quote, and copy controls",
  );
  await assertReferenceMessageCard(
    page,
    console,
    actionCard,
    artifacts,
    seated.map(messageFooterLabel),
    activityReplay,
    seated[0].lane,
  );

  const scroller = plane;
  const compensationViewport = page.viewportSize();
  await page.setViewportSize({ width: compensationViewport.width, height: 360 });
  await scroller.evaluate((element) => element.scrollTo(0, 0));

  const topText = `mosaic at-top follow probe ${crypto.randomUUID()}`;
  await activityReplay.injectAgentProse(page, seated[0].lane, topText);
  const topCard = console
    .locator(`.mosaic-message-card[data-agent-lane="${seated[0].lane}"]`)
    .filter({ hasText: topText });
  await topCard.waitFor({ timeout: 10_000 });
  await waitForPage(page, "at-top Mosaic activity commits", TRANSITION_MS, (text) => {
    const card = [...document.querySelectorAll(".mosaic-message-card")]
      .find((candidate) => candidate.textContent?.includes(text));
    return card?.dataset.layoutPhase === "committed";
  }, topText);
  const topFollow = await scroller.evaluate((element, key) => {
    const card = [...element.querySelectorAll(".mosaic-message-card")]
      .find((candidate) => candidate.dataset.mosaicCardKey === key);
    const viewport = element.getBoundingClientRect();
    const box = card?.getBoundingClientRect();
    return {
      card: box ? { bottom: box.bottom, top: box.top } : null,
      scrollTop: element.scrollTop,
      viewport: { bottom: viewport.bottom, top: viewport.top },
      visible: Boolean(box && box.bottom > viewport.top && box.top < viewport.bottom),
    };
  }, await topCard.getAttribute("data-mosaic-card-key"));
  assert.equal(topFollow.scrollTop, 0, "at-top activity remains at the plane origin");
  assert.ok(topFollow.visible, `at-top activity follows visibly: ${JSON.stringify(topFollow)}`);
  await topCard.waitFor({ timeout: TRANSITION_MS });

  await assertMosaicScrollInputs(page, scroller);
  const beforeCompensation = await scrollMosaicAwayFromTop(page, scroller);
  const compensationMessage = [
    "mosaic scroll compensation probe",
    ...Array.from({ length: 72 }, (_, index) => `- exact row delta ${index + 1}`),
  ].join("\n");
  await activityReplay.injectAgentProse(page, seated[0].lane, compensationMessage);
  const compensationCard = console
    .locator(`.mosaic-message-card[data-agent-lane="${seated[0].lane}"]`)
    .filter({ hasText: "mosaic scroll compensation probe" });
  await compensationCard.waitFor({ timeout: 10_000 });
  await waitForPage(page, "mosaic rows extend for the streamed card", TRANSITION_MS, (before) => {
    const element = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    const maxRow = Number(element?.dataset.maxRow);
    return maxRow > before.maxRow;
  }, beforeCompensation);
  const afterCompensation = await mosaicScrollSnapshot(scroller);
  assertMosaicAnchorPreserved(
    beforeCompensation,
    afterCompensation,
    "desktop incoming activity while scrolled",
  );
  assertMosaicLifecycleAdvanced(
    beforeCompensation,
    afterCompensation,
    "desktop incoming activity while scrolled",
  );
  assert.ok(afterCompensation.maxRow > beforeCompensation.maxRow);
  assert.equal(
    afterCompensation.scrollTop,
    beforeCompensation.scrollTop
      + (afterCompensation.maxRow - beforeCompensation.maxRow) * afterCompensation.module,
    `incremental placement compensates by exactly the added lattice rows: ${JSON.stringify({
      beforeCompensation,
      afterCompensation,
    })}`,
  );

  await page.setViewportSize({ width: 520, height: 360 });
  await waitForPage(page, "narrow Mosaic commits its responsive width", TRANSITION_MS,
    (desktopWidth) => {
      const plane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
      return plane?.dataset.measurementPhase === "ready"
        && Number(plane.dataset.committedWidth) < desktopWidth;
    }, afterCompensation.committedWidth);
  const narrow = await mosaicScrollSnapshot(scroller);
  assertMosaicAnchorPreserved(
    afterCompensation,
    narrow,
    "narrow responsive reflow while scrolled",
  );
  await page.setViewportSize(compensationViewport);
  await waitForPage(page, "desktop Mosaic width is restored", TRANSITION_MS,
    (narrowWidth) => Number(document.querySelector(
      '[data-pane="agent-console"] .lane-mosaic-plane',
    )?.dataset.committedWidth) > narrowWidth,
    narrow.committedWidth);
  await compensationCard.waitFor({ timeout: TRANSITION_MS });

  await activityReplay.injectAgentProse(page, seated[3].lane, quoteDensityMessage);
  const quoteDensityCard = console
    .locator(`.mosaic-message-card[data-agent-lane="${seated[3].lane}"]`)
    .filter({ hasText: quoteDensityPreview });
  await quoteDensityCard.waitFor({ timeout: 10_000 });
  await waitForPage(page, "quote density card commits", TRANSITION_MS, (preview) => {
    const card = [...document.querySelectorAll(".mosaic-message-card")]
      .find((candidate) => candidate.textContent?.includes(preview));
    return card?.dataset.layoutPhase === "committed";
  }, quoteDensityPreview);
  const quoteCards = {
    ack: console.locator('[data-card-kind="agent-message"]')
      .filter({ hasText: acknowledgementReply }),
    user: quoteDensityCard,
  };
  await assertQuoteBlockDensity(quoteCards, "Chromium wide Mosaic");
  await captureQuoteBlockDensity(page, quoteCards, artifacts, "chromium-quotes-wide");
  const quoteDesktopWidth = Number(await scroller.getAttribute("data-committed-width"));
  await page.setViewportSize({ width: 520, height: 360 });
  await waitForPage(page, "narrow quote density Mosaic commits", TRANSITION_MS,
    (desktopWidth) => {
      const plane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
      return plane?.dataset.measurementPhase === "ready"
        && Number(plane.dataset.committedWidth) < desktopWidth;
    }, quoteDesktopWidth);
  await assertQuoteBlockDensity(quoteCards, "Chromium narrow Mosaic");
  await captureQuoteBlockDensity(page, quoteCards, artifacts, "chromium-quotes-narrow");
  const quoteNarrowWidth = Number(await scroller.getAttribute("data-committed-width"));
  await page.setViewportSize(compensationViewport);
  await waitForPage(page, "desktop quote density Mosaic is restored", TRANSITION_MS,
    (narrowWidth) => Number(document.querySelector(
      '[data-pane="agent-console"] .lane-mosaic-plane',
    )?.dataset.committedWidth) > narrowWidth,
    quoteNarrowWidth);

  await page.locator('.task-pill[data-task-stem="app"]').waitFor();
  await page.locator('.task-pill[data-task-stem="canvas"]').waitFor();
  assert.match(await page.locator('.task-pill[data-task-stem="app"]').innerText(), /\d+·\d+·\d+/);

  const outgoing = console
    .locator(`.mosaic-message-card[data-agent-lane="${seated[0].lane}"]`)
    .filter({ hasText: messages[0] });
  const inbox = await reply(fixture, ["inbox", seated[0].lane]);
  assert.equal(inbox.body.lanes.length, 1);
  assert.equal(inbox.body.lanes[0].pending.length, 1);
  const pendingDirective = inbox.body.lanes[0].pending[0];
  assert.equal(pendingDirective.text, messages[0]);
  assert.equal(await outgoing.getAttribute("data-card-kind"), "agent-message");
  const age = outgoing.locator(".message-age");
  assert.match((await age.innerText()).trim(), /^(?:now|\d+[smhd])$/);
  assert.match(await age.getAttribute("title"), /^tape \d+:\d+\+\d+#\d+$/);
  assert.equal(await age.getAttribute("data-age-clock"), "deadline");
  assert.match(await age.getAttribute("data-age-authored"), /^\d+$/);
  assert.equal(await outgoing.locator(".message-actions button").count(), 3);

  await console.getByRole("tab", { name: /^Board/ }).click();
  await console.locator(".organization-task-workspace").waitFor();
  assert.equal(
    await console.locator(".organization-task-workspace").count(),
    1,
    "Board renders one organization-scoped operational workspace",
  );
  await console.getByRole("tab", { name: /^Team/ }).click();
  await console.locator(".organization-detail-card").nth(3).waitFor();
  assert.equal(await console.locator(".organization-detail-card").count(), 4, "Org reaches all agents");
  await console.getByText(organizationWorktrees[0], { exact: true }).waitFor();
  await console.getByRole("tab", { name: /^Agents/ }).click();
  await assertFourAgentDensity(console);
  await assertAgentWorkspaceGeometry(page, "populated desktop Lanes page");
  await page.screenshot({ path: join(artifacts, "canvas-light-mosaic-agent.png"), fullPage: true });
  await captureMessageStreamBoundaries(page, console, artifacts, "chromium");
  const hiddenContinuity = await assertModeSwitchPreservesDeferredMosaic(
    page,
    console,
    seated[0],
    activityReplay,
  );
  await activityReplay.assertOlderToolIgnored(page, console);
  const restoredProse = await activityReplay.restoreProseSnapshot(page);
  const restoredKeys = new Set(restoredProse.map((message) => message.key));
  const postHiddenMosaic = {
    ...hiddenContinuity,
    cards: hiddenContinuity.cards.filter((card) => restoredKeys.has(card.key)),
  };
  assert.deepEqual(
    new Set(postHiddenMosaic.cards.map((card) => card.key)),
    restoredKeys,
    "the post-hidden continuity witness covers every authoritative restored message",
  );
  await assertRetainedMosaicIdentity(
    console.locator(".lane-mosaic-plane"),
    postHiddenMosaic,
    "the canonical snapshot replacement",
  );
  const restoredQuoteProse = restoredProse.find(
    (message) => message.lane === organization.quoteLane,
  )?.prose;
  assert.ok(restoredQuoteProse, "the canonical replay retains the quote lane's assistant prose");
  const tasteReply = await assertInlineTastePills(
    page,
    console,
    seated[0].lane,
    activityReplay,
  );
  const broadcastReply = await assertInlineBroadcastPill(
    page,
    console,
    seated[1].lane,
    activityReplay,
  );
  const finalReply = await assertImmediateFinalCard(
    page,
    console,
    seated[2].lane,
    activityReplay,
  );
  const tasteHeading = restoredProse.find((message) => message.lane === seated[0].lane);
  assert.ok(tasteHeading, "the taste response belongs to the primary heading lane");
  tasteHeading.prose = tasteReply;
  const broadcastHeading = restoredProse.find((message) => message.lane === seated[1].lane);
  assert.ok(broadcastHeading, "the broadcast response belongs to its heading lane");
  broadcastHeading.prose = broadcastReply;
  const finalHeading = restoredProse.find((message) => message.lane === seated[2].lane);
  assert.ok(finalHeading, "the immediate final response belongs to its heading lane");
  finalHeading.prose = finalReply;
  const clearedInbox = await reply(fixture, ["inbox", seated[2].lane]);
  for (const pending of clearedInbox.body.lanes[0].pending) {
    await reply(fixture, ["steer", "cancel", pending.directive]);
  }
  return {
    lane: organization.lane,
    quoteLane: organization.quoteLane,
    agentName: organization.agentName,
    agentAttribution: organization.agentAttribution,
    quoteProse: restoredQuoteProse,
    acknowledgementReply,
    primaryHeadings: restoredProse,
    organizationWorktrees,
  };
}

// The newest thing an agent itself said, which is what a composer heading
// shows. Each fake agent emits one assistant record naming its own worktree,
// so this is a different string per lane and seeded operator directions cannot
// masquerade as the producer's own prose.
function agentProse(worktree) {
  return `Canvas message reports \`CONTROL-1kKbC3j4\` and \`APP-a1\` from ${basename(worktree)}`;
}

export async function assertFourAgentDensity(console) {
  const geometry = await console.evaluate((fleet) => {
    const content = fleet.parentElement;
    const contentStyle = content ? getComputedStyle(content) : null;
    return {
      fleetWidth: fleet.getBoundingClientRect().width,
      contentWidth: content?.getBoundingClientRect().width ?? 0,
      contentPaddingInline: contentStyle ? [
        Number.parseFloat(contentStyle.paddingLeft),
        Number.parseFloat(contentStyle.paddingRight),
      ] : [],
      fleetOverflow: fleet.scrollWidth - fleet.clientWidth,
      headers: [...fleet.querySelectorAll(".composer-shard-heading")].map((node) => {
        const box = node.getBoundingClientRect();
        return { left: box.left, right: box.right, top: box.top, bottom: box.bottom };
      }),
      columns: [...fleet.querySelectorAll(".organization-agent")].map((node) => {
        const box = node.getBoundingClientRect();
        return { left: box.left, right: box.right, top: box.top, bottom: box.bottom };
      }),
    };
  });
  assert.equal(geometry.headers.length, 4, "four intelligent draft headers are simultaneous");
  assert.equal(geometry.columns.length, 4, "four live composer columns are simultaneous");
  assert.ok(
    Math.abs(geometry.fleetWidth + geometry.contentPaddingInline.reduce(
      (total, padding) => total + padding, 0,
    ) - geometry.contentWidth) <= 1,
    `one live lineage consumes the content box exactly once: ${JSON.stringify(geometry)}`,
  );
  assert.ok(geometry.fleetOverflow <= 1, "the organization fleet does not hide horizontal content");
  for (const row of [geometry.headers, geometry.columns]) {
    for (let index = 1; index < row.length; index += 1) {
      assert.ok(row[index - 1].right <= row[index].left + 1, "agent columns never overlap");
      assert.ok(Math.abs(row[index - 1].top - row[index].top) <= 1, "agent columns share one row");
    }
  }
}

async function assertRosterPips(page, console, seated, artifacts) {
  const organizationPips = console.locator(".organization-light");
  await organizationPips.nth(seated.length - 1).waitFor();
  const organizationPipGrid = await organizationPips.evaluateAll((pips) => pips.map((pip) => ({
    lane: pip.dataset.laneLightTargetId,
    status: pip.dataset.laneStatus,
    column: pip.dataset.pipColumn,
    row: pip.dataset.pipRow,
  })));
  const organizationRoster = [...organizationPipGrid].sort((left, right) => left.lane.localeCompare(right.lane));
  assert.deepEqual(
    organizationRoster.map((entry) => entry.lane),
    seated.map((member) => member.lane).sort(),
    "the organization pips are keyed by every admitted member and no decorative slot",
  );
  assert.ok(
    organizationRoster.every((entry) => ["running", "running-stale"].includes(entry.status)),
    "every supervised member has a lit liveness pip",
  );
  const organizationPipsByLane = new Map(organizationPipGrid.map((pip) => [pip.lane, pip]));
  assert.deepEqual(
    seated.map((member) => {
      const pip = organizationPipsByLane.get(member.lane);
      return [pip.column, pip.row];
    }),
    [["1", "1"], ["1", "2"], ["2", "1"], ["2", "2"]],
    "organization pip identities follow composers from the right edge without moving the stack",
  );
  await assertLaneHeaderPipMatrix(page, console, artifacts, "chromium", "light");

}

async function assertQueuedRosterPip(page, console, member) {
  const organizationPip = console.locator(
    `.organization-light[data-lane-light-target-id="${member.lane}"]`,
  );
  await waitForPage(page, "organization roster shows queued", TRANSITION_MS, (lane) => (
    document.querySelector(`.organization-light[data-lane-light-target-id="${lane}"]`)
      ?.getAttribute("data-lane-status") === "queued"
  ), member.lane);
  const organizationRoster = await console.locator(".organization-light").evaluateAll((pips) => pips.map((pip) => ({
    column: pip.dataset.pipColumn,
    lane: pip.dataset.laneLightTargetId,
    row: pip.dataset.pipRow,
    status: pip.dataset.laneStatus,
  })));
  assert.deepEqual(
    organizationRoster.filter(({ status }) => status === "queued").map(({ lane }) => lane),
    [member.lane],
    "only the directed member lights its organization pip as queued",
  );
  const queuedOrgPip = organizationRoster.find(({ lane }) => lane === member.lane);
  assert.deepEqual(
    [queuedOrgPip.column, queuedOrgPip.row],
    ["1", "1"],
    "the queued member keeps its reversed right-edge organization-pip assignment",
  );
  const organizationStyle = await organizationPip.evaluate((pip) => {
    const style = getComputedStyle(pip);
    return {
      backgroundColor: style.backgroundColor,
      boxShadow: style.boxShadow,
      opacity: style.opacity,
    };
  });

  assert.equal(organizationStyle.opacity, "1", "queued direction never mutes an occupied agent");
  assert.notEqual(organizationStyle.boxShadow, "none", "the queued agent remains visibly lit");
}
