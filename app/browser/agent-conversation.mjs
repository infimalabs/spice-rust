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
import {
  assertQuoteBlockDensity,
  captureQuoteBlockDensity,
} from "./quote-block-density.mjs";

const AGENT_SEAT_MS = 15_000;

export { assertMosaicLattice };
export const longAgentName = "Bexley Integration Reliability Operator";
export const longWorktreeBasename = "lane-4-release-observability-and-follow-through";
export const agentNames = ["Ava", longAgentName, "Marco"];
const teamSeatNames = [...agentNames, undefined];

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
const teamLaneIds = [
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

async function assignTeamLaneIds(page) {
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
  }, teamLaneIds);
}

/// Seat the canonical four-member fixture team without running its scenarios.
///
/// Other independently runnable browser shards need a real lineage but must
/// not replay the conversation assertions or their deliberate quiet window.
export async function seatAgentTeam(
  page,
  fixture,
  worktrees,
  assertInvalidVendor = false,
  beforeReload = null,
) {
  const teamWorktrees = worktrees.slice(0, 4);
  const console = page.locator('[data-pane="agent-console"]');
  await chooseCanvasView(page, "Agents");
  await console.waitFor();
  await console.getByRole("tab", { name: /^Info/ }).click();
  await console.getByLabel("Name").fill(agentNames[0]);
  await console.getByLabel("Worktree").fill(teamWorktrees[0]);
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

  await assignTeamLaneIds(page);
  await console.getByLabel("Vendor", { exact: true }).fill("Claude");
  await console.getByRole("button", { name: "Seat agent" }).click();
  await refusal.waitFor({ state: "detached", timeout: 10_000 });
  await waitForElement(page, "first external agent seat renders", AGENT_SEAT_MS,
    console.locator(".team-detail-card").nth(0));

  const firstSnapshot = await reply(fixture, ["lane"]);
  const host = firstSnapshot.body.lanes.find((lane) => lane.worktree === teamWorktrees[0]);
  assert.ok(host, "the Canvas seat control creates the lineage host");
  assert.equal(host.lane, teamLaneIds[0], "the fixture fixes the lineage host accent slot");
  const hostSurface = console.locator(
    `.team-surface[data-lineage-id="${host.lineage_id}"]`,
  );
  assert.equal(
    await hostSurface.locator(".team-pane").getAttribute("data-team-mode"),
    "Info",
    "the first seated lineage inherits the empty Canvas Info selection",
  );
  assert.equal(
    await hostSurface.locator(".team-detail-card").count(),
    1,
    "the inherited Info surface keeps the first seat details visible",
  );
  for (const [index, worktree] of teamWorktrees.slice(1).entries()) {
    await console.getByLabel("Name").fill(teamSeatNames[index + 1] ?? "");
    await console.getByLabel("Worktree").fill(worktree);
    await console.getByLabel("Life").fill("steer");
    await console.getByLabel("Vendor", { exact: true }).fill("Claude");
    await console.getByRole("button", { name: "Seat agent" }).click();
    await waitForElement(page, "next external agent seat renders", AGENT_SEAT_MS,
      console.locator(".team-detail-card").nth(index + 1));
  }
  for (const lane of teamLaneIds) {
    const wake = await reply(fixture, ["lane", "wake", lane]);
    assert.equal(wake.body.lane, lane, "the fixture wakes each canonical team member");
    assert.equal(wake.body.to, "occupied", "the fixture starts an agent before prose assertions");
  }
  if (beforeReload) await beforeReload(console);
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Agents");
  await console.getByRole("tab", { name: /^Info/ }).click();
  await waitForElement(page, "seated agents render after reload", CONNECT_MS,
    console.locator(".team-detail-card").nth(3));
  await waitForPage(page, "seated agents publish their session identities", CONNECT_MS, () => {
    const cards = [...document.querySelectorAll(".team-detail-card")];
    return cards.length >= 4 && cards.every((card) => !card.title.includes("thread: unbound"));
  }, undefined, {
    observed: () => console.locator(".team-detail-card").evaluateAll((cards) => (
      cards.map((card) => card.title)
    )),
  });

  const lanes = await reply(fixture, ["lane"]);
  const seated = teamWorktrees.map(
    (worktree) => lanes.body.lanes.find((lane) => lane.worktree === worktree),
  );
  assert.ok(seated.every(Boolean), "one lineage admits all four linked worktrees");
  assert.deepEqual(seated.map((lane) => lane.lane), teamLaneIds, "four fixed lane identities render");
  assert.ok(seated.every((lane) => lane.lineage_id === host.lineage_id), "one lineage renders");
  assert.deepEqual(seated.map((lane) => lane.name), teamSeatNames);
  assert.equal(seated[1].name, longAgentName, "the fixture admits a long configured agent name");
  assert.equal(
    messageFooterLabel(seated[3]),
    longWorktreeBasename,
    "the unnamed fixture lane exposes a long worktree basename",
  );
  assert.ok(seated.every((lane) => lane.vendor === "claude"));
  assert.ok(seated.every((lane) => lane.life === "steer"), "the team shares one lifetime");
  assert.equal(
    await console.locator(".team-detail-card").first().getAttribute("title"),
    `Claude vendor; model: claude-opus-5; effort: high; thread: ${seated[0].occupant}; `
      + "session: 018f7777-7777-7444-8888-999999999999",
    "the agent card tooltip carries the admitted launch and session identity",
  );

  return {
    console,
    host,
    seated,
    teamWorktrees,
    lane: seated[0].lane,
    quoteLane: seated[3].lane,
    agentName: agentNames[0],
    agentAttribution: canonicalAgentAttribution(seated[0]),
    quoteProse: agentProse(teamWorktrees[3]),
    primaryHeadings: seated.map((lane, index) => ({
      lane: lane.lane,
      prose: index === 0
        ? "newest lane prose witness"
        : agentProse(teamWorktrees[index]),
    })),
  };
}

