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
  seatAgentOrg,
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
  assertPerOrgWatchbands,
  captureReferenceViewport,
} from "./canvas-watchbands.mjs";
import { assertMotionPreference } from "./mosaic-motion.mjs";
import { assertIntegratedMosaicProof } from "./mosaic-integrated-proof.mjs";
import { assertBidirectionalMediaExchange } from "./media-exchange.mjs";
import { assertOrgArrangement } from "./organization-arrangement.mjs";
import { assertOrgMenu } from "./organization-menu.mjs";
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
import {
  assertAgentRenewalRequest,
  assertTaskChipTypographyMatrix,
} from "./control-reference.mjs";
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
  const organization = page.locator('[data-pane="agent-console"]');
  await organization.getByRole("tab", { name: /^Agents/ }).click();
  const lane = page.locator(`.organization-agent[data-agent-lane="${seeded.lane}"]`);
  const activity = organization.locator(`.mosaic-message-card[data-agent-lane="${seeded.lane}"]`);
  const quoteLane = page.locator(`.organization-agent[data-agent-lane="${seeded.quoteLane}"]`);
  const quoteActivity = organization.locator(
    `.mosaic-message-card[data-agent-lane="${seeded.quoteLane}"]`,
  );
  await assertComposerShards({
    page,
    fixture,
    organization,
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
    await assertStagedMosaicHydration(page, organization, activityReplay, (hydration) => (
      assertIntegratedMosaicProof(
        page,
        organization,
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
    organization.locator(".lane-mosaic-plane"),
    seeded.postHiddenMosaic,
    "staged replay and restored geometry",
  );

  const quoteSourceCandidate = organization.locator(
    `.mosaic-message-card[data-card-kind="agent-message"][data-agent-lane="${seeded.lane}"]`,
  ).first();
  await quoteSourceCandidate.waitFor();
  const quoteSourceKey = await quoteSourceCandidate.getAttribute("data-mosaic-card-key");
  assert.ok(quoteSourceKey, "the quote source has one stable Mosaic identity");
  const quoteSource = organization.locator(`[data-mosaic-card-key="${quoteSourceKey}"]`);

  const compactionBoundary = await assertCompactionBoundary(
    page, fixture, organization, seeded, artifacts, restoredMosaic,
  );
  const unnamedCompaction = await activityReplay.injectCompaction(page, seeded.quoteLane);
  const unnamedDivider = organization.locator(
    `[data-mosaic-card-key="${unnamedCompaction.key}"]`,
  );
  const unnamedAfter = await activityReplay.injectAgentProse(
    page,
    seeded.quoteLane,
    "The unnamed lane continues after compaction.",
  );
  await organization.locator(`[data-mosaic-card-key="${unnamedAfter.key}"]`).waitFor();
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
  assert.equal(await organization.locator('[data-card-kind="operator-message"]').count(), 0);
  await activityReplay.restoreProseSnapshot(page);
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
  await chooseCanvasView(page, "Flow");
  await assertOfflineObserved(page, "refusing a stale generation");
  assert.match(await page.locator(".status-copy").innerText(), /Reconnecting/);
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
  await chooseCanvasView(page, "Flow");
  await assertGenerationOnline(page, epoch, "the half-open page connects", CONNECT_MS);
  await assertHalfOpenSocketRecovers(page, monitor, route, epoch, async () => {
    await reply(fixture, ["task", "note", seeded.task, "written while the socket was half open"]);
  });
  monitor.assertClean();
  await context.close();
}

async function assertSameGenerationResume(
  page,
  monitor,
  fixture,
  seeded,
  epoch,
) {
  await chooseCanvasView(page, "Flow");
  monitor.expectNetworkInterruption(true);
  await page.evaluate(() => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    socket.close(4000, "fixture reconnect");
  });
  await assertOfflineObserved(page, "the fixture closes its socket");
  assert.match(await page.locator(".status-copy").innerText(), /Reconnecting/);
  await reply(fixture, ["task", "note", seeded.task, "written during browser disconnect"]);

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
      await run.phase("vendor-reporting", async () => {
        await run.phase("survivors", () => assertSurvivingVendorsAreReported());
        await run.phase("fully-reaped", () => assertFullyReapedVendorsStaySilent());
      });
      await run.phase("process-lifecycle", () => assertFixtureTeardownReaps());
    });
  } else {
    fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-browser-")));
    linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-lanes-")));
    owner = ownFixture({ fixture, linkedRoot });
    await mkdir(artifacts, { recursive: true });
    const worktrees = await configureFixture(fixture, linkedRoot, 6, {
      // Several reload-sensitive agent workflows test no lease deadline, so
      // keep their occupants stable; short expiry remains covered elsewhere.
      laneLease: run.owns("interaction") || run.owns("speech")
        || run.owns("media") || run.owns("resilience")
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
        const organization = await seatAgentOrg(lightPage, fixture, worktrees);
        await organization.console.getByRole("tab", { name: /^Agents/ }).click();
        const surface = organization.console.locator(
          `.organization-surface[data-lineage-id="${organization.host.lineage_id}"]`,
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
          `.organization-agent[data-agent-lane="${seeded.lane}"]`,
        );
        await assertComposerPointerSelection(
          lightPage,
          pointerLane,
          pointerLane.locator(".composer-shard--primary"),
        );
        await assertOrgMenu(lightPage, fixture, artifacts);
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
        await assertTaskNotes(lightPage, seeded.task);
        lightMonitor.assertClean();
        await light.close();
      });
    } else if (run.owns("speech")) {
      const added = await reply(fixture, [
        "lane", "add", worktrees[0], "--name", "Speech", "--life", "Steer", "--vendor", "claude",
      ]);
      const wake = await reply(fixture, ["lane", "wake", added.body.lane]);
      assert.equal(wake.body.to, "occupied", "the speech fixture starts its agent");
      await run.scenario("automatic-speech", async () => {
        await assertAutomaticSpeechEngines(
          resident.discovery.endpoint,
          fixture,
          added.body.lane,
          artifacts,
        );
      });
    } else if (run.owns("media")) {
      await reply(fixture, [
        "lane", "add", worktrees[0], "--name", "Control", "--life", "Steer", "--vendor", "claude",
      ]);
      const media = await browser.newContext({
        colorScheme: "light",
        viewport: { width: 1000, height: 720 },
      });
      await installSocketControl(media);
      const page = await media.newPage();
      const monitor = monitorPage(page);
      await page.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
      await assertCanvas(page, "media-exchange");
      await run.scenario("media-exchange", async () => {
        await assertBidirectionalMediaExchange(page, fixture, worktrees[4]);
      });
      monitor.assertClean();
      await media.close();
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
      const organization = await seatAgentOrg(lightPage, fixture, worktrees);
      Object.assign(seeded, {
        lane: organization.lane,
        quoteLane: organization.quoteLane,
        agentName: organization.agentName,
        quoteProse: organization.quoteProse,
        primaryHeadings: organization.primaryHeadings,
      });
      await organization.console.getByRole("tab", { name: /^Agents/ }).first().click();
      const primary = organization.console.locator(
        `.organization-surface[data-lineage-id="${organization.host.lineage_id}"]`,
      );
      await primary.locator('.organization-lifetime input[type="range"]').fill("2");
      await primary.getByRole("button", { name: "Drain", exact: true }).waitFor();
      const configured = await reply(fixture, ["lane"]);
      const configuredPolicy = {
        lineage: organization.host.lineage_id,
        policy: configured.body.subscriptions.find(
          (subscription) => subscription.lineage_id === organization.host.lineage_id,
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
        await chooseCanvasView(lightPage, "Flow");
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
        const organization = await seatAgentOrg(lightPage, fixture, worktrees);
        Object.assign(seeded, {
          lane: organization.lane,
          quoteLane: organization.quoteLane,
          agentName: organization.agentName,
          quoteProse: organization.quoteProse,
          primaryHeadings: organization.primaryHeadings,
        });
        await reply(fixture, ["steer", seeded.lane, "browser direction survives streaming"]);
        for (const [index, member] of organization.seated.entries()) {
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
        await assertSameGenerationResume(
          lightPage, lightMonitor, fixture, seeded, resident.discovery.epoch,
        );
        await assertHalfOpenSocket(
          browser, resident.discovery.endpoint, resident.discovery.epoch, fixture, seeded,
        );
        await assertAgesTickWhileTheResidentIsSilent(
          browser, resident.discovery.endpoint, seeded.lane,
        );
      });
      await run.scenario("organization-lifecycle", async () => {
        await assertPerOrgWatchbands(
          lightPage,
          fixture,
          worktrees,
          { artifacts, staleEpoch },
        );
        await assertOrgArrangement(lightPage, fixture, worktrees, artifacts);
        const renewedOrg = lightPage.locator('[data-pane="agent-console"]');
        await renewedOrg.getByRole("tab", { name: /^Agents/ }).first().click();
        await assertAgentRenewalRequest(
          lightPage,
          renewedOrg.locator(".organization-surface").first(),
          fixture,
        );
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
