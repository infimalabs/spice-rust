import assert from "node:assert/strict";
import { basename, join } from "node:path";

import {
  assertInitiallyDeferredMosaicIsReadable,
  assertMosaicLattice,
  assertPersistentMosaic,
  waitForMosaicReady,
} from "./agent-conversation-mosaic.mjs";
import { assertReferenceControlStack } from "./control-stack.mjs";
import {
  assertHeldTaskCount,
  chooseCanvasView,
  reply,
  CONNECT_MS,
  TRANSITION_MS,
  waitForLocator as waitForElement,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

const AGENT_SEAT_MS = 15_000;

export { assertMosaicLattice };
export const agentNames = ["Ava", "Bex", "Cy", "Dee"];
/// The fourth identity ends in 6 rather than 3 on purpose, and tidying it back
/// to a run of four consecutive values would silently delete coverage. An
/// accent slot is the lane identity modulo six, so consecutive identities can
/// never collide, and a fixture built from them would prove only the easy half
/// of the contract. Ending in 6 puts the fourth lane on the first lane's slot,
/// which is the ordinary case a lineage meets -- six members hold six distinct
/// slots 720 times in 46656 -- and lets this file assert that a shared colour
/// still leaves two agents told apart by name.
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
    await console.getByLabel("Vendor").fill("My Agent");
    await console.getByRole("button", { name: "Seat agent" }).click();
    await refusal.waitFor({ timeout: 10_000 });
    assert.match(await refusal.innerText(), /Vendor must be one lower-case word/);
  }

  await assignTeamLaneIds(page);
  await console.getByLabel("Vendor").fill("Claude");
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
    await console.getByLabel("Name").fill(agentNames[index + 1]);
    await console.getByLabel("Worktree").fill(worktree);
    await console.getByLabel("Life").fill("steer");
    await console.getByLabel("Vendor").fill("Claude");
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

  const lanes = await reply(fixture, ["lane"]);
  const seated = teamWorktrees.map(
    (worktree) => lanes.body.lanes.find((lane) => lane.worktree === worktree),
  );
  assert.ok(seated.every(Boolean), "one lineage admits all four linked worktrees");
  assert.deepEqual(seated.map((lane) => lane.lane), teamLaneIds, "four fixed lane identities render");
  assert.ok(seated.every((lane) => lane.lineage_id === host.lineage_id), "one lineage renders");
  assert.deepEqual(seated.map((lane) => lane.name), agentNames);
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
    quoteProse: agentProse(teamWorktrees[3]),
    primaryHeadings: seated.map((lane, index) => ({
      lane: lane.lane,
      prose: index === 0
        ? "newest lane prose witness"
        : agentProse(teamWorktrees[index]),
    })),
  };
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
  const producerNames = new Set(await authored.locator(".message-author").allTextContents());
  assert.ok(producerIds.size > 1, "the packed plane contains more than one producing agent");
  assert.ok(producerNames.size > 1, "each authored card footer names its producing agent");
  await assertInitiallyDeferredMosaicIsReadable(page, console);
  await assertRosterPips(page, console, seated);
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
    await console
      .locator(`.mosaic-message-card[data-agent-lane="${lane.lane}"]`)
      .filter({ hasText: messageLabels[index] })
      .waitFor({ timeout: 10_000 });
  }
  const quoteLane = console.locator(`.team-agent[data-agent-lane="${seated[3].lane}"]`);
  await quoteLane.getByRole("textbox").fill(entirelyQuotedMessage);
  await console.getByRole("button", { name: "Steer", exact: true }).click();
  await console
    .locator(`.mosaic-message-card[data-agent-lane="${seated[3].lane}"]`)
    .filter({ hasText: "Borrowed context has no authored preview." })
    .waitFor({ timeout: 10_000 });
  const tallLane = console.locator(`.team-agent[data-agent-lane="${seated[1].lane}"]`);
  await tallLane.getByRole("textbox").fill(tallMosaicMessage);
  await console.getByRole("button", { name: "Steer", exact: true }).click();
  await console
    .locator(`.mosaic-message-card[data-agent-lane="${seated[1].lane}"]`)
    .filter({ hasText: "Measured lattice witness" })
    .waitFor({ timeout: 10_000 });
  const orderedInbox = await reply(fixture, ["inbox", seated[1].lane]);
  assert.deepEqual(
    await console
      .locator(`[data-card-kind="directive"][data-agent-lane="${seated[1].lane}"]`)
      .evaluateAll((cards) => cards.map((card) => card.dataset.directive)),
    orderedInbox.body.lanes[0].pending.map((directive) => directive.directive),
    "pending directives reach the lattice in durable journal order",
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
  assert.ok(placements.length > seated.length, "the lane packs agent and directive cards");
  assert.ok(placements.every((card) => card.span >= 1 && card.track + card.span <= 12));
  assert.ok(placements.every((card) => card.rows >= 3));
  assert.ok(placements.every((card) => card.columnStart === card.track + 1));
  assert.ok(placements.every((card) => card.row === card.base));
  assert.equal(Math.min(...placements.map((card) => card.base)), 0, "the lane owns its row floor");
  const packedMembers = new Set(placements.map((card) => card.lane));
  for (const member of seated) {
    assert.ok(packedMembers.has(member.lane), "every worktree member reaches the lane plane");
  }
  await assertPersistentMosaic(page, fixture, console, plane, seated[0]);

  const directiveChrome = await console.locator('[data-card-kind="directive"]').evaluateAll(
    (cards) => cards.map((card) => ({
      lane: card.dataset.agentLane,
      accent: card.dataset.accent,
      edge: getComputedStyle(card).borderRightColor,
    })),
  );
  const repeatedLane = seated[1].lane;
  const repeatedChrome = directiveChrome.filter((card) => card.lane === repeatedLane);
  assert.ok(repeatedChrome.length >= 2, "the accent probe has two cards from one lane");
  assert.ok(repeatedChrome.every((card) => card.accent === repeatedChrome[0].accent));
  assert.ok(repeatedChrome.every((card) => card.edge === repeatedChrome[0].edge));
  const distinctChrome = directiveChrome.find(
    (card) => card.lane !== repeatedLane && card.accent !== repeatedChrome[0].accent,
  );
  assert.ok(distinctChrome, "the fixture exposes a different lane accent slot");
  assert.notEqual(distinctChrome.edge, repeatedChrome[0].edge);

  // The first and fourth lanes hold one slot between them, by the construction
  // teamLaneIds explains. That is the ordinary case for a lineage, so the
  // colour is decoration and the assertions below are what an operator and a
  // test actually read to tell the two apart. A message card names its producer
  // as the worktree leaf beside the short author; a directive card names only
  // the short lane, which is why the pairing is asserted on message cards.
  const messageChrome = await console.locator('[data-card-kind="agent-message"]').evaluateAll(
    (cards) => cards.map((card) => ({
      lane: card.dataset.agentLane,
      accent: card.dataset.accent,
      producer: card.querySelector(".message-author")?.textContent?.trim() ?? "",
    })),
  );
  const sharedSlot = [seated[0].lane, seated[3].lane].map(
    (lane) => messageChrome.find((card) => card.lane === lane),
  );
  assert.ok(sharedSlot.every(Boolean), "both lanes sharing a slot render a message card");
  assert.equal(
    sharedSlot[0].accent, sharedSlot[1].accent,
    "the seeded pair holds one accent slot, so the colour names neither lane",
  );
  assert.notEqual(sharedSlot[0].lane, sharedSlot[1].lane);
  assert.notEqual(
    sharedSlot[0].producer, sharedSlot[1].producer,
    "the producer name is what stays distinct when the colour does not",
  );
  const expectedActions = ["Speak this message", "Quote this message", "Copy this message"];
  for (const kind of ["agent-message", "directive"]) {
    const card = console.locator(`[data-card-kind="${kind}"]`).first();
    assert.ok((await card.locator(".message-author").innerText()).trim());
    assert.deepEqual(
      await card.locator(".message-actions button").evaluateAll(
        (buttons) => buttons.map((button) => button.getAttribute("aria-label")),
      ),
      expectedActions,
      `${kind} exposes the reference speak, quote, and copy controls`,
    );
  }

  const scroller = plane;
  const compensationViewport = page.viewportSize();
  await page.setViewportSize({ width: compensationViewport.width, height: 360 });
  await scroller.evaluate((element) => element.scrollTo(0, element.scrollHeight));
  const beforeCompensation = await scroller.evaluate((element) => {
    return {
      scrollTop: element.scrollTop,
      maxRow: Number(element.dataset.maxRow),
    };
  });
  assert.ok(beforeCompensation.scrollTop > 80, "the compensation probe begins in a scrolled pane");
  const compensationMessage = [
    "mosaic scroll compensation probe",
    ...Array.from({ length: 72 }, (_, index) => `- exact row delta ${index + 1}`),
  ].join("\n");
  const compensationLane = console.locator(
    `.team-agent[data-agent-lane="${seated[0].lane}"]`,
  );
  await compensationLane.getByRole("textbox").fill(compensationMessage);
  await console.getByRole("button", { name: "Steer", exact: true }).click();
  const compensationCard = console
    .locator(`.mosaic-message-card[data-agent-lane="${seated[0].lane}"]`)
    .filter({ hasText: "mosaic scroll compensation probe" });
  await compensationCard.waitFor({ timeout: 10_000 });
  await waitForPage(page, "mosaic rows extend for the streamed card", TRANSITION_MS, (before) => {
    const element = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    const maxRow = Number(element?.dataset.maxRow);
    return maxRow > before.maxRow;
  }, beforeCompensation);
  const afterCompensation = await scroller.evaluate((element) => {
    return {
      scrollTop: element.scrollTop,
      maxRow: Number(element.dataset.maxRow),
      module: Number(element.dataset.module),
    };
  });
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
  await page.setViewportSize(compensationViewport);
  const compensationInbox = await reply(fixture, ["inbox", seated[0].lane]);
  const compensationDirective = compensationInbox.body.lanes[0].pending.find(
    (directive) => directive.text === compensationMessage,
  );
  assert.ok(compensationDirective, "the compensation probe reached the real inbox");
  await reply(fixture, ["steer", "cancel", compensationDirective.directive]);
  await compensationCard.waitFor({ state: "detached", timeout: 10_000 });
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
  assert.equal(
    await outgoing.locator(".message-key").innerText(),
    `${pendingDirective.key} · ${pendingDirective.stage}`,
  );
  await assertQueuedRosterPip(page, console, seated[0]);
  const age = outgoing.locator(".message-age");
  const authoredAt = Date.parse(inbox.body.lanes[0].pending[0].authored);
  const beforeRead = Date.now();
  const renderedAge = await age.innerText();
  const accepted = ageLabelsWithinOneTick(authoredAt, beforeRead, Date.now());
  assert.ok(
    accepted.includes(renderedAge),
    `the card shows authored age: ${JSON.stringify({ renderedAge, accepted })}`,
  );
  assert.equal(
    await age.getAttribute("title"),
    `stage ${pendingDirective.stage}; authored ${pendingDirective.authored}; expires ${pendingDirective.expires}`,
  );
  assert.equal(await outgoing.locator(".message-actions button").count(), 3);

  for (const mode of ["Filters", "Metrics", "Info"]) {
    await console.getByRole("tab", { name: new RegExp(`^${mode}`) }).click();
    await console.locator(".team-detail-card").nth(3).waitFor();
    assert.equal(await console.locator(".team-detail-card").count(), 4, `${mode} reaches all agents`);
  }
  await console.getByText(teamWorktrees[0], { exact: true }).waitFor();
  await console.getByRole("tab", { name: /^Compose/ }).click();
  await assertFourAgentDensity(console);
  await page.screenshot({ path: join(artifacts, "canvas-light-mosaic-agent.png"), fullPage: true });
  await assertHeldTaskCount(console, fixture, seated[0].lane);
  await activityReplay.assertOlderToolIgnored(page, console);
  const clearedInbox = await reply(fixture, ["inbox", seated[2].lane]);
  for (const pending of clearedInbox.body.lanes[0].pending) {
    await reply(fixture, ["steer", "cancel", pending.directive]);
  }
  return {
    lane: team.lane,
    quoteLane: team.quoteLane,
    agentName: team.agentName,
    quoteProse: team.quoteProse,
    primaryHeadings: team.primaryHeadings,
    teamWorktrees,
  };
}