/// Seat one two-member lineage directly under automatic task policy.
///
/// Unlike the conversation fixture, this lane never transitions from an
/// already-running Steer process into task work. Their first rendered composers
/// and automatic leases therefore belong to one startup generation.
export async function seatLeaseAgent(page, fixture, worktrees) {
  const [expiryWorktree, releaseWorktree] = worktrees;
  const console = page.locator('[data-pane="agent-console"]');
  await chooseCanvasView(page, "Agents");
  await console.getByRole("tab", { name: /^Info/ }).click();
  await console.getByLabel("Name").fill(agentNames[0]);
  await console.getByLabel("Worktree").fill(expiryWorktree);
  await console.getByLabel("Life").fill("steer");
  await assignTeamLaneIds(page);
  await console.getByLabel("Vendor", { exact: true }).fill("Claude");
  await console.getByRole("button", { name: "Seat agent" }).click();
  await waitForElement(page, "automatic lease agent renders", AGENT_SEAT_MS,
    console.locator(".team-detail-card").first());

  await console.getByLabel("Name").fill(agentNames[1]);
  await console.getByLabel("Worktree").fill(releaseWorktree);
  await console.getByLabel("Life").fill("steer");
  await console.getByLabel("Vendor", { exact: true }).fill("Claude");
  await console.getByRole("button", { name: "Seat agent" }).click();
  await waitForElement(page, "explicit release lease agent renders", AGENT_SEAT_MS,
    console.locator(".team-detail-card").nth(1));

  const lanes = await reply(fixture, ["lane"]);
  const seated = [expiryWorktree, releaseWorktree].map(
    (worktree) => lanes.body.lanes.find((lane) => lane.worktree === worktree),
  );
  assert.ok(seated.every(Boolean), "both lease agents have durable standings");
  assert.deepEqual(
    seated.map((lane) => lane.lane),
    teamLaneIds.slice(0, 2),
    "the lease fixture fixes both lane identities",
  );
  for (const [index, lane] of seated.entries()) {
    await reply(fixture, ["lane", "wake", lane.lane]);
    await replyAsLane(fixture, lane.lane, [
      "task", "add", index === 0 ? "Lease provenance witness" : "Explicit release witness",
      "--project", "app.browser",
      "--acceptance", "the automatic lease remains observable in Canvas",
    ]);
    await reply(fixture, ["lane", "drain", lane.lane]);
  }
  await console.getByRole("tab", { name: /^Compose/ }).click();
  await console.locator('.team-lifetime input[type="range"]').fill("1");
  await console.getByRole("button", { name: "Drive", exact: true }).waitFor();
  for (const worktree of [expiryWorktree, releaseWorktree]) {
    await waitForLeaseAgentOccupant(fixture, worktree, AGENT_SEAT_MS * 2);
  }
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Agents");
  await console.getByRole("tab", { name: /^Compose/ }).click();
  for (const lane of seated) {
    await console.locator(`.team-agent[data-agent-lane="${lane.lane}"]`).waitFor();
  }
  return { console, lanes: seated.map((lane) => lane.lane) };
}

