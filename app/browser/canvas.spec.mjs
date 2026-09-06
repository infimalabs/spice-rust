import assert from "node:assert/strict";
import { mkdtemp, mkdir, realpath, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { installActivityReplay } from "./activity-replay.mjs";
import { assertAgesTickWhileTheResidentIsSilent } from "./age-ticker.mjs";
import {
  assertAgentConversation, assertMosaicLattice, entirelyQuotedMessage,
  longAgentName, longWorktreeBasename, nestedQuoteMessage, nestedQuotePreview,
  seatAgentTeam, seatLeaseAgent,
} from "./agent-conversation.mjs";
import {
  assertCompactionBoundaryPersistence,
  assertRetainedMosaicIdentity,
  assertStagedMosaicHydration,
  installPersistedMosaicImageRoutes,
  rearmRetainedMosaicIdentity,
  waitForMosaicReady,
} from "./agent-conversation-mosaic.mjs";
import {
  assertCompactionDivider,
  assertNoUnavailableToolTerminal,
  waitForCompactionReady,
} from "./compaction-boundary-reference.mjs";
import { assertCompactionBoundary } from "./mosaic-transitions.mjs";
import {
  assertFirefoxRestoredMosaic,
  captureFirefoxAgentPageSpacing,
} from "./firefox-restored-mosaic.mjs";
import { assertCandidateMeasurementPaintEngineInventory } from "./mosaic-candidate-paint.mjs";
import {
  assertComposerPointerSelection,
  assertComposerShards,
  assertComposerTypingDoesNotRemeasureActivity,
} from "./composer-shards.mjs";
import { assertCompleteComposerSubmission } from "./composer-submission.mjs";
import { assertComposerRecovery } from "./composer-recovery.mjs";
import { assertVacantWakeSubmission } from "./vacant-wake-submission.mjs";
import {
  assertAgentSpacingRejectsFallbackOnlyMarkup,
  assertCanvas,
  captureReportedAgentPage,
} from "./canvas-readiness.mjs";
import { assertCanvasProtocolRefresh } from "./canvas-compatibility.mjs";
import { assertAuthoritySurface } from "./canvas-authority.mjs";
import {
  assertPerTeamWatchbands,
  captureReferenceViewport,
} from "./canvas-watchbands.mjs";
import { assertMotionPreference } from "./mosaic-motion.mjs";
import { assertIntegratedMosaicProof } from "./mosaic-integrated-proof.mjs";
import { assertTeamArrangement } from "./team-arrangement.mjs";
import { assertTeamMenu } from "./team-menu.mjs";
import { assertTaskNotes } from "./task-notes.mjs";
import { assertHandshakeTimeout } from "./handshake-timeout.mjs";
import { assertHeadlessLaunchSites, launchBrowser } from "./headless-launch.mjs";
import {
  assertEndpointProbeClosesWithoutResponse,
  assertEndpointProbeStates,
} from "./canvas-endpoint-probes.mjs";
import {
  assertLineageLifeIsolation,
  assertSevenMemberComposerStrip,
} from "./canvas-lineage-life.mjs";
import { assertLineageDrainFailureTeardown } from "./canvas-lineage-teardown.mjs";
import { assertTaskChipTypographyMatrix } from "./control-reference.mjs";
import { assertLiveThreadCoalescing } from "./live-thread-coalescing.mjs";
import { ownFixture, recordedVendorPids } from "./canvas-lifecycle.mjs";
import { assertBoundaryLoadReading, withLoadReading } from "./canvas-load.mjs";
import { auditOwnership } from "./canvas-process-ownership.mjs";
import { descendantPids, emitShardEvent, startShardWorker } from "./canvas-shards.mjs";
import { assertSignalExitedChildSkipsTeardown } from "./canvas-signal-teardown.mjs";
import { assertFixtureTeardownReaps } from "./canvas-teardown.mjs";
import {
  assertFullyReapedVendorsStaySilent,
  assertSurvivingVendorsAreReported,
} from "./canvas-vendor-reap.mjs";
import {
  assertAutomaticSpeechEngines,
  assertSpeechVariants,
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
  appendSubstantialPrimaryHistory,
  buildResident,
  chooseCanvasView,
  configureFixture,
  assertHeldTaskCount,
  journalExtent,
  monitorPage,
  observedBoundedWait,
  reply,
  replyAsLane,
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
const staleEpoch = "018f9999-9999-7444-8888-999999999999";
const RETAINED_HISTORY_BYTES = 64 * 1_024 * 1_024;
const RESTART_READY_MS = 10_000;
const RESTART_CPU_MS = 10_000;
const RESTART_RSS_BYTES = 512 * 1_024 * 1_024;
const SHUTDOWN_MS = 5_000;

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

async function assertMosaicComposer(page, fixture, seeded, activityReplay) {
  await chooseCanvasView(page, "Lanes");
  const team = page.locator('[data-pane="agent-console"]');
  await team.getByRole("tab", { name: /^Lanes/ }).click();
  const lane = page.locator(`.team-agent[data-agent-lane="${seeded.lane}"]`);
  const activity = team.locator(`.mosaic-message-card[data-agent-lane="${seeded.lane}"]`);
  const quoteLane = page.locator(`.team-agent[data-agent-lane="${seeded.quoteLane}"]`);
  const quoteActivity = team.locator(
    `.mosaic-message-card[data-agent-lane="${seeded.quoteLane}"]`,
  );
  await assertComposerShards({
    page,
    fixture,
    team,
    quoteLane,
    quoteActivity,
    seeded,
    nestedQuotePreview,
    nestedQuoteMessage,
    entirelyQuotedMessage,
    latestPrimaryProse: seeded.quoteProse,
    activityReplay,
    artifacts,
  });

  const originalViewport = page.viewportSize();
  assert.ok(originalViewport, "the integrated Mosaic proof has a viewport to restore");
  try {
    await page.setViewportSize({ width: 2018, height: 1728 });
    await waitForMosaicReady(page);
    await assertStagedMosaicHydration(page, team, activityReplay, (hydration) => (
      assertIntegratedMosaicProof(
        page,
        team,
        artifacts,
        [longAgentName, longWorktreeBasename],
        hydration,
      )
    ));
  } finally {
    await page.setViewportSize(originalViewport);
    await waitForMosaicReady(page);
  }
  const restoredMosaic = await rearmRetainedMosaicIdentity(
    team.locator(".lane-mosaic-plane"),
    seeded.postHiddenMosaic,
    "staged replay and restored geometry",
  );

  const quoteSourceCandidate = team.locator(
    `.mosaic-message-card[data-card-kind="agent-message"][data-agent-lane="${seeded.lane}"]`,
  ).first();
  await quoteSourceCandidate.waitFor();
  const quoteSourceKey = await quoteSourceCandidate.getAttribute("data-mosaic-card-key");
  assert.ok(quoteSourceKey, "the quote source has one stable Mosaic identity");
  const quoteSource = team.locator(`[data-mosaic-card-key="${quoteSourceKey}"]`);

  const compactionBoundary = await assertCompactionBoundary(
    page, fixture, team, seeded, artifacts, restoredMosaic,
  );
  const unnamedCompaction = await activityReplay.injectCompaction(page, seeded.quoteLane);
  const unnamedDivider = team.locator(
    `[data-mosaic-card-key="${unnamedCompaction.key}"]`,
  );
  const unnamedAfter = await activityReplay.injectAgentProse(
    page,
    seeded.quoteLane,
    "The unnamed lane continues after compaction.",
  );
  await team.locator(`[data-mosaic-card-key="${unnamedAfter.key}"]`).waitFor();
  await waitForCompactionReady(page, unnamedDivider, "unnamed compaction boundary");
  await assertCompactionDivider(
    unnamedDivider,
    "unnamed compaction boundary",
    longWorktreeBasename,
  );
  await assertNoUnavailableToolTerminal(
    unnamedDivider,
    "unnamed compaction boundary",
    longWorktreeBasename,
  );

  const quoteSourceBody = (await quoteSource.locator(".message-body").innerText()).trim();
  await quoteSource.getByRole("button", { name: "Quote this message" }).click();
  const composer = lane.locator(".composer-shard--primary");
  const quotedComposer = lane.locator(".composer-shard--quote");
  const siblingComposer = quoteLane.locator(".composer-shard--primary");
  await quotedComposer.waitFor();
  // Quoting still carries its context, but on the shard that owns it rather
  // than inside a rendered copy of the draft.
  const quotedTitle = await quotedComposer.getAttribute("title");
  assert.ok(quotedTitle, "the quoted message remains attached to the shard it minted");
  assert.equal(
    quotedTitle.replaceAll("`", "").replace(/\s+/g, " ").trim(),
    quoteSourceBody.replace(/\s+/g, " ").trim(),
    "the quote shard preserves the selected assistant card's content",
  );
  await assertComposerTypingDoesNotRemeasureActivity(
    page,
    lane,
    [composer, quotedComposer, siblingComposer],
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
    quoteSource,
    quoteSourceTitle: quotedTitle,
    artifactPath: join(artifacts, "canvas-light-mosaic-composer.png"),
  });
  await assertCompactionBoundaryPersistence(page, seeded, compactionBoundary);

  await lane.evaluate((target) => {
    const transfer = new DataTransfer();
    transfer.setData("text/plain", "dragged card direction");
    target.dispatchEvent(new DragEvent("drop", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
    }));
  });
  const dropped = await reply(fixture, ["inbox", seeded.lane]);
  assert.ok(
    dropped.body.lanes[0].pending.some(
      (directive) => directive.text === "> dragged card direction",
    ),
    "dropping text on a lane sends quoted steering",
  );
  assert.equal(await activity.filter({ hasText: "dragged card direction" }).count(), 0);
  assert.equal(await team.locator('[data-card-kind="operator-message"]').count(), 0);
  await activityReplay.restoreProseSnapshot(page);
}

