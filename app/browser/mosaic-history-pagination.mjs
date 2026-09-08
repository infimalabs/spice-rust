import assert from "node:assert/strict";
import { dirname, join } from "node:path";

import {
  chooseCanvasView,
  reply,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";
import {
  armPlacementProbe,
  assertPlacementStable,
  assertUnmoved,
  finishPlacementProbe,
} from "./mosaic-placement-reference.mjs";

async function waitForReady(page, label = "progressive Mosaic settles") {
  await waitForPage(page, label, TRANSITION_MS, () => {
    const plane = document.querySelector(
      '[data-pane="agent-console"] .organization-surface .lane-mosaic-plane',
    );
    return plane?.dataset.measurementPhase === "ready"
      && plane.dataset.measurementPending === "0";
  });
}

async function waitForCard(page, key, label) {
  await waitForPage(page, label, TRANSITION_MS, (expected) => {
    const plane = document.querySelector(
      '[data-pane="agent-console"] .organization-surface .lane-mosaic-plane',
    );
    const card = plane?.querySelector(`[data-mosaic-card-key="${expected}"]`);
    return Boolean(card)
      && card.dataset.layoutPhase === "committed"
      && plane.dataset.measurementPending === "0";
  }, key);
}

async function revealHistoryEdge(plane, key) {
  const card = plane.locator(`[data-mosaic-card-key="${key}"]`);
  await card.waitFor();
  await card.scrollIntoViewIfNeeded();
  await plane.evaluate((element) => new Promise((resolve) => requestAnimationFrame(() => {
    element.dispatchEvent(new Event("scroll"));
    resolve();
  })));
}

async function placement(plane) {
  return plane.evaluate((element) => ({
    commits: Number(element.dataset.planeCommits),
    transition: element.dataset.planeTransition,
    cards: [...element.querySelectorAll('.mosaic-message-card[data-layout-phase="committed"]')]
      .map((card) => ({
        key: card.dataset.mosaicCardKey,
        track: card.dataset.track,
        span: card.dataset.span,
        base: card.dataset.base,
        rows: card.dataset.rows,
      })),
  }));
}

async function assertChronology(plane, expected, label) {
  await waitForReady(plane.page(), label);
  const keys = await plane.locator(
    '.mosaic-message-card[data-layout-phase="committed"]',
  ).evaluateAll((cards) => cards.map((card) => card.dataset.mosaicCardKey));
  assert.deepEqual(keys, expected, `${label} follows authoritative total chronology`);
  assert.equal(new Set(keys).size, keys.length, `${label} renders every card identity once`);
}

async function readOrgWorktrees(organization) {
  await organization.getByRole("tab", { name: /^Team/ }).click();
  const cards = organization.locator(".organization-detail-card");
  await cards.nth(3).waitFor();
  const worktrees = await cards.evaluateAll((elements) => elements.map((card) => {
    const row = [...card.querySelectorAll("dl > div")].find(
      (candidate) => candidate.querySelector("dt")?.textContent === "Worktree",
    );
    return row?.querySelector("dd")?.textContent ?? "";
  }));
  assert.equal(worktrees.length, 4, "the progressive organization exposes four fixture worktrees");
  assert.ok(worktrees.every((worktree) => worktree.startsWith("/")),
    "each progressive fixture worktree is an absolute path");
  await organization.getByRole("tab", { name: /^Threads/ }).click();
  return worktrees;
}

function dragOrg(page, dragged, target) {
  return page.evaluate(({ dragged, target }) => {
    const pips = document.querySelector(
      `.organization-surface[data-lineage-id="${dragged}"] .organization-lights[data-lane-drag-handle]`,
    );
    const surface = document.querySelector(`.organization-surface[data-lineage-id="${target}"]`);
    if (!pips || !surface) throw new Error("joining organizations requires pips and both durable surfaces");
    const transfer = new DataTransfer();
    pips.dispatchEvent(new DragEvent("dragstart", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
    }));
    const bounds = surface.getBoundingClientRect();
    const position = {
      clientX: Math.round(bounds.left + bounds.width * 0.1),
      clientY: Math.round(bounds.top + bounds.height / 2),
    };
    surface.dispatchEvent(new DragEvent("dragover", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
      ...position,
    }));
    surface.dispatchEvent(new DragEvent("drop", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
      ...position,
    }));
  }, { dragged, target });
}