async function waitForLeaseAgentOccupant(fixture, worktree, timeout) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    const lanes = await reply(fixture, ["lane"]);
    last = lanes.body.lanes.find((lane) => lane.worktree === worktree);
    if (last?.occupant) return last;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for automatic lease occupant: ${JSON.stringify(last)}`);
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
  const composer = console.locator(`.team-agent[data-agent-lane="${lane}"]`);
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
  await chooseCanvasView(page, "Agents");
  await console.getByRole("tab", { name: /^Compose/ }).click();
  await assertOperatorProseHidden(console, replyText, "pre-acknowledgement replay");

  await writeFile(join(worktree, ".spice-ack-trigger"), `${sent.body.key}\n`);
  const acknowledgedCard = console.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
  ).filter({ hasText: replyText });
  await acknowledgedCard.waitFor({ timeout: CONNECT_MS });
  assert.equal(await acknowledgedCard.count(), 1, "acknowledgement renders one assistant card");
  assert.equal(await acknowledgedCard.getAttribute("data-message-role"), "agent");
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
  await chooseCanvasView(page, "Agents");
  await console.getByRole("tab", { name: /^Compose/ }).click();
  await acknowledgedCard.waitFor({ timeout: CONNECT_MS });
  assert.equal(await acknowledgedCard.count(), 1, "bootstrap restores one acknowledged assistant card");
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

export async function assertAgentConversation(
  page,
  fixture,
  worktrees,
  activityReplay,
  artifacts,
) {
  const team = await seatAgentTeam(
    page,
    fixture,
    worktrees,
    true,
    (console) => activityReplay.assertBootstrapLiveMessages(console),
  );
  const {
    console,
    host,
    seated,
    teamWorktrees,
  } = team;

  await console.getByRole("tab", { name: /^Compose/ }).click();
  await console.locator(".team-agent").nth(3).waitFor();
  assert.equal(await console.locator(".team-surface").count(), 1, "one lineage renders one surface");
  assert.equal(
    await console.locator(".team-activity-grid").count(),
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
    teamWorktrees[0],
  );
  const currentProse = seated.map((member) => activityReplay.nextAgentProse(member.lane));
  await Promise.all([
    writeFile(join(teamWorktrees[0], ".spice-tool-trigger"), "emit\n"),
    ...teamWorktrees.slice(0, seated.length).map((worktree) => (
      writeFile(join(worktree, ".spice-prose-trigger"), "emit\n")
    )),
  ]);
  await Promise.all(currentProse);
  await assertRosterPips(page, console, seated, artifacts);
  const accessibility = await console.locator(".team-activity-grid").ariaSnapshot();
  for (const name of producerNames) {
    assert.match(accessibility, new RegExp(name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  }
  assert.equal(await console.getByRole("button", { name: "Steer", exact: true }).count(), 1);
  const lifetime = console.locator('.team-lifetime input[type="range"]');
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
    const column = console.locator(`.team-agent[data-agent-lane="${lane.lane}"]`);
    await column.getByRole("textbox").fill(messages[index]);
    await console.getByRole("button", { name: "Steer", exact: true }).click();
  }
  const quoteLane = console.locator(`.team-agent[data-agent-lane="${seated[3].lane}"]`);
  await quoteLane.getByRole("textbox").fill(entirelyQuotedMessage);
  await console.getByRole("button", { name: "Steer", exact: true }).click();
  const tallLane = console.locator(`.team-agent[data-agent-lane="${seated[1].lane}"]`);
  await tallLane.getByRole("textbox").fill(tallMosaicMessage);
  await console.getByRole("button", { name: "Steer", exact: true }).click();
  const refreshedProse = seated.map((member) => activityReplay.nextAgentProse(member.lane));
  await Promise.all(teamWorktrees.slice(0, seated.length).map((worktree) => (
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
  await assertPersistentMosaic(page, fixture, console, plane, seated[0], activityReplay);
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
    { label: "Speak this message", icon: "▶" },
    { label: "Quote this message", icon: "❝" },
    { label: "Copy this message", icon: "⧉" },
  ];
  const actionCard = console.locator('[data-card-kind="agent-message"]').first();
  assert.ok((await actionCard.locator(".message-agent-name").innerText()).trim());
  assert.deepEqual(
    await actionCard.locator(".message-actions button").evaluateAll(
      (buttons) => buttons.map((button) => ({
        label: button.getAttribute("aria-label"),
        icon: button.querySelector(".card-action-icon")?.textContent,
        iconHidden: button.querySelector(".card-action-icon")?.getAttribute("aria-hidden"),
        title: button.title,
      })),
    ),
    expectedActions.map(({ label, icon }) => ({
      label,
      icon,
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
  await assertQueuedRosterPip(page, console, seated[0]);
  const age = outgoing.locator(".message-age");
  assert.match((await age.innerText()).trim(), /^(?:now|\d+[smhd])$/);
  assert.match(await age.getAttribute("title"), /^tape \d+:\d+\+\d+#\d+$/);
  assert.equal(await age.getAttribute("data-age-clock"), "deadline");
  assert.match(await age.getAttribute("data-age-authored"), /^\d+$/);
  assert.equal(await outgoing.locator(".message-actions button").count(), 3);

  await console.getByRole("tab", { name: /^Tasks/ }).click();
  await console.locator(".team-task-workspace").waitFor();
  assert.equal(
    await console.locator(".team-task-workspace").count(),
    1,
    "Tasks renders one team-scoped operational workspace",
  );
  for (const mode of ["Metrics", "Info"]) {
    await console.getByRole("tab", { name: new RegExp(`^${mode}`) }).click();
    await console.locator(".team-detail-card").nth(3).waitFor();
    assert.equal(await console.locator(".team-detail-card").count(), 4, `${mode} reaches all agents`);
  }
  await console.getByText(teamWorktrees[0], { exact: true }).waitFor();
  await console.getByRole("tab", { name: /^Compose/ }).click();
  await assertFourAgentDensity(console);
  await assertAgentWorkspaceGeometry(page, "populated desktop Agents page");
  await page.screenshot({ path: join(artifacts, "canvas-light-mosaic-agent.png"), fullPage: true });
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
    (message) => message.lane === team.quoteLane,
  )?.prose;
  assert.ok(restoredQuoteProse, "the canonical replay retains the quote lane's assistant prose");
  const clearedInbox = await reply(fixture, ["inbox", seated[2].lane]);
  for (const pending of clearedInbox.body.lanes[0].pending) {
    await reply(fixture, ["steer", "cancel", pending.directive]);
  }
  return {
    lane: team.lane,
    quoteLane: team.quoteLane,
    agentName: team.agentName,
    agentAttribution: team.agentAttribution,
    quoteProse: restoredQuoteProse,
    acknowledgementReply,
    postHiddenMosaic,
    primaryHeadings: restoredProse,
    teamWorktrees,
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
      columns: [...fleet.querySelectorAll(".team-agent")].map((node) => {
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
  assert.ok(geometry.fleetOverflow <= 1, "the team fleet does not hide horizontal content");
  for (const row of [geometry.headers, geometry.columns]) {
    for (let index = 1; index < row.length; index += 1) {
      assert.ok(row[index - 1].right <= row[index].left + 1, "agent columns never overlap");
      assert.ok(Math.abs(row[index - 1].top - row[index].top) <= 1, "agent columns share one row");
    }
  }
}

async function assertRosterPips(page, console, seated, artifacts) {
  const teamPips = console.locator(".team-light");
  await teamPips.nth(seated.length - 1).waitFor();
  const teamPipGrid = await teamPips.evaluateAll((pips) => pips.map((pip) => ({
    lane: pip.dataset.laneLightTargetId,
    status: pip.dataset.laneStatus,
    column: pip.dataset.pipColumn,
    row: pip.dataset.pipRow,
  })));
  const teamRoster = [...teamPipGrid].sort((left, right) => left.lane.localeCompare(right.lane));
  assert.deepEqual(
    teamRoster.map((entry) => entry.lane),
    seated.map((member) => member.lane).sort(),
    "the team pips are keyed by every admitted member and no decorative slot",
  );
  assert.ok(
    teamRoster.every((entry) => ["running", "running-stale"].includes(entry.status)),
    "every supervised member has a lit liveness pip",
  );
  const teamPipsByLane = new Map(teamPipGrid.map((pip) => [pip.lane, pip]));
  assert.deepEqual(
    seated.map((member) => {
      const pip = teamPipsByLane.get(member.lane);
      return [pip.column, pip.row];
    }),
    [["1", "1"], ["1", "2"], ["2", "1"], ["2", "2"]],
    "team pip identities follow composers from the right edge without moving the stack",
  );
  await assertLaneHeaderPipMatrix(page, console, artifacts, "chromium", "light");

  await chooseCanvasView(page, "Flow");
  const brandPips = page.locator(".brand-dot");
  await brandPips.nth(seated.length - 1).waitFor();
  const chromeRoster = await brandPips.evaluateAll((pips) => pips.map((pip) => ({
    lane: pip.dataset.agentLane,
    status: pip.dataset.agentStatus,
  })).sort((left, right) => left.lane.localeCompare(right.lane)));
  assert.deepEqual(
    chromeRoster,
    teamRoster.map(({ lane, status }) => ({ lane, status })),
    "the compact chrome roster preserves the live team identities and liveness states",
  );
  const chromePipGrid = await brandPips.evaluateAll((pips) => pips.map((pip) => {
    const box = pip.getBoundingClientRect();
    return {
      column: pip.dataset.pipColumn,
      lane: pip.dataset.agentLane,
      left: box.left,
      row: pip.dataset.pipRow,
      top: box.top,
    };
  }));
  const chromePipsByLane = new Map(chromePipGrid.map((pip) => [pip.lane, pip]));
  const orderedPips = seated.map((member) => chromePipsByLane.get(member.lane));
  assert.deepEqual(
    orderedPips.map(({ column, row }) => [column, row]),
    [["1", "1"], ["2", "1"], ["1", "2"], ["2", "2"]],
    "compact pip identities follow the same right-edge member order",
  );
  assert.equal(orderedPips[0].left, orderedPips[2].left);
  assert.ok(orderedPips[0].top < orderedPips[2].top);
  assert.ok(orderedPips[1].left > orderedPips[0].left);
  assert.equal(orderedPips[1].top, orderedPips[0].top);
  assert.equal(orderedPips[1].left, orderedPips[3].left);
  assert.equal(orderedPips[3].top, orderedPips[2].top);
  await assertPartialPipStack(page, artifacts);
  assert.equal(
    await page.locator("button.axis-button", { hasText: /^Mosaic/ }).count(),
    0,
    "analytic chrome keeps the layout implementation name private",
  );
  await chooseCanvasView(page, "Agents");
  await console.waitFor();
  await waitForMosaicReady(page);
}

async function assertPartialPipStack(page, artifacts) {
  const originalViewport = page.viewportSize();
  assert.ok(originalViewport, "pip screenshot coverage requires a fixed viewport");
  await page.evaluate(() => {
    const original = document.querySelector(".brand-dots");
    const clone = original.cloneNode(true);
    const upperRight = clone.querySelector('[data-pip-column="2"][data-pip-row="1"]');
    if (!upperRight) throw new Error("four-pip roster is missing its upper-right pip");
    upperRight.remove();
    clone.setAttribute("aria-label", "3 admitted agents");
    clone.dataset.pipLayoutWitness = "three";
    window.__pipLayoutOriginal = original;
    original.replaceWith(clone);
  });
  const witness = page.locator('[data-pip-layout-witness="three"]');
  const geometry = async () => witness.locator(".brand-dot").evaluateAll((pips) => (
    pips.map((pip) => {
      const box = pip.getBoundingClientRect();
      return {
        column: pip.dataset.pipColumn,
        left: box.left,
        row: pip.dataset.pipRow,
        top: box.top,
      };
    })
  ));
  const assertGeometry = (pips, viewport) => {
    assert.equal(pips.length, 3, `${viewport} compact roster contains three physical pips`);
    pips.sort((left, right) => (
      Number(right.column) - Number(left.column) || Number(right.row) - Number(left.row)
    ));
    assert.deepEqual(
      pips.map(({ column, row }) => [column, row]),
      [["2", "2"], ["1", "2"], ["1", "1"]],
      `${viewport} partial grid completes the left column and leaves the lower-right pip`,
    );
    assert.ok(pips[1].left < pips[0].left);
    assert.equal(pips[1].top, pips[0].top);
    assert.equal(pips[1].left, pips[2].left);
    assert.ok(pips[1].top > pips[2].top);
  };
  const browser = page.context().browser().browserType().name();
  try {
    assertGeometry(await geometry(), "desktop");
    await page.screenshot({
      path: join(artifacts, `${browser}-pip-grid-three-desktop.png`),
      clip: { x: 0, y: 0, width: Math.min(originalViewport.width, 640), height: 180 },
    });
    await page.setViewportSize({ width: 480, height: 720 });
    assertGeometry(await geometry(), "narrow");
    await page.screenshot({
      path: join(artifacts, `${browser}-pip-grid-three-narrow.png`),
      clip: { x: 0, y: 0, width: 480, height: 180 },
    });
  } finally {
    await page.setViewportSize(originalViewport);
    await page.evaluate(() => {
      const witness = document.querySelector('[data-pip-layout-witness="three"]');
      if (witness && window.__pipLayoutOriginal) witness.replaceWith(window.__pipLayoutOriginal);
      delete window.__pipLayoutOriginal;
    });
  }
}

async function assertQueuedRosterPip(page, console, member) {
  const teamPip = console.locator(
    `.team-light[data-lane-light-target-id="${member.lane}"]`,
  );
  await waitForPage(page, "team roster shows queued", TRANSITION_MS, (lane) => (
    document.querySelector(`.team-light[data-lane-light-target-id="${lane}"]`)
      ?.getAttribute("data-lane-status") === "queued"
  ), member.lane);
  const teamStyle = await teamPip.evaluate((pip) => {
    const style = getComputedStyle(pip);
    return {
      backgroundColor: style.backgroundColor,
      boxShadow: style.boxShadow,
      opacity: style.opacity,
    };
  });

  await chooseCanvasView(page, "Flow");
  const brandPip = page.locator(`.brand-dot[data-agent-lane="${member.lane}"]`);
  assert.equal(await brandPip.count(), 1, "the queued member retains exactly one brand pip");
  await waitForPage(page, "brand roster shows queued", TRANSITION_MS, (lane) => (
    document.querySelector(`.brand-dot[data-agent-lane="${lane}"]`)
      ?.getAttribute("data-agent-status") === "queued"
  ), member.lane);
  assert.equal(await brandPip.getAttribute("data-agent-status"), "queued");
  const brandStyle = await brandPip.evaluate((pip) => {
    const style = getComputedStyle(pip);
    return {
      backgroundColor: style.backgroundColor,
      boxShadow: style.boxShadow,
      opacity: style.opacity,
    };
  });
  assert.deepEqual(
    brandStyle,
    teamStyle,
    "a queued live member keeps the team roster's visible warning state in the brand roster",
  );
  assert.equal(brandStyle.opacity, "1", "queued direction never mutes an occupied agent");
  assert.notEqual(brandStyle.boxShadow, "none", "the queued agent remains visibly lit");
  await chooseCanvasView(page, "Agents");
  await console.waitFor();
  await waitForMosaicReady(page);
}
