import assert from "node:assert/strict";
import { mkdtemp, mkdir, realpath, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { installActivityReplay } from "./activity-replay.mjs";
import { assertAgesTickWhileTheResidentIsSilent } from "./age-ticker.mjs";
import {
  agentNames,
  assertAgentConversation,
  assertFourAgentDensity,
  assertMosaicLattice,
  entirelyQuotedMessage,
  nestedQuoteMessage,
} from "./agent-conversation.mjs";
import {
  assertComposerShards,
  primaryDestination,
} from "./composer-shards.mjs";
import { assertCompleteComposerSubmission } from "./composer-submission.mjs";
import { assertComposerRecovery } from "./composer-recovery.mjs";
import { assertCanvas } from "./canvas-readiness.mjs";
import { assertPrimaryComposerDragging } from "./composer-dragging.mjs";
import { assertAgentRenewalRequest, captureReferenceControls } from "./control-reference.mjs";
import { assertMotionPreference } from "./mosaic-motion.mjs";
import { assertTeamArrangement } from "./team-arrangement.mjs";
import { assertTeamMenu } from "./team-menu.mjs";
import { assertHandshakeTimeout } from "./handshake-timeout.mjs";
import { assertHeadlessLaunchSites, launchBrowser } from "./headless-launch.mjs";
import {
  assertEndpointProbeClosesWithoutResponse,
  assertEndpointProbeStates,
} from "./canvas-endpoint-probes.mjs";
import { assertLineageLifeIsolation } from "./canvas-lineage-life.mjs";
import { assertLineageDrainFailureTeardown } from "./canvas-lineage-teardown.mjs";
import { ownFixture } from "./canvas-lifecycle.mjs";
import { assertBoundaryLoadReading, withLoadReading } from "./canvas-load.mjs";
import { assertSignalExitedChildSkipsTeardown } from "./canvas-signal-teardown.mjs";
import { assertFixtureTeardownReaps } from "./canvas-teardown.mjs";
import {
  assertFullyReapedVendorsStaySilent,
  assertSurvivingVendorsAreReported,
} from "./canvas-vendor-reap.mjs";
import {
  assertSpeechVariants,
  assertSystemSpeech,
  installBrowserSpeech,
  installSocketControl,
} from "./canvas-speech.mjs";
import {
  assertHalfOpenSocketRecovers,
  routeCanvasSocket,
} from "./half-open-socket.mjs";
import {
  assertGenerationOnline,
  assertOfflineObserved,
  assertResidentLossObserved,
  assertResidentRestart,
  assertWaitReportDistinguishesLayout,
  assertWaitReporting,
  buildResident,
  chooseCanvasView,
  configureFixture,
  journalExtent,
  monitorPage,
  reply,
  seedAuthority,
  startResident,
  stopResident,
  CONNECT_MS,
  RECONNECT_MS,
  TRANSITION_MS,
  WAIT_PROBE_MS,
  waitForLocator as waitForElement,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const artifacts = join(root, "artifacts/canvas");
const coverageReceiptFlag = "--spice-coverage-receipt";
const coverageReceipt = "BROWSER-1kKSWhZk browser coverage complete\n";
const coverageArguments = process.argv.slice(2);
let coverageReceiptPath;
if (coverageArguments.length > 0) {
  if (
    coverageArguments.length !== 2 ||
    coverageArguments[0] !== coverageReceiptFlag ||
    !coverageArguments[1] ||
    coverageArguments[1].startsWith("--")
  ) {
    throw new Error(`expected ${coverageReceiptFlag} FILE or no arguments`);
  }
  coverageReceiptPath = coverageArguments[1];
}
const staleEpoch = "018f9999-9999-7444-8888-999999999999";

async function waitAtFirstSpecCaller(page) {
  return waitForPage(page, "first call-site self-check", WAIT_PROBE_MS, () => false)
    .then(() => null, (error) => error);
}

async function waitAtSecondSpecCaller(page) {
  const absent = page.locator('[data-wait-call-site-self-check="absent"]');
  return waitForElement(page, "second call-site self-check", WAIT_PROBE_MS, absent)
    .then(() => null, (error) => error);
}

function reportedWaitSite(failure) {
  assert.ok(failure, "a call-site self-check that cannot be satisfied must fail");
  const site = failure.message.split("\n").find((line) => line.startsWith("  site: "));
  assert.ok(site, "the call-site self-check reports the frame that armed it");
  return site;
}

async function assertDistinctWaitCallSites(page) {
  const first = reportedWaitSite(await waitAtFirstSpecCaller(page));
  const second = reportedWaitSite(await waitAtSecondSpecCaller(page));
  assert.match(
    first,
    /site: at waitAtFirstSpecCaller \(file:.*canvas\.spec\.mjs:\d+:\d+\)/,
    "the first wait reports its named spec caller",
  );
  assert.match(
    second,
    /site: at waitAtSecondSpecCaller \(file:.*canvas\.spec\.mjs:\d+:\d+\)/,
    "the second wait reports its named spec caller",
  );
  assert.notEqual(first, second, "two waits armed at different spec sites remain distinguishable");
}

const nestedQuotePreview = "The author words belong in the composer band.";

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

async function assertPerTeamWatchbands(page, fixture, worktrees) {
  const before = await reply(fixture, ["lane"]);
  const primary = before.body.lanes.find((lane) => lane.worktree === worktrees[0]);
  assert.ok(primary, "the primary team remains seated for watchband proof");
  await reply(fixture, [
    "lane", "add", worktrees[4],
    "--life", "Drive",
    "--vendor", "claude",
  ]);
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Agents");
  const console = page.locator('[data-pane="agent-console"]');
  await console.getByRole("tab", { name: /^Compose/ }).first().click();
  await waitForElement(page, "second team renders after reload", CONNECT_MS,
    console.locator(".team-surface").nth(1));

  const lanes = await reply(fixture, ["lane"]);
  const secondary = lanes.body.lanes.find((lane) => lane.worktree === worktrees[4]);
  assert.ok(secondary, "the second team is independently seated");
  const primarySurface = console.locator(
    `.team-surface[data-lineage-id="${primary.lineage_id}"]`,
  );
  const secondarySurface = console.locator(
    `.team-surface[data-lineage-id="${secondary.lineage_id}"]`,
  );
  const noCardDestination = await primaryDestination(secondarySurface, secondary.lane);
  assert.equal(
    noCardDestination.rendered.length,
    0,
    "the freshly seated secondary agent exercises the primary lineage default before any card",
  );
  const firstSecondaryDirection = "the lineage default precedes this first rendered card";
  await secondarySurface.getByRole("textbox").fill(firstSecondaryDirection);
  await secondarySurface.getByRole("button", { name: "Drive", exact: true }).click();
  await secondarySurface
    .locator(".mosaic-message-card")
    .filter({ hasText: firstSecondaryDirection })
    .waitFor({ timeout: 10_000 });
  assert.equal(await console.locator(".team-surface").count(), 2);
  assert.equal(await console.locator(".team-speech").count(), 2);
  assert.equal(await console.locator(".team-lifetime").count(), 2);
  const weightedWidths = await console.locator(".team-surface").evaluateAll((surfaces) =>
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

  const primaryLife = primarySurface.locator('.team-lifetime input[type="range"]');
  const primarySeatLives = new Map(
    lanes.body.lanes
      .filter((lane) => lane.lineage_id === primary.lineage_id)
      .map((lane) => [lane.lane, lane.life]),
  );
  const initialPrimaryPolicy = lanes.body.subscriptions.find(
    (subscription) => subscription.lineage_id === primary.lineage_id,
  )?.policy;
  assert.ok(initialPrimaryPolicy, "the primary team has one authoritative task policy");
  const initialPrimaryLabel = `${initialPrimaryPolicy[0].toUpperCase()}${initialPrimaryPolicy.slice(1)}`;
  const initialPrimaryIndex = { steer: "0", drive: "1", drain: "2" }[initialPrimaryPolicy];
  await page.evaluate((missingLineage) => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    const send = socket.send.bind(socket);
    let intercepted = false;
    const rewrite = (bytes) => {
      const command = JSON.parse(new TextDecoder().decode(bytes));
      if (!intercepted && command.kind === "lineage::configure") {
        intercepted = true;
        command.body.lineage_id = missingLineage;
        return new TextEncoder().encode(JSON.stringify(command));
      }
      return bytes;
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
  }, staleEpoch);
  await primaryLife.fill("2");
  await waitForPage(
    page,
    "rejected lifetime command settles",
    TRANSITION_MS,
    (lineage) => document
      .querySelector(`.team-surface[data-lineage-id="${lineage}"] .team-composer-controls`)
      ?.dataset.lifetimePending === "false",
    primary.lineage_id,
  );
  await primarySurface
    .getByRole("button", { name: initialPrimaryLabel, exact: true })
    .waitFor();
  assert.equal(await primaryLife.inputValue(), initialPrimaryIndex);
  assert.match(await primarySurface.locator(".team-control-issue").innerText(), /lineage::missing/);

  await primaryLife.fill("2");
  await waitForPage(
    page,
    "accepted lifetime command settles",
    TRANSITION_MS,
    (lineage) => document
      .querySelector(`.team-surface[data-lineage-id="${lineage}"] .team-composer-controls`)
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
  assert.equal(primaryRows.length, 4, "the first team retains all four worktrees");
  assert.equal(primarySubscription?.policy, "drain");
  assert.ok(
    primaryRows.every((lane) => lane.life === primarySeatLives.get(lane.lane)),
    "watchband Drain never changes terminal lane life",
  );
  assert.ok(
    primaryRows.every((lane) => lane.occupant && lane.disposition === "occupied"),
    "watchband Drain changes task policy and never tears down occupants",
  );
  assert.equal(
    await secondarySurface.getByRole("button", { name: "Drive", exact: true }).count(),
    1,
    "the sibling team keeps its own authoritative lifetime",
  );

  const primarySpeech = primarySurface.locator('.team-speech input[type="range"]');
  const secondarySpeech = secondarySurface.locator('.team-speech input[type="range"]');
  await primarySpeech.fill("0");
  await secondarySpeech.fill("2");
  assert.equal(await primarySpeech.inputValue(), "0");
  assert.equal(await secondarySpeech.inputValue(), "2");
  assert.equal(await primarySurface.locator(".team-speech span").innerText(), "quiet");
  assert.equal(await secondarySurface.locator(".team-speech span").innerText(), "narrate");
  await console.getByRole("tab", { name: /^Info/ }).first().click();
  await console.getByRole("tab", { name: /^Compose/ }).first().click();
  assert.equal(await primarySpeech.inputValue(), "0", "Quiet survives a team redraw");
  assert.equal(await secondarySpeech.inputValue(), "2", "Narrate survives a team redraw");
  assert.equal(await primarySurface.locator(".team-speech span").innerText(), "quiet");
  assert.equal(await secondarySurface.locator(".team-speech span").innerText(), "narrate");

  await page.evaluate(() => { window.__speechLog = []; });
  await primarySurface.getByLabel("Speak this message").first().click();
  assert.deepEqual(
    await page.evaluate(() => window.__speechLog),
    [],
    "Quiet suppresses manual speech for only its team",
  );
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
  const primaryNarrator = primaryRows.find((lane) => lane.worktree === worktrees[0]);
  assert.ok(primaryNarrator, "the first team exposes a real narrator worktree");
  const primaryNarrationCards = primarySurface.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${primaryNarrator.lane}"]`,
  );
  const secondaryNarrationCards = secondarySurface.locator(
    `[data-card-kind="agent-message"][data-agent-lane="${secondary.lane}"]`,
  );
  await triggerAgentProse(primaryNarrator.worktree, primaryNarrationCards);
  await waitForPage(page, "the first lineage owns the global narration head", CONNECT_MS,
    () => window.__speechLog.filter((row) => row.action === "speak").length === 1);
  await triggerAgentProse(secondary.worktree, secondaryNarrationCards);
  await triggerAgentProse(secondary.worktree, secondaryNarrationCards);
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

  await assertAgentRenewalRequest(primarySurface, fixture);

  const policyMessage = "drain lifetime is task policy, not occupant teardown";
  await primarySurface.locator(".team-agent").first().getByRole("textbox").fill(policyMessage);
  await primarySurface.getByRole("button", { name: "Drain", exact: true }).click();
  await primarySurface
    .locator(".mosaic-message-card")
    .filter({ hasText: policyMessage })
    .waitFor({ timeout: 10_000 });
  const inbox = await reply(fixture, ["inbox", primaryRows[0].lane]);
  assert.ok(
    inbox.body.lanes[0].pending.some((directive) => directive.text === policyMessage),
    "the Drain-labelled submit follows the configured team policy into real steering",
  );
  assert.equal(await primarySpeech.inputValue(), "0", "Quiet survives a streamed refresh");
  assert.equal(await secondarySpeech.inputValue(), "2", "Narrate survives a streamed refresh");
  assert.equal(await primarySurface.locator(".team-speech span").innerText(), "quiet");
  assert.equal(await secondarySurface.locator(".team-speech span").innerText(), "narrate");

  const renderedPrimaryMembers = await primarySurface.locator(".team-agent").evaluateAll(
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
  await reply(fixture, ["lane", "drain", lifecycleLane.lane]);
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
  await waitForPage(page, "departed author cards retain their lane", TRANSITION_MS,
    ({ lineage, lane }) => {
    const cards = [...document.querySelectorAll(
      `.team-surface[data-lineage-id="${lineage}"] `
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
    "drain",
  );
  await reply(fixture, ["lane", "wake", lifecycleLane.lane]);
  const wokenSnapshot = await reply(fixture, ["lane"]);
  const woken = wokenSnapshot.body.lanes.find((lane) => lane.lane === lifecycleLane.lane);
  assert.ok(woken?.occupant, "watchband Drain leaves the member eligible for a normal wake");
  assert.equal(woken.disposition, "occupied");
  assert.equal(woken.life, primarySeatLives.get(woken.lane));
  await assertPrimaryComposerDragging(
    page,
    fixture,
    console,
    primary.lineage_id,
    secondary.lineage_id,
    join(artifacts, "canvas-primary-composer-dragging.png"),
  );
  await page.screenshot({
    path: join(artifacts, "canvas-light-watchband-teams.png"),
    fullPage: true,
  });
  await page.evaluate(() => {
    window.__speechLog = [];
    window.__speechUtterances = [];
  });
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Agents");
  const reconnected = page.locator('[data-pane="agent-console"]');
  await reconnected.getByRole("tab", { name: /^Compose/ }).first().click();
  const restoredPrimary = reconnected.locator(
    `.team-surface[data-lineage-id="${primary.lineage_id}"]`,
  );
  const restoredSecondary = reconnected.locator(
    `.team-surface[data-lineage-id="${secondary.lineage_id}"]`,
  );
  await restoredPrimary.locator('.team-speech input[type="range"]').fill("2");
  await restoredSecondary.locator('.team-speech input[type="range"]').fill("2");
  await reconnected.getByRole("tab", { name: /^Info/ }).first().click();
  await reconnected.getByRole("tab", { name: /^Compose/ }).first().click();
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

async function captureReferenceViewport(page) {
  await chooseCanvasView(page, "Agents");
  const team = page.locator('[data-pane="agent-console"]');
  await team.getByRole("tab", { name: /^Info/ }).click();
  for (const name of agentNames) {
    await team.getByText(name, { exact: true }).waitFor();
  }
  await team.getByText("0 pending", { exact: true }).waitFor();
  await team.getByText("running", { exact: true }).first().waitFor();
  await team.getByText("running-stale", { exact: true }).first().waitFor();
  await page.screenshot({
    path: join(artifacts, "canvas-reference-agent-details-dark.png"),
    fullPage: false,
  });
  await team.getByRole("tab", { name: /^Compose/ }).click();
  await team.locator(".mosaic-message-card").nth(3).waitFor({ timeout: 10_000 });
  assert.equal(
    await page.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches),
    true,
    "the reference capture explicitly exercises dark mode",
  );
  await assertFourAgentDensity(team);
  assert.equal(
    await team.locator(".team-surface").getAttribute("data-lineage-weight"),
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
  assert.equal(pixels.background, "rgb(11, 15, 12)", "the captured surface uses the dark palette");
  await captureReferenceControls(
    page,
    team,
    join(artifacts, "canvas-reference-four-agent-dark.png"),
  );
}

async function assertAuthoritySurface(page, seeded) {
  await chooseCanvasView(page, "Flow");
  const currentWork = page.locator(".section").filter({ hasText: "Current work" });
  await currentWork.getByText("Browser fixture task", { exact: true }).waitFor();
  await currentWork.getByText("Continuation witness", { exact: true }).waitFor();

  await chooseCanvasView(page, "Magnitude");
  await page.getByRole("heading", { name: "Consent budgets", exact: true }).waitFor();
  await page.getByText("browser", { exact: true }).waitFor();
  assert.match(await page.locator(".section").filter({ hasText: "Consent budgets" }).innerText(), /2\s+2/);

  await chooseCanvasView(page, "Topology");
  const topology = page.locator("main.content");
  await topology.getByText("Resident topology", { exact: true }).waitFor();
  await topology.getByText("claude", { exact: true }).first().waitFor();
  assert.match(await topology.innerText(), new RegExp(seeded.lane.slice(0, 8)));

  await chooseCanvasView(page, "Chronology");
  await page.getByText("browser direction survives streaming", { exact: true }).waitFor();
  const trust = page.locator('[data-pane="trust"]');
  await trust.waitFor();
  const trustText = await trust.innerText();
  assert.match(trustText, /trust::pended/);
  assert.match(trustText, /trust::granted/);
  assert.match(trustText, /granted/);
}

async function assertMosaicComposer(page, fixture, seeded) {
  await chooseCanvasView(page, "Agents");
  const team = page.locator('[data-pane="agent-console"]');
  await team.getByRole("tab", { name: /^Compose/ }).click();
  const lane = page.locator(`.team-agent[data-agent-lane="${seeded.lane}"]`);
  const activity = team.locator(`.mosaic-message-card[data-agent-lane="${seeded.lane}"]`);
  const quoteLane = page.locator(`.team-agent[data-agent-lane="${seeded.quoteLane}"]`);
  const quoteActivity = team.locator(
    `.mosaic-message-card[data-agent-lane="${seeded.quoteLane}"]`,
  );
  await activity.filter({ hasText: "browser direction survives streaming" }).waitFor();

  await assertComposerShards({
    page,
    team,
    quoteLane,
    quoteActivity,
    seeded,
    nestedQuotePreview,
    nestedQuoteMessage,
    entirelyQuotedMessage,
    latestPrimaryProse: seeded.quoteProse,
  });

  await activity
    .filter({ hasText: "browser direction survives streaming" })
    .first()
    .getByRole("button", { name: "Quote this message" })
    .click();
  const composer = lane.locator(".composer-shard--primary");
  const quotedComposer = lane.locator(".composer-shard--quote");
  await quotedComposer.waitFor();
  // Quoting still carries its context, but on the shard that owns it rather
  // than inside a rendered copy of the draft.
  assert.equal(
    await quotedComposer.getAttribute("title"),
    "browser direction survives streaming",
    "the quoted message stays reachable from the shard it minted",
  );
  await assertCompleteComposerSubmission({
    page,
    fixture,
    lane,
    activity,
    quoteLane,
    quoteActivity,
    seeded,
    composer,
    quotedComposer,
    artifactPath: join(artifacts, "canvas-light-mosaic-composer.png"),
  });

  await lane.evaluate((target) => {
    const transfer = new DataTransfer();
    transfer.setData("text/plain", "dragged card direction");
    target.dispatchEvent(new DragEvent("drop", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
    }));
  });
  await activity.filter({ hasText: "dragged card direction" }).waitFor({
    timeout: 10_000,
  });
  const dropped = await reply(fixture, ["inbox", seeded.lane]);
  assert.ok(
    dropped.body.lanes[0].pending.some(
      (directive) => directive.text === "> dragged card direction",
    ),
    "dropping text on a lane sends quoted steering",
  );
}

async function assertPackedHistory(page, fixture, extent, linked = false) {
  await chooseCanvasView(page, "Chronology");
  const journal = page.locator('[data-pane="commands"]');
  const facts = journal.locator(".event strong");
  await facts.nth(extent.facts - 1).waitFor();
  // One snapshot, and every count below is derived from it. Counting the rows
  // and reading their sequences as two separate calls would let the resident
  // admit a fact between them, and the assertions would then describe a page
  // state that never existed at any instant.
  const sequences = await facts.evaluateAll((nodes) => nodes.map((node) => {
    const value = Number(node.textContent?.split("/")[0].trim());
    if (!Number.isInteger(value)) throw new Error(`invalid rendered sequence: ${node.textContent}`);
    return value;
  }));
  // Sampled after the snapshot, so it closes the other side of the sandwich.
  // The journal only grows, so a fact the page rendered legitimately was
  // admitted before the snapshot and is therefore counted here too.
  const settled = await journalExtent(fixture);
  // A floor, not an equality. `extent` was sampled before this read, and the
  // resident admits facts of its own while the suite runs -- accounting a lane's
  // transcript is one of them -- so a page that is correctly caught up can hold
  // strictly more than the sample counted. What it can never do is hold fewer.
  assert.ok(
    sequences.length >= extent.facts,
    `all ${extent.facts} admitted facts render (rendered ${sequences.length})`,
  );
  // And a ceiling, which is the half a floor alone gives up. A page inventing a
  // fact past the horizon renders a sequence no one admitted, and that tail
  // stays distinct and stays contiguous, so neither proof below would see it.
  // Only a count the journal itself bounds can.
  assert.ok(
    sequences.length <= settled.facts,
    `no fact renders that the journal never admitted (rendered ${sequences.length} > ${settled.facts})`,
  );

  const provenance = await journal.locator('[data-subject="facts"]').allTextContents();
  assert.ok(provenance.length > 1, "two-row packing produces multiple fact pages");
  assert.match(provenance[0], /omitted \/ continue/, "first page exposes overflow continuation");
  assert.doesNotMatch(provenance.at(-1), /omitted \/ continue/, "final page closes continuation");
  const horizons = await journal
    .locator('[data-subject="facts"] span')
    .evaluateAll((nodes) => nodes.map((node) => node.textContent?.trim()).filter((text) => text?.startsWith("horizon ")));
  const horizonValues = horizons.map((text) => Number(text.split(" ")[1]));
  // A floor for the same reason the fact count is one: the cursor the caller
  // sampled has already been passed if the resident admitted anything since.
  // Reaching it is the claim; standing exactly on it was never the claim.
  assert.ok(
    horizonValues.at(-1) >= extent.cursor,
    `latest page reaches the current horizon (${horizonValues.at(-1)} < ${extent.cursor})`,
  );
  assert.ok(
    horizonValues.every((value, index) => index === 0 || value >= horizonValues[index - 1]),
    "continued and later live pages retain monotone packed horizons",
  );

  // This is the duplicate-application proof, and it needs no extent at all:
  // applying one fact twice renders its sequence twice, so distinct-equals-total
  // catches it whether or not the caller's sample is still current. The previous
  // form compared the distinct count against `extent.facts`, which only held
  // while the journal happened to be quiescent and was never the thing that made
  // a repeat detectable.
  assert.equal(
    new Set(sequences).size,
    sequences.length,
    "continuation fabricates no duplicate fact",
  );
  // Contiguous from one, so nothing between the origin and the rendered horizon
  // is missing either. A dropped fact fails here, a repeated one fails above,
  // and an invented one fails against the ceiling -- it is contiguous and
  // distinct, so those two are the wrong instrument for it.
  assert.deepEqual(
    [...sequences].sort((left, right) => left - right),
    Array.from({ length: sequences.length }, (_, index) => index + 1),
    "continuation loses no fact before the packed horizon",
  );

  await chooseCanvasView(page, "Topology");
  const thread = page.locator('[data-pane="lineage"]');
  const minimumRows = linked ? 2 : 1;
  const threadRowsLocator = thread.locator("tbody tr");
  await threadRowsLocator.nth(minimumRows - 1).waitFor();
  assert.ok(
    await threadRowsLocator.count() >= minimumRows,
    linked ? "multi-link causal thread renders" : "terminal causal thread renders",
  );
  // The terminal label is the tail of a walk built one admitted frame at a time,
  // not a value the row count settles. `observe_stream` inserts one CausalLink
  // per admitted frame and the walk stops at `ChainEnd::Dangling(position)` the
  // moment a position is missing (app/src/state.rs:351), which renders as
  // `dangling at N` (app/src/browser/view.rs:588). Two rows can be on screen
  // while headers below them are still in flight, so the row wait above leaves
  // this field mid-flight: it was observed once reading `dangling at 14` on a
  // tree that re-ran green, and CAUSAL_HEADERS is 1024, so position 14 was never
  // eviction.
  //
  // Keep the wait rejection: a walk that genuinely remains dangling or cyclic
  // is a defect, and neither terminal state is an acceptable substitute for a
  // root. The rejection carries the label it found instead, because the position
  // is the diagnosis -- reading 14 against CAUSAL_HEADERS of 1024 is what ruled
  // eviction out above, where a bare timeout carries no position at all. That
  // reading is the whole observation: a second read taken after the wait could
  // only report a later frame than the one that failed, so there is none.
  //
  // The pane carries exactly one count, and the predicate requires that rather
  // than assuming it: a second one would have `querySelector` silently answer
  // for the wrong node, and the strict locator handed to `observed` reports that
  // by name instead.
  const terminal = thread.locator(".section-count");
  await waitForPage(page, "the causal walk reaches root", TRANSITION_MS, () => {
    const counts = document.querySelectorAll('[data-pane="lineage"] .section-count');
    return counts.length === 1 && counts[0].textContent === "root";
  }, undefined, { observed: () => terminal.innerText() });
  const threadRows = await thread.locator("tbody tr").allTextContents();
  if (linked) {
    assert.ok(threadRows.some((row) => !row.endsWith("root")), "causal walk includes a linked cause");
  }

  await chooseCanvasView(page, "Chronology");
  await page.screenshot({ path: join(artifacts, "canvas-chronology.png"), fullPage: true });
}

async function assertNoCredentialOrLandingGap(page, capabilities) {
  const content = await page.content();
  assert.doesNotMatch(content, /release::not-landed/);
  for (const capability of capabilities) {
    assert.ok(!content.includes(capability), "rendered DOM does not expose a capability");
  }
  const urls = await page.evaluate(() => performance.getEntriesByType("resource").map((entry) => entry.name));
  for (const capability of capabilities) {
    assert.ok(urls.every((url) => !url.includes(capability)), "resource URLs do not expose a capability");
  }
}

async function assertStaleGeneration(browser, endpoint, currentEpoch) {
  const context = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  const page = await context.newPage();
  const monitor = monitorPage(page);
  await page.route("**/bootstrap", async (route) => {
    const response = await route.fetch();
    const bootstrap = await response.json();
    assert.equal(bootstrap.epoch, currentEpoch);
    bootstrap.epoch = staleEpoch;
    await route.fulfill({
      response,
      contentType: "application/json; charset=utf-8",
      body: JSON.stringify(bootstrap),
    });
  });
  await page.goto(endpoint, { waitUntil: "domcontentloaded" });
  await chooseCanvasView(page, "Chronology");
  await assertOfflineObserved(page, "refusing a stale generation");
  const fault = page.locator(".event.fault");
  await fault.waitFor();
  assert.match(await fault.innerText(), /Canvas connection reset/);
  assert.match(await fault.innerText(), /accepted|frame|epoch|socket/i);
  monitor.assertClean();
  await context.close();
}

/// Drive the half-open case on a context of its own.
///
/// The route has to be in place before the first navigation, and every other
/// page in this suite has already connected by the time it gets here. Reusing
/// one would mean routing a socket that was opened before the route existed.
async function assertHalfOpenSocket(browser, endpoint, epoch, fixture, seeded) {
  const context = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  const page = await context.newPage();
  const monitor = monitorPage(page);
  const route = await routeCanvasSocket(page);
  await page.goto(endpoint, { waitUntil: "domcontentloaded" });
  let advanced;
  await assertHalfOpenSocketRecovers(page, monitor, route, epoch, async () => {
    await reply(fixture, ["task", "note", seeded.task, "written while the socket was half open"]);
    advanced = await journalExtent(fixture);
  });
  // Taken during the outage, so it counts a fact the page could not have had
  // and the resume cannot pass vacuously. It is a floor and not a total: the
  // resident keeps admitting facts of its own while this runs, so an extent
  // sampled here is already behind by the time the page is read.
  // `assertPackedHistory` proves duplicate application from the rendered
  // sequences themselves rather than from that count.
  await assertPackedHistory(page, fixture, advanced);
  monitor.assertClean();
  await context.close();
}

async function assertSameGenerationResume(
  page,
  monitor,
  fixture,
  seeded,
  epoch,
  beforeExtent,
) {
  await chooseCanvasView(page, "Chronology");
  const before = await page.locator('[data-pane="commands"] .event strong').allTextContents();
  monitor.expectNetworkInterruption(true);
  await page.evaluate(() => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    socket.close(4000, "fixture reconnect");
  });
  await assertOfflineObserved(page, "the fixture closes its socket");
  assert.match(await page.locator(".status-copy").innerText(), /Reconnecting/);
  await chooseCanvasView(page, "Chronology");
  await page.locator(".event.fault").waitFor();

  await reply(fixture, ["task", "note", seeded.task, "written during browser disconnect"]);
  const advanced = await journalExtent(fixture);
  assert.ok(advanced.facts > beforeExtent.facts, "offline command advances resident authority");

  await assertGenerationOnline(page, epoch, "the socket is reopened", RECONNECT_MS);
  monitor.expectNetworkInterruption(false);
  await assertPackedHistory(page, fixture, advanced);
  const after = await page.locator('[data-pane="commands"] .event strong').allTextContents();
  assert.ok(after.length > before.length, "same-generation resume appends missed facts");
}

assertBoundaryLoadReading();
// The launch-site guard interrogates this exact executable, so building is a
// precondition rather than something an agent must remember after source gains
// a no-pop gate. No fixture process can start before both steps pass.
await buildResident();
await assertHeadlessLaunchSites();
await assertEndpointProbeClosesWithoutResponse();
await assertEndpointProbeStates();
await assertSignalExitedChildSkipsTeardown();
await assertLineageDrainFailureTeardown();
await assertSurvivingVendorsAreReported();
await assertFullyReapedVendorsStaySilent();
await assertFixtureTeardownReaps();
const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-browser-")));
const linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-lanes-")));
// Ownership is taken before the fixture is configured rather than inside the
// try below, because configuring it is already enough work to be interrupted
// during, and an interruption there used to leave both directories behind.
const owner = ownFixture({ fixture, linkedRoot });
await mkdir(artifacts, { recursive: true });

let resident;
let browser;
try {
  // Configuring the fixture is inside the guarded region, not before it.
  // Outside, a failure here -- a bad stage copy, a git worktree that will not
  // add -- left both temporary directories behind with nothing to remove them.
  const worktrees = await configureFixture(fixture, linkedRoot, 5, { noCardMember: 5 });
  resident = owner.ownResident(await startResident(fixture));
  const seeded = await seedAuthority(fixture);
  browser = await launchBrowser();
  await assertHandshakeTimeout(browser, resident.discovery.endpoint);
  await assertLineageLifeIsolation(browser);

  const light = await browser.newContext({
    colorScheme: "light",
    viewport: { width: 1440, height: 900 },
  });
  const activityReplay = await installActivityReplay(light);
  await installBrowserSpeech(light);
  await installSocketControl(light);
  const lightPage = await light.newPage();
  const lightMonitor = monitorPage(lightPage);
  lightPage.on("console", (message) => process.stderr.write(`browser console: ${message.text()}\n`));
  await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await assertCanvas(lightPage, "light");
  await assertDistinctWaitCallSites(lightPage);
  const hiddenStatusReport = await assertWaitReporting(lightPage, resident);
  Object.assign(
    seeded,
    await assertAgentConversation(lightPage, fixture, worktrees, activityReplay, artifacts),
  );
  await assertTeamMenu(lightPage, fixture);
  const reference = await browser.newContext({
    colorScheme: "dark",
    viewport: { width: 1000, height: 568 },
  });
  await installBrowserSpeech(reference);
  const referencePage = await reference.newPage();
  const referenceMonitor = monitorPage(referencePage);
  await referencePage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await captureReferenceViewport(referencePage);
  referenceMonitor.assertClean();
  await reference.close();
  seeded.extent = await journalExtent(fixture);
  await assertAuthoritySurface(lightPage, seeded);
  await assertMosaicComposer(lightPage, fixture, seeded);
  await assertComposerRecovery(lightPage, fixture, seeded, lightMonitor);
  await assertPackedHistory(lightPage, fixture, await journalExtent(fixture), true);
  await assertNoCredentialOrLandingGap(lightPage, [resident.discovery.capability]);
  await assertSystemSpeech(lightPage);
  await chooseCanvasView(lightPage, "Flow");
  await lightPage.getByLabel("Task").fill(seeded.task);
  await lightPage.getByLabel("Observation").fill("browser mutation reached the resident");
  await lightPage.getByRole("button", { name: "Add note" }).click();
  await chooseCanvasView(lightPage, "Chronology");
  await lightPage.getByText(/Note appended to/).waitFor({ timeout: 10_000 });
  await chooseCanvasView(lightPage, "Flow");
  await lightPage.getByLabel("Task").fill("not a handle");
  await lightPage.getByLabel("Observation").fill("the callback must explain this refusal");
  const addNote = lightPage.getByRole("button", { name: "Add note" });
  assert.equal(await addNote.isEnabled(), true, "nonempty malformed input reaches the callback");
  await addNote.click();
  const noteRefusal = lightPage.getByRole("alert");
  await noteRefusal.waitFor();
  assert.match(await noteRefusal.innerText(), /Task must be .*both parts alphanumeric/);
  assert.match(await noteRefusal.innerText(), /"not a handle"/);
  const existingTasks = await lightPage.locator("#note-tasks option").evaluateAll(
    (options) => options.map((option) => option.value),
  );
  const absentTask = ["SPICE-aBcD", "SPICE-zYxW"].find(
    (candidate) => !existingTasks.includes(candidate),
  );
  assert.ok(absentTask, "an absent canonical task handle is available");
  await lightPage.getByLabel("Task").fill(absentTask);
  await lightPage.getByLabel("Observation").fill("this task does not exist");
  await addNote.click();
  await noteRefusal.waitFor({ state: "detached" });
  await chooseCanvasView(lightPage, "Chronology");
  await lightPage.locator(".event.refusal").waitFor({ timeout: 10_000 });
  const beforeReconnect = await journalExtent(fixture);
  await assertPackedHistory(lightPage, fixture, beforeReconnect);

  const dark = await browser.newContext({
    colorScheme: "dark",
    viewport: { width: 390, height: 844 },
  });
  await installBrowserSpeech(dark);
  const darkPage = await dark.newPage();
  const darkMonitor = monitorPage(darkPage);
  await darkPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await assertCanvas(darkPage, "dark-narrow");
  await assertMosaicLattice(darkPage, false);
  await assertAuthoritySurface(darkPage, seeded);
  await assertNoCredentialOrLandingGap(darkPage, [resident.discovery.capability]);
  darkMonitor.assertClean();
  await assertMotionPreference(browser, resident.discovery.endpoint, artifacts);
  await assertSpeechVariants(browser, resident.discovery.endpoint);
  await assertStaleGeneration(browser, resident.discovery.endpoint, resident.discovery.epoch);

  await assertSameGenerationResume(
    lightPage,
    lightMonitor,
    fixture,
    seeded,
    resident.discovery.epoch,
    beforeReconnect,
  );
  await assertHalfOpenSocket(
    browser,
    resident.discovery.endpoint,
    resident.discovery.epoch,
    fixture,
    seeded,
  );
  // Its own context for the same reason the half-open case takes one: the route
  // that makes the resident go quiet has to exist before the socket opens, and
  // every page above this line connected long ago.
  await assertAgesTickWhileTheResidentIsSilent(
    browser,
    resident.discovery.endpoint,
    fixture,
    seeded.lane,
  );
  const configuredPolicy = await assertPerTeamWatchbands(lightPage, fixture, worktrees);
  await assertTeamArrangement(lightPage, fixture, artifacts);
  lightMonitor.assertClean();
  await dark.close();

  const liveTeam = await reply(fixture, ["lane"]);
  for (const agent of liveTeam.body.lanes) {
    await reply(fixture, ["lane", "drain", agent.lane]);
  }
  const firstEpoch = resident.discovery.epoch;
  const firstCapability = resident.discovery.capability;
  lightMonitor.expectNetworkInterruption(true);
  await chooseCanvasView(lightPage, "Chronology");
  await assertWaitReportDistinguishesLayout(lightPage, resident, hiddenStatusReport);
  await stopResident(fixture, resident);
  await assertResidentLossObserved(lightPage, resident);
  const restart = { epoch: firstEpoch, capability: firstCapability, policy: configuredPolicy };
  resident = owner.ownResident(await assertResidentRestart(fixture, lightPage, restart));
  lightMonitor.expectNetworkInterruption(false);
  await assertCanvas(lightPage, "light-restarted");
  await chooseCanvasView(lightPage, "Chronology");
  assert.equal(
    await lightPage.getByText(/Note appended to/).count(),
    0,
    "new generation clears prior command reply state",
  );
  const restartedExtent = await journalExtent(fixture);
  await assertPackedHistory(lightPage, fixture, restartedExtent);
  await assertAuthoritySurface(lightPage, seeded);
  await assertNoCredentialOrLandingGap(lightPage, [firstCapability, resident.discovery.capability]);
  lightMonitor.assertClean();

  await light.close();
} catch (error) {
  throw withLoadReading(error);
} finally {
  if (browser) await browser.close();
  await owner.cleanup();
}
if (coverageReceiptPath) await writeFile(coverageReceiptPath, coverageReceipt);