// The newest thing an agent itself said, which is what a composer heading
// shows. Each fake agent emits one assistant record naming its own worktree,
// so this is a different string per lane and seeded operator messages cannot
// masquerade as the producer's own prose.
function agentProse(worktree) {
  return `Canvas message reports CONTROL-1kKbC3j4 from ${basename(worktree)}`;
}

// Mirrors `app::watchband::elapsed_label`, including the saturating day arm
// that keeps the label inside its fixed slot. Drifting from the Rust bound
// would make this the more permissive of the two and hide a widened label
// rather than catch it, so the ceiling is restated here rather than left out
// as unreachable in the fixtures.
const AGE_LABEL_MAX_DAYS = 99;

function elapsedLabel(millis) {
  const seconds = Math.trunc(millis / 1_000);
  if (seconds <= 0) return "now";
  if (seconds < 60) return `${seconds}s`;
  if (seconds < 3_600) return `${Math.trunc(seconds / 60)}m`;
  if (seconds < 86_400) return `${Math.trunc(seconds / 3_600)}h`;
  const days = Math.trunc(seconds / 86_400);
  return days <= AGE_LABEL_MAX_DAYS ? `${days}d` : `${AGE_LABEL_MAX_DAYS}d+`;
}

/// The shared clock's period, mirroring `TICK_MILLIS` in
/// `app/src/browser/clock.rs`.
const AGE_TICK_MS = 1_000;