async function joinAndRemoveMember(page, fixture, worktrees, surface, plane, primaryLineage) {
  await reply(fixture, [
    "lane", "add", worktrees[4], "--life", "Drive", "--vendor", "claude",
  ]);
  const secondarySnapshot = await reply(fixture, ["lane"]);
  const secondary = secondarySnapshot.body.lanes.find((lane) => lane.worktree === worktrees[4]);
  assert.ok(secondary, "the fixture seats one secondary lineage for the join proof");
  await waitForPage(page, "the secondary progressive lineage renders", TRANSITION_MS * 4,
    (lineage) => Boolean(document.querySelector(
      `.organization-surface[data-lineage-id="${lineage}"]`,
    )), secondary.lineage_id);

  await dragOrg(page, secondary.lineage_id, primaryLineage);
  await waitForPage(page, "the secondary member joins the progressive organization",
    TRANSITION_MS * 4, ({ primary, secondary }) => (
      document.querySelector(
        `.organization-surface[data-lineage-id="${primary}"]`,
      )?.querySelectorAll(".organization-agent").length === 5
        && !document.querySelector(`.organization-surface[data-lineage-id="${secondary}"]`)
    ), { primary: primaryLineage, secondary: secondary.lineage_id });
  await reply(fixture, [
    "lane", "add", worktrees[5], "--lineage", secondary.lineage_id,
    "--life", "Drive", "--vendor", "claude",
  ]);
  const recreatedSnapshot = await reply(fixture, ["lane"]);
  const cleanup = recreatedSnapshot.body.lanes.find(
    (lane) => lane.worktree === worktrees[5],
  );
  assert.ok(cleanup, "the recreated lineage exposes its temporary cleanup lane");
  const recreated = surface.page().locator(
    `.organization-surface[data-lineage-id="${secondary.lineage_id}"]`,
  );
  await recreated.waitFor({ timeout: TRANSITION_MS * 4 });
  const source = surface.locator(
    `.composer-shard--primary[data-shard-target-id="${secondary.lane}"] .composer-shard-prose`,
  );
  const target = recreated.locator(".composer-shard--primary").first();
  await source.dragTo(target, { targetPosition: { x: 10, y: 10 } });
  await waitForPage(page, "the joined member leaves the progressive organization",
    TRANSITION_MS * 4, ({ lane, primary, secondary }) => (
      !document.querySelector(
        `.organization-surface[data-lineage-id="${primary}"] `
          + `.composer-shard--primary[data-shard-target-id="${lane}"]`,
      )
        && Boolean(document.querySelector(
          `.organization-surface[data-lineage-id="${secondary}"] `
            + `.composer-shard--primary[data-shard-target-id="${lane}"]`,
        ))
    ), { lane: secondary.lane, primary: primaryLineage, secondary: secondary.lineage_id });

  await reply(fixture, ["lane", "remove", secondary.lane]);
  await reply(fixture, ["lane", "remove", cleanup.lane]);
  await waitForPage(page, "the topology proof restores the original progressive organization",
    TRANSITION_MS * 4, ({ primary, secondary }) => (
      document.querySelector(
        `.organization-surface[data-lineage-id="${primary}"]`,
      )?.querySelectorAll(".organization-agent").length === 4
        && !document.querySelector(`.organization-surface[data-lineage-id="${secondary}"]`)
    ), { primary: primaryLineage, secondary: secondary.lineage_id });
}

async function answerReplay(history, requests, generation, plane) {
  const [firstLane, secondLane, thirdLane] = history.lanes;
  const next = history.requestCount();
  history.answer(requests.get(secondLane), history.pages.second);
  history.answer(requests.get(thirdLane), history.pages.third);
  history.answer(requests.get(firstLane), history.pages.firstReplay);

  await revealHistoryEdge(plane, history.keys.firstOlder);
  await revealHistoryEdge(plane, history.keys.thirdBeforeBoundary);

  const firstNext = await history.waitForRequest(
    firstLane,
    history.records.firstOlder.locus,
    generation,
    next,
  );
  const thirdNext = await history.waitForRequest(
    thirdLane,
    history.records.thirdBeforeBoundary.locus,
    generation,
    next,
  );
  history.answer(thirdNext, history.pages.thirdFinal);
  history.answer(firstNext, history.pages.firstFinal);
}