async function renderedFactSequences(page) {
  return page.locator('[data-pane="commands"] .event strong').evaluateAll((nodes) => nodes.map((node) => {
    const value = Number(node.textContent?.split("/")[0].trim());
    if (!Number.isInteger(value)) throw new Error(`invalid rendered sequence: ${node.textContent}`);
    return value;
  }));
}

/// Assert the bounded evidence an operator legitimately retains across reconnects.
///
/// Operator subscriptions cross reconnect at the current live barrier. Missing
/// history is therefore expected; retained rows must instead remain authentic,
/// unique, and ordered under monotone page horizons.
async function assertRetainedHistory(page, fixture) {
  await chooseCanvasView(page, "Chronology");
  const journal = page.locator('[data-pane="commands"]');
  const sequences = await renderedFactSequences(page);
  const provenance = await journal.locator('[data-subject="facts"]').allTextContents();
  const horizons = await journal
    .locator('[data-subject="facts"] span')
    .evaluateAll((nodes) => nodes.map((node) => node.textContent?.trim()).filter((text) => text?.startsWith("horizon ")));
  const horizonValues = horizons.map((text) => Number(text.split(" ")[1]));
  const authority = await journalExtent(fixture);
  assert.ok(sequences.length > 0, "the operator retains admitted live fact evidence");
  assert.equal(provenance.length, horizonValues.length, "every retained page names its horizon");
  assert.ok(
    horizonValues.every(Number.isInteger),
    "every retained page horizon is a journal sequence",
  );
  assert.ok(
    horizonValues.every((value, index) => index === 0 || value >= horizonValues[index - 1]),
    "retained and later live pages keep monotone horizons",
  );
  assert.ok(
    sequences.every((seq) => seq <= authority.cursor)
      && horizonValues.every((horizon) => horizon <= authority.cursor),
    "retained facts and horizons do not outrun journal authority",
  );
  assert.equal(
    new Set(sequences).size,
    sequences.length,
    "reconnect and later delivery retain no duplicate fact",
  );

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
  await page.clock.install();
  const monitor = monitorPage(page);
  const route = await routeCanvasSocket(page);
  await page.goto(endpoint, { waitUntil: "domcontentloaded" });
  // A fresh operator begins at the current live barrier, so the authority
  // extent -- not a historical page drain -- brackets the gap induced below.
  await chooseCanvasView(page, "Chronology");
  await assertGenerationOnline(page, epoch, "the half-open page connects", CONNECT_MS);
  const beforeExtent = await journalExtent(fixture);
  let advanced;
  await assertHalfOpenSocketRecovers(page, monitor, route, epoch, async () => {
    await reply(fixture, ["task", "note", seeded.task, "written while the socket was half open"]);
    advanced = await journalExtent(fixture);
  });
  await assertRecoveryBeginsLive(
    page,
    fixture,
    seeded,
    beforeExtent,
    advanced,
    "written after half-open recovery",
  );
  monitor.assertClean();
  await context.close();
}