/// Every label the card may legitimately be showing when Node reads it.
///
/// The canvas no longer spells an age from the instant its frame rendered; it
/// spells one from a clock published once a tick, which is what lets a quiet
/// lane keep ageing. The cost is that the label trails a Node-side reading by
/// up to one publication, so demanding the label a `Date.now()` taken at the
/// read implies is demanding sub-tick agreement the design never promised.
///
/// The window is one tick and nothing more. It is not a load allowance --
/// `canvas-load.mjs` argues at length that load belongs in the report rather
/// than in the bound -- and it stays narrow enough to fail: an age spelled from
/// the wrong instant misses by minutes, not by a second.
function ageLabelsWithinOneTick(authoredAt, beforeRead, afterRead) {
  // Stepping a whole tick at a time cannot skip a label, because two instants
  // one tick apart land in adjacent seconds; the closing instant is added
  // separately so a read that took less than a step still contributes its own.
  const labels = new Set([elapsedLabel(afterRead - authoredAt)]);
  for (let instant = beforeRead - AGE_TICK_MS; instant < afterRead; instant += AGE_TICK_MS) {
    labels.add(elapsedLabel(instant - authoredAt));
  }
  return [...labels];
}

export async function assertFourAgentDensity(console) {
  const geometry = await console.evaluate((surface) => ({
    surfaceWidth: surface.getBoundingClientRect().width,
    contentWidth: surface.parentElement?.getBoundingClientRect().width ?? 0,
    surfaceOverflow: surface.scrollWidth - surface.clientWidth,
    headers: [...surface.querySelectorAll(".composer-shard-heading")].map((node) => {
      const box = node.getBoundingClientRect();
      return { left: box.left, right: box.right, top: box.top, bottom: box.bottom };
    }),
    columns: [...surface.querySelectorAll(".team-agent")].map((node) => {
      const box = node.getBoundingClientRect();
      return { left: box.left, right: box.right, top: box.top, bottom: box.bottom };
    }),
  }));
  assert.equal(geometry.headers.length, 4, "four intelligent draft headers are simultaneous");
  assert.equal(geometry.columns.length, 4, "four live composer columns are simultaneous");
  assert.ok(
    geometry.surfaceWidth >= geometry.contentWidth - 16 &&
      geometry.surfaceWidth <= geometry.contentWidth + 1,
    "one live lineage consumes the available reference viewport",
  );
  assert.ok(geometry.surfaceOverflow <= 1, "the team surface does not hide horizontal content");
  for (const row of [geometry.headers, geometry.columns]) {
    for (let index = 1; index < row.length; index += 1) {
      assert.ok(row[index - 1].right <= row[index].left + 1, "agent columns never overlap");
      assert.ok(Math.abs(row[index - 1].top - row[index].top) <= 1, "agent columns share one row");
    }
  }
}