/// Prove the merged Mosaic through the production member-local paging path.
/// The historical name remains as the public test entrypoint, but no staged
/// whole-stream replacement participates in hydration.
export async function assertStagedMosaicHydration(
  page,
  organization,
  activityReplay,
  inspectOneShot,
) {
  assert.equal(typeof inspectOneShot, "function", "pagination supplies an integrated proof sink");
  const surface = organization.locator(".organization-surface").first();
  const lineage = await surface.getAttribute("data-lineage-id");
  assert.ok(lineage, "the progressive proof owns one durable organization identity");
  const plane = surface.locator(".lane-mosaic-plane");
  const history = activityReplay.progressiveHistory();
  const organizationWorktrees = await readOrgWorktrees(organization);
  const fixture = organizationWorktrees[0];
  const [firstLane, secondLane, thirdLane] = history.lanes;
  assert.ok(history.lanes.includes(lineage), "the retained fixture includes the lineage host lane");

  history.sendHeads("initial");
  const initialRequests = await history.waitForRequests(history.lanes);
  await waitForCard(page, history.keys.thirdHead, "three unequal retained heads render");
  assert.equal(initialRequests.size, 3, "three member-local requests are outstanding together");

  const backfillProbe = await armPlacementProbe(plane);
  history.answer(initialRequests.get(firstLane), history.pages.first);
  await waitForCard(page, history.keys.firstOlder, "the first lane backfills independently");
  const backfilled = await finishPlacementProbe(plane, backfillProbe);
  assert.equal(backfilled.transition, "backfill", "the older first page is a prefix backfill");
  assertPlacementStable(backfillProbe, backfilled, "the independent prefix backfill");
  await revealHistoryEdge(plane, history.keys.firstOlder);
  const firstNext = await history.waitForRequest(firstLane, history.records.firstOlder.locus);

  const epochExcluded = await plane.locator(".mosaic-message-card").evaluateAll(
    (cards, retained) => cards
      .filter((card) => card.dataset.mosaicCardKey !== retained)
      .map((card) => card.dataset.mosaicCardKey),
    history.keys.boundary,
  );
  const epochProbe = await armPlacementProbe(plane, epochExcluded);
  history.answer(initialRequests.get(thirdLane), history.pages.third);
  await waitForCard(page, history.keys.thirdAfterBoundary,
    "the compaction-crossing member history page renders");
  const epoch = await finishPlacementProbe(plane, epochProbe);
  assert.equal(epoch.transition, "epoch-replay",
    "a member page spanning the compaction boundary replays one structural epoch");
  assertPlacementStable(epochProbe, epoch, "the unaffected compaction divider");

  const hiddenProbe = await armPlacementProbe(plane);
  await surface.getByRole("tab", { name: /^Tasks/ }).click();
  history.sendLive();
  await surface.getByRole("tab", { name: /^Threads/ }).click();
  await waitForCard(page, history.keys.live, "live work received while Lanes is hidden appears");
  const revealed = await finishPlacementProbe(plane, hiddenProbe);
  assertPlacementStable(hiddenProbe, revealed, "the hidden live append reveal");

  await revealHistoryEdge(plane, history.keys.thirdBeforeBoundary);
  const thirdNext = await history.waitForRequest(
    thirdLane,
    history.records.thirdBeforeBoundary.locus,
  );
  history.sendStale(initialRequests.get(thirdLane));
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  assert.equal(await plane.locator(`[data-mosaic-card-key="${history.keys.stale}"]`).count(), 0,
    "a stale page for the prior third-lane frontier is ignored");

  await waitForCard(page, history.keys.live, "live work interleaves while one lane remains stalled");
  assert.equal(initialRequests.get(secondLane).answered, false,
    "the second lane remains stalled while its siblings and live work advance");
  history.answer(firstNext, history.pages.firstFinal);
  await waitForCard(page, history.keys.firstDeep, "the first lane exhausts independently");
  history.answer(thirdNext, history.pages.thirdFinal);
  await waitForReady(page, "the third lane exhausts independently");
  history.answer(initialRequests.get(secondLane), history.pages.second);
  await assertChronology(plane, history.expectedKeys(), "the independently paged run");
  const staged = await placement(plane);

  const linkedRoot = dirname(organizationWorktrees[1]);
  await joinAndRemoveMember(
    page,
    fixture,
    [...organizationWorktrees, join(linkedRoot, "lane-5"), join(linkedRoot, "lane-6")],
    surface,
    plane,
    lineage,
  );
  await assertChronology(plane, history.expectedKeys(), "organization removal and join");

  const reconnectProbe = await armPlacementProbe(plane);
  const beforeReconnect = history.generation;
  const reconnectRequestStart = history.requestCount();
  await page.evaluate(() => window.__canvasSockets.at(-1).close(4000, "history reconnect"));
  const reconnectGeneration = await history.waitForSocketAfter(beforeReconnect);
  await history.waitForServerSnapshot(reconnectGeneration);
  history.sendHeads("reconnect", [thirdLane], reconnectGeneration);
  const rearmed = await history.waitForRequests(
    [thirdLane], reconnectGeneration, reconnectRequestStart,
  );
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  assert.ok(history.requestsForGeneration(reconnectGeneration).every(
    (request) => request.lane === thirdLane,
  ), "same-scope reconnect re-arms only the required third-lane frontier generation");
  history.answer(rearmed.get(thirdLane), history.pages.third);
  await revealHistoryEdge(plane, history.keys.thirdBeforeBoundary);
  const reconnectNext = await history.waitForRequest(
    thirdLane,
    history.records.thirdBeforeBoundary.locus,
    reconnectGeneration,
    reconnectRequestStart,
  );
  history.answer(reconnectNext, history.pages.thirdFinal);
  await waitForReady(page, "same-scope history reconnect settles");
  const reconnected = await finishPlacementProbe(plane, reconnectProbe);
  assertUnmoved(reconnectProbe, reconnected, "same-scope frontier re-arming");

  const mainSocket = history.socketForGeneration(reconnectGeneration);
  const beforeReload = history.generation;
  const reloadRequestStart = history.requestCount();
  const replayViewport = page.viewportSize();
  assert.ok(replayViewport, "reload replay inherits the progressive viewport");
  const replayPage = await page.context().newPage();
  await replayPage.setViewportSize(replayViewport);
  await replayPage.goto(page.url(), { waitUntil: "networkidle" });
  const openedGeneration = await history.waitForSocketAfter(beforeReload);
  await history.waitForServerSnapshot(openedGeneration);
  await replayPage.reload({ waitUntil: "networkidle" });
  const reloadGeneration = await history.waitForSocketAfter(openedGeneration);
  await history.waitForServerSnapshot(reloadGeneration);
  await chooseCanvasView(replayPage, "Lanes");
  const replayOrg = replayPage.locator('[data-pane="agent-console"]');
  const restoredSurface = replayOrg.locator(
    `.organization-surface[data-lineage-id="${lineage}"]`,
  );
  await restoredSurface.getByRole("tab", { name: /^Threads/ }).click();
  history.sendHeads("reload", history.lanes, reloadGeneration);
  const reloadRequests = await history.waitForRequests(
    history.lanes, reloadGeneration, reloadRequestStart,
  );
  const restoredPlane = restoredSurface.locator(".lane-mosaic-plane");
  await answerReplay(history, reloadRequests, reloadGeneration, restoredPlane);
  await assertChronology(restoredPlane, history.expectedKeys(), "full reload replay");
  const oneShot = await placement(restoredPlane);
  assert.deepEqual(oneShot.cards, staged.cards,
    "member-local reload replay converges to deterministic Mosaic placement");

  const audit = history.audit();
  assert.equal(audit.deliveries.filter((delivery) => delivery.kind === "stale").length, 1,
    "the audit retains one deliberately stale response");
  await inspectOneShot({
    edge: backfilled,
    oneShot,
    staged,
    stageSizes: audit.deliveries
      .filter((delivery) => delivery.kind === "history")
      .map((delivery) => delivery.keys.length),
    pagination: audit,
  });
  await replayPage.close();
  history.stop();
  await activityReplay.restoreProseSnapshot(page, mainSocket);
  await waitForReady(page, "the canonical prose snapshot is restored after pagination");
}