/// Prove an operator reconnect crosses a live barrier instead of replaying.
///
/// `beforeExtent..missedExtent` was admitted while the browser could receive
/// nothing. Its absence proves the replacement subscription did not drain the
/// gap; the later note proves the same subscription is live rather than stuck.
async function assertRecoveryBeginsLive(
  page,
  fixture,
  seeded,
  beforeExtent,
  missedExtent,
  liveNote,
) {
  await chooseCanvasView(page, "Chronology");
  assert.ok(
    missedExtent.cursor > beforeExtent.cursor,
    "authority crossed the browser's cursor while inbound delivery was absent",
  );
  const missed = (seq) => seq > beforeExtent.cursor && seq <= missedExtent.cursor;
  assert.ok(
    (await renderedFactSequences(page)).every((seq) => !missed(seq)),
    "operator recovery skips facts admitted while inbound delivery was absent",
  );

  await assertLiveNoteArrives(page, fixture, seeded, missedExtent.cursor, liveNote);
  const recovered = await renderedFactSequences(page);
  assert.ok(
    recovered.every((seq) => !missed(seq)),
    "later live delivery does not backfill the operator's missed range",
  );
}

async function assertLiveNoteArrives(page, fixture, seeded, afterCursor, liveNote) {
  await reply(fixture, ["task", "note", seeded.task, liveNote]);
  const liveExtent = await journalExtent(fixture);
  assert.ok(liveExtent.cursor > afterCursor, "live note advances authority");
  const liveRow = page
    .locator('[data-pane="commands"] .event strong')
    .filter({ hasText: new RegExp(`^${liveExtent.cursor} / `) });
  await liveRow.waitFor({ timeout: RECONNECT_MS });
  assert.equal(
    await liveRow.count(),
    1,
    "the active operator subscription delivers a new live fact exactly once",
  );
  return liveExtent;
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
  const before = await renderedFactSequences(page);
  monitor.expectNetworkInterruption(true);
  await page.evaluate(() => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    socket.close(4000, "fixture reconnect");
  });
  await assertOfflineObserved(page, "the fixture closes its socket");
  assert.match(await page.locator(".status-copy").innerText(), /Reconnecting/);
  await chooseCanvasView(page, "Chronology");
  await page
    .locator(".event.fault")
    .filter({ hasText: "Canvas connection reset" })
    .waitFor();

  await reply(fixture, ["task", "note", seeded.task, "written during browser disconnect"]);
  const advanced = await journalExtent(fixture);
  assert.ok(advanced.facts > beforeExtent.facts, "offline command advances resident authority");

  await assertGenerationOnline(page, epoch, "the socket is reopened", RECONNECT_MS);
  await observedBoundedWait(
    page,
    "the replacement operator subscription is live",
    CONNECT_MS,
    () => page.evaluate(() => {
      const socket = window.__canvasSockets.at(-1);
      if (!socket) throw new Error("the replacement Canvas socket was not captured");
      return socket.__fixtureSubscriptionReady;
    }),
  );
  monitor.expectNetworkInterruption(false);
  await assertRecoveryBeginsLive(
    page,
    fixture,
    seeded,
    beforeExtent,
    advanced,
    "written after browser reconnect",
  );
  const after = await renderedFactSequences(page);
  assert.ok(
    after.some((seq) => before.includes(seq)),
    "same-generation recovery retains previously admitted fact evidence",
  );
}