async function assertRosterPips(page, console, seated) {
  const teamPips = console.locator(".team-light");
  await teamPips.nth(seated.length - 1).waitFor();
  const teamRoster = await teamPips.evaluateAll((pips) => pips.map((pip) => ({
    lane: pip.dataset.laneLightTargetId,
    status: pip.dataset.laneStatus,
  })).sort((left, right) => left.lane.localeCompare(right.lane)));
  assert.deepEqual(
    teamRoster.map((entry) => entry.lane),
    seated.map((member) => member.lane).sort(),
    "the team pips are keyed by every admitted member and no decorative slot",
  );
  assert.ok(
    teamRoster.every((entry) => ["running", "running-stale"].includes(entry.status)),
    "every supervised member has a lit liveness pip",
  );

  await chooseCanvasView(page, "Flow");
  const brandPips = page.locator(".brand-dot");
  await brandPips.nth(seated.length - 1).waitFor();
  const chromeRoster = await brandPips.evaluateAll((pips) => pips.map((pip) => ({
    lane: pip.dataset.agentLane,
    status: pip.dataset.agentStatus,
  })).sort((left, right) => left.lane.localeCompare(right.lane)));
  assert.deepEqual(
    chromeRoster,
    teamRoster,
    "the compact chrome roster preserves the live team identities and liveness states",
  );
  const chromePipGrid = await brandPips.evaluateAll((pips) => pips.map((pip) => {
    const box = pip.getBoundingClientRect();
    return {
      left: box.left,
      top: box.top,
    };
  }));
  assert.equal(chromePipGrid[0].left, chromePipGrid[1].left);
  assert.ok(chromePipGrid[0].top > chromePipGrid[1].top);
  assert.ok(chromePipGrid[2].left < chromePipGrid[0].left);
  assert.equal(chromePipGrid[2].top, chromePipGrid[0].top);
  assert.equal(chromePipGrid[2].left, chromePipGrid[3].left);
  assert.equal(chromePipGrid[3].top, chromePipGrid[1].top);
  assert.equal(
    await page.locator("button.axis-button", { hasText: /^Mosaic/ }).count(),
    0,
    "analytic chrome keeps the layout implementation name private",
  );
  await chooseCanvasView(page, "Agents");
  await console.waitFor();
  await waitForMosaicReady(page);
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