assertBoundaryLoadReading();
const run = startShardWorker(process.argv.slice(2));
if (process.env.SPICE_CANVAS_PREBUILT !== "1" && !process.env.SPICE_CANVAS_BINARY) {
  await buildResident();
}
let fixture;
let linkedRoot;
let owner;
let resident;
let browser;
try {
  if (run.owns("ownership")) {
    await run.scenario("launch-boundaries", async () => {
      await assertHeadlessLaunchSites();
      await assertEndpointProbeClosesWithoutResponse();
      await assertEndpointProbeStates();
      await assertSignalExitedChildSkipsTeardown();
      await assertLineageDrainFailureTeardown();
    });
    await run.scenario("fixture-reaping", async () => {
      await assertSurvivingVendorsAreReported();
      await assertFullyReapedVendorsStaySilent();
      await assertFixtureTeardownReaps();
    });
  } else {
    fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-browser-")));
    linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-lanes-")));
    owner = ownFixture({ fixture, linkedRoot });
    await mkdir(artifacts, { recursive: true });
    const worktrees = await configureFixture(fixture, linkedRoot, 6, {
      // Interaction owns several reload-sensitive agent workflows and tests no
      // lease deadline. Keep those occupants stable just as the dedicated
      // lease shard does; short expiry remains covered by the other shards.
      laneLease: run.owns("lease") || run.owns("interaction") || run.owns("resilience")
        ? "5m"
        : "5s",
      noCardMember: 5,
      worktreeNames: {
        4: longWorktreeBasename,
      },
    });
    resident = owner.ownResident(await startResident(fixture));
    const seeded = await seedAuthority(fixture);
    browser = await launchBrowser();
    let light;
    let lightPage;
    let lightMonitor;
    let hiddenStatusReport;
    let dark;
    let darkMonitor;
    if (run.owns("readiness")) {
      await run.scenario("light-readiness", async () => {
        await assertHandshakeTimeout(browser, resident.discovery.endpoint);
        await assertCanvasProtocolRefresh(browser, resident.discovery.endpoint);
        await assertLineageLifeIsolation(browser);
        await assertSevenMemberComposerStrip(browser);
        light = await browser.newContext({ colorScheme: "light", viewport: { width: 1440, height: 900 } });
        await installBrowserSpeech(light);
        await installSocketControl(light);
        lightPage = await light.newPage();
        lightMonitor = monitorPage(lightPage);
        lightPage.on("console", (message) => process.stderr.write(`browser console: ${message.text()}\n`));
        await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
        await assertCanvas(lightPage, "light");
        await assertDistinctWaitCallSites(lightPage);
        await assertWaitReporting(lightPage, resident);
        lightMonitor.assertClean();
        await light.close();
      });
    } else if (run.owns("task-chip")) {
      await run.scenario("inline-task-chip", async () => {
        light = await browser.newContext({ colorScheme: "dark", viewport: { width: 1000, height: 568 } });
        await installBrowserSpeech(light);
        await installSocketControl(light);
        lightPage = await light.newPage();
        lightMonitor = monitorPage(lightPage);
        await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
        await assertCanvas(lightPage, "task-chip");
        await captureFirefoxAgentPageSpacing(resident.discovery.endpoint, artifacts, {
          file: "firefox-agents-empty-480x468.png",
          populated: false,
          state: "Firefox empty Lanes viewport",
        });
        const team = await seatAgentTeam(lightPage, fixture, worktrees);
        await team.console.getByRole("tab", { name: /^Lanes/ }).click();
        const surface = team.console.locator(
          `.team-surface[data-lineage-id="${team.host.lineage_id}"]`,
        );
        await assertTaskChipTypographyMatrix(
          lightPage,
          surface,
          join(artifacts, "canvas-task-chip.png"),
        );
        await captureFirefoxAgentPageSpacing(resident.discovery.endpoint, artifacts, {
          file: "firefox-agents-short-480x468.png",
          populated: true,
          state: "Firefox short Lanes viewport",
        });
        lightMonitor.assertClean();
        await light.close();
      });
    } else if (run.owns("interaction")) {
      light = await browser.newContext({ colorScheme: "light", viewport: { width: 1440, height: 900 } });
      await installPersistedMosaicImageRoutes(light);
      const activityReplay = await installActivityReplay(light);
      await installBrowserSpeech(light);
      await installSocketControl(light);
      lightPage = await light.newPage();
      lightMonitor = monitorPage(lightPage);
      lightPage.on("console", (message) => process.stderr.write(`browser console: ${message.text()}\n`));
      await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
      await assertCanvas(lightPage, "light");
      await run.scenario("agent-conversation", async () => {
        Object.assign(seeded, await assertAgentConversation(
          lightPage,
          fixture,
          worktrees,
          activityReplay,
          artifacts,
        ));
        const pointerLane = lightPage.locator(
          `.team-agent[data-agent-lane="${seeded.lane}"]`,
        );
        await assertComposerPointerSelection(
          lightPage,
          pointerLane,
          pointerLane.locator(".composer-shard--primary"),
        );
        await assertTeamMenu(lightPage, fixture, artifacts);
      });
      await run.scenario("reference-composer", async () => {
        const reference = await browser.newContext({ colorScheme: "dark", viewport: { width: 1000, height: 568 } });
        await installPersistedMosaicImageRoutes(reference);
        await installBrowserSpeech(reference);
        const page = await reference.newPage();
        const monitor = monitorPage(page);
        await page.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
        await captureReferenceViewport(page, artifacts);
        monitor.assertClean();
        await reference.close();
        seeded.extent = await journalExtent(fixture);
        await assertAuthoritySurface(lightPage, seeded);
        await assertMosaicComposer(lightPage, fixture, seeded, activityReplay);
        await assertComposerRecovery(lightPage, fixture, seeded, lightMonitor);
        await assertVacantWakeSubmission(lightPage, fixture, seeded);
        await assertRetainedHistory(lightPage, fixture);
        await assertNoCredentialOrLandingGap(lightPage, [resident.discovery.capability]);
      });
      await run.scenario("firefox-restored-mosaic", async () => {
        await assertFirefoxRestoredMosaic(
          resident.discovery.endpoint,
          artifacts,
          seeded.agentName,
          seeded.acknowledgementReply,
        );
        assertCandidateMeasurementPaintEngineInventory();
      });
      await run.scenario("authority-task-flow", async () => {
        await assertLiveThreadCoalescing(
          browser,
          resident.discovery.endpoint,
          fixture,
          seeded.task,
        );
        await assertTaskNotes(lightPage, seeded.task);
        await assertRetainedHistory(lightPage, fixture);
        lightMonitor.assertClean();
        await light.close();
      });
      await run.scenario("automatic-speech", async () => {
        await assertAutomaticSpeechEngines(
          resident.discovery.endpoint,
          fixture,
          worktrees[0],
          seeded.lane,
          artifacts,
        );
      });
    } else if (run.owns("lease")) {
      light = await browser.newContext({ colorScheme: "light", viewport: { width: 1440, height: 900 } });
      await installBrowserSpeech(light);
      await installSocketControl(light);
      lightPage = await light.newPage();
      lightMonitor = monitorPage(lightPage);
      lightPage.on("console", (message) => process.stderr.write(`browser console: ${message.text()}\n`));
      await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
      await assertCanvas(lightPage, "light-lease");
      await run.scenario("composer-lease", async () => {
        const team = await seatLeaseAgent(lightPage, fixture, worktrees.slice(0, 2));
        await assertHeldTaskCount(
          team.console,
          fixture,
          team.lanes,
          async (deadline, observeWhileStopped) => {
            lightMonitor.expectNetworkInterruption(true);
            await stopResident(fixture, resident);
            const observedDeadline = await observeWhileStopped();
            const remaining = Math.max(deadline, observedDeadline) - Date.now();
            if (remaining > 0) {
              await new Promise((resolve) => setTimeout(resolve, remaining));
            }
            resident = owner.ownResident(await startResident(fixture));
            await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
            lightMonitor.expectNetworkInterruption(false);
            await chooseCanvasView(lightPage, "Lanes");
          },
          artifacts,
        );
        lightMonitor.assertClean();
        await light.close();
      });
    } else if (run.owns("restart")) {
      light = await browser.newContext({ colorScheme: "light", viewport: { width: 1440, height: 900 } });
      await installBrowserSpeech(light);
      await installSocketControl(light);
      lightPage = await light.newPage();
      lightMonitor = monitorPage(lightPage);
      lightPage.on("console", (message) => process.stderr.write(`browser console: ${message.text()}\n`));
      await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
      await assertCanvas(lightPage, "light-restart");
      hiddenStatusReport = await assertWaitReporting(lightPage, resident);
      const team = await seatAgentTeam(lightPage, fixture, worktrees);
      Object.assign(seeded, {
        lane: team.lane,
        quoteLane: team.quoteLane,
        agentName: team.agentName,
        quoteProse: team.quoteProse,
        primaryHeadings: team.primaryHeadings,
      });
      await team.console.getByRole("tab", { name: /^Lanes/ }).first().click();
      const primary = team.console.locator(
        `.team-surface[data-lineage-id="${team.host.lineage_id}"]`,
      );
      await primary.locator('.team-lifetime input[type="range"]').fill("2");
      await primary.getByRole("button", { name: "Drain", exact: true }).waitFor();
      const configured = await reply(fixture, ["lane"]);
      const configuredPolicy = {
        lineage: team.host.lineage_id,
        policy: configured.body.subscriptions.find(
          (subscription) => subscription.lineage_id === team.host.lineage_id,
        )?.policy,
      };
      assert.equal(configuredPolicy.policy, "drain", "restart setup records a task policy");
      await reply(fixture, ["steer", seeded.lane, "browser direction survives streaming"]);
      await run.scenario("resident-restart", async () => {
        const firstEpoch = resident.discovery.epoch;
        const firstCapability = resident.discovery.capability;
        await chooseCanvasView(lightPage, "Lanes");
        const settingsMenu = lightPage.locator(".canvas-menu");
        if ((await settingsMenu.getAttribute("open")) === null) {
          await settingsMenu.locator(":scope > summary").click();
        }
        const fastMode = settingsMenu.getByRole("menuitemcheckbox", { name: /Fast mode/i });
        await fastMode.click();
        await waitForPage(
          lightPage,
          "fast mode commits before resident restart",
          TRANSITION_MS,
          () => document
            .querySelector('[role="menuitemcheckbox"]')
            ?.getAttribute("aria-checked") === "true",
        );
        lightMonitor.expectNetworkInterruption(true);
        await chooseCanvasView(lightPage, "Chronology");
        await assertWaitReportDistinguishesLayout(lightPage, resident, hiddenStatusReport);
        const shutdownStarted = performance.now();
        await stopResident(fixture, resident);
        const shutdownMs = performance.now() - shutdownStarted;
        assert.ok(shutdownMs < SHUTDOWN_MS,
          `resident semantic shutdown stays bounded: ${JSON.stringify({ shutdownMs })}`);
        await assertResidentLossObserved(lightPage, resident);
        const retainedBytes = await appendSubstantialPrimaryHistory(fixture);
        assert.ok(retainedBytes >= RETAINED_HISTORY_BYTES,
          "restart fixture authors substantial retained history");
        const restart = { epoch: firstEpoch, capability: firstCapability, policy: configuredPolicy };
        resident = owner.ownResident(
          await assertResidentRestart(fixture, lightPage, restart, { measure: true }),
        );
        const usage = resident.startupUsage;
        assert.ok(usage.wallMs < RESTART_READY_MS,
          `resident readiness ignores retained-history size: ${JSON.stringify(usage)}`);
        assert.ok(usage.cpuMs < RESTART_CPU_MS,
          `resident startup CPU stays bounded: ${JSON.stringify(usage)}`);
        assert.ok(usage.peakRssBytes < RESTART_RSS_BYTES,
          `resident startup RSS stays bounded: ${JSON.stringify(usage)}`);
        lightMonitor.expectNetworkInterruption(false);
        await assertCanvas(lightPage, "light-restarted");
        await chooseCanvasView(lightPage, "Lanes");
        if ((await settingsMenu.getAttribute("open")) === null) {
          await settingsMenu.locator(":scope > summary").click();
        }
        assert.equal(
          await settingsMenu
            .getByRole("menuitemcheckbox", { name: /Fast mode/i })
            .getAttribute("aria-checked"),
          "true",
          "resident restart restores fast mode from durable journal authority",
        );
        await assertAgentSpacingRejectsFallbackOnlyMarkup(
          lightPage,
          "light-restarted Lanes spacing",
        );
        await chooseCanvasView(lightPage, "Chronology");
        assert.equal(await lightPage.getByText(/Note appended to/).count(), 0,
          "new generation clears prior command reply state");
        const restartExtent = await journalExtent(fixture);
        assert.ok(
          (await renderedFactSequences(lightPage))
            .every((seq) => seq > restartExtent.cursor),
          "the new-generation operator subscription replays no prior journal facts",
        );
        await assertLiveNoteArrives(
          lightPage,
          fixture,
          seeded,
          restartExtent.cursor,
          "written after resident restart",
        );
        await assertAuthoritySurface(lightPage, seeded);
        await assertNoCredentialOrLandingGap(
          lightPage, [firstCapability, resident.discovery.capability],
        );
        await chooseCanvasView(lightPage, "Lanes");
        await captureReportedAgentPage(lightPage, "restored short-content Lanes page");
        lightMonitor.assertClean();
        await light.close();
      });
    } else {
      await run.scenario("variant-surfaces", async () => {
        light = await browser.newContext({ colorScheme: "light", viewport: { width: 1440, height: 900 } });
        await installBrowserSpeech(light);
        await installSocketControl(light);
        lightPage = await light.newPage();
        lightMonitor = monitorPage(lightPage);
        await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
        await assertCanvas(lightPage, "light-resilience");
        hiddenStatusReport = await assertWaitReporting(lightPage, resident);
        const team = await seatAgentTeam(lightPage, fixture, worktrees);
        Object.assign(seeded, {
          lane: team.lane,
          quoteLane: team.quoteLane,
          agentName: team.agentName,
          quoteProse: team.quoteProse,
          primaryHeadings: team.primaryHeadings,
        });
        await reply(fixture, ["steer", seeded.lane, "browser direction survives streaming"]);
        for (const [index, member] of team.seated.entries()) {
          const rows = Array.from({ length: 18 + index * 6 }, (_, row) => `- row ${row + 1}`);
          await reply(fixture, ["steer", member.lane,
            [`resilience lattice witness ${index + 1}`, ...rows].join("\n")]);
        }
        seeded.extent = await journalExtent(fixture);
        dark = await browser.newContext({ colorScheme: "dark", viewport: { width: 390, height: 844 } });
        await installBrowserSpeech(dark);
        const page = await dark.newPage();
        darkMonitor = monitorPage(page);
        await page.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
        await assertCanvas(page, "dark-narrow");
        await assertMosaicLattice(page, false);
        await assertAuthoritySurface(page, seeded);
        await assertNoCredentialOrLandingGap(page, [resident.discovery.capability]);
        await assertMotionPreference(
          browser,
          resident.discovery.endpoint,
          artifacts,
          worktrees[0],
        );
        await assertSpeechVariants(browser, resident.discovery.endpoint);
        await assertStaleGeneration(browser, resident.discovery.endpoint, resident.discovery.epoch);
      });
      await run.scenario("socket-resilience", async () => {
        const extent = await journalExtent(fixture);
        await assertSameGenerationResume(
          lightPage, lightMonitor, fixture, seeded, resident.discovery.epoch, extent,
        );
        await assertHalfOpenSocket(
          browser, resident.discovery.endpoint, resident.discovery.epoch, fixture, seeded,
        );
        await assertAgesTickWhileTheResidentIsSilent(
          browser, resident.discovery.endpoint, seeded.lane,
        );
      });
      await run.scenario("team-lifecycle", async () => {
        await assertPerTeamWatchbands(
          lightPage,
          fixture,
          worktrees,
          { artifacts, staleEpoch },
        );
        await assertTeamArrangement(lightPage, fixture, worktrees, artifacts);
        lightMonitor.assertClean();
        darkMonitor.assertClean();
        await dark.close();
      });
    }
  }
} catch (error) {
  throw withLoadReading(error);
} finally {
  if (owner) {
    const pids = [...new Set([resident?.child.pid,
      ...await recordedVendorPids(fixture), ...descendantPids()])].filter(Number.isInteger);
    try {
      if (browser) await browser.close();
    } finally {
      await owner.cleanup();
      const ownership = {
        kind: "ownership",
        shard: run.shard.name,
        workerPid: process.pid,
        pids,
        fixture,
        linkedRoot,
      };
      await auditOwnership(run.shard.name, ownership);
      emitShardEvent(ownership);
    }
  }
}
run.finish();
