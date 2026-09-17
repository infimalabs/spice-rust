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
      '[data-pane="agent-console"] .team-surface .lane-mosaic-plane',
    );
    return plane?.dataset.measurementPhase === "ready"
      && plane.dataset.measurementPending === "0";
  });
}

async function waitForCard(page, key, label, timeout = TRANSITION_MS) {
  await waitForPage(page, label, timeout, (expected) => {
    const plane = document.querySelector(
      '[data-pane="agent-console"] .team-surface .lane-mosaic-plane',
    );
    const card = plane?.querySelector(`[data-mosaic-card-key="${expected}"]`);
    return Boolean(card)
      && card.dataset.layoutPhase === "committed"
      && plane.dataset.measurementPending === "0";
  }, key);
}

async function revealHistoryEdge(plane, key) {
  await plane.locator(`[data-mosaic-card-key="${key}"]`).waitFor();
  const geometry = await plane.evaluate((element) => new Promise((resolve) => requestAnimationFrame(() => {
    element.scrollTop = element.scrollHeight;
    element.dispatchEvent(new Event("scroll"));
    resolve({
      clientHeight: element.clientHeight,
      scrollHeight: element.scrollHeight,
      scrollTop: element.scrollTop,
    });
  })));
  assert.ok(
    geometry.scrollHeight - geometry.clientHeight - geometry.scrollTop <= 1,
    `history traversal reaches the trailing scroll edge: ${JSON.stringify(geometry)}`,
  );
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

async function assertAgentHistoryDepth(plane, lanes, minimum) {
  const counts = await plane.locator(
    '.mosaic-message-card[data-layout-phase="committed"]',
  ).evaluateAll((cards) => cards.reduce((observed, card) => {
    const lane = card.dataset.agentLane;
    observed[lane] = (observed[lane] ?? 0) + 1;
    return observed;
  }, {}));
  for (const lane of lanes) {
    assert.ok(counts[lane] >= minimum,
      `automatic bounded hydration restores at least ${minimum} cards for ${lane}: `
        + JSON.stringify(counts));
  }
}

async function readTeamWorktrees(console) {
  await console.getByRole("tab", { name: /^Team/ }).click();
  const cards = console.locator(".team-detail-card");
  await cards.nth(3).waitFor();
  const worktrees = await cards.evaluateAll((elements) => elements.map((card) => {
    const row = [...card.querySelectorAll("dl > div")].find(
      (candidate) => candidate.querySelector("dt")?.textContent === "Worktree",
    );
    return row?.querySelector("dd")?.textContent ?? "";
  }));
  assert.equal(worktrees.length, 4, "the progressive team exposes four fixture worktrees");
  assert.ok(worktrees.every((worktree) => worktree.startsWith("/")),
    "each progressive fixture worktree is an absolute path");
  await console.getByRole("tab", { name: /^Agents/ }).click();
  return worktrees;
}

function dragTeam(page, dragged, target) {
  return page.evaluate(({ dragged, target }) => {
    const pips = document.querySelector(
      `.team-surface[data-team="${dragged}"] .team-lights[data-lane-drag-handle]`,
    );
    const surface = document.querySelector(`.team-surface[data-team="${target}"]`);
    if (!pips || !surface) throw new Error("joining teams requires pips and both durable surfaces");
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

async function joinAndRemoveMember(page, fixture, worktrees, surface, plane, primaryTeam) {
  await reply(fixture, [
    "lane", "add", worktrees[4], "--life", "Drive", "--vendor", "claude",
  ]);
  const secondarySnapshot = await reply(fixture, ["lane"]);
  const secondary = secondarySnapshot.body.lanes.find((lane) => lane.worktree === worktrees[4]);
  assert.ok(secondary, "the fixture seats one secondary team for the join proof");
  await waitForPage(page, "the secondary progressive team renders", TRANSITION_MS * 4,
    (team) => Boolean(document.querySelector(
      `.team-surface[data-team="${team}"]`,
    )), secondary.team);

  await dragTeam(page, secondary.team, primaryTeam);
  await waitForPage(page, "the secondary member joins the progressive team",
    TRANSITION_MS * 4, ({ primary, secondary }) => (
      document.querySelector(
        `.team-surface[data-team="${primary}"]`,
      )?.querySelectorAll(".team-agent").length === 5
        && !document.querySelector(`.team-surface[data-team="${secondary}"]`)
    ), { primary: primaryTeam, secondary: secondary.team });
  await reply(fixture, [
    "lane", "add", worktrees[5], "--team", secondary.team,
    "--life", "Drive", "--vendor", "claude",
  ]);
  const recreatedSnapshot = await reply(fixture, ["lane"]);
  const cleanup = recreatedSnapshot.body.lanes.find(
    (lane) => lane.worktree === worktrees[5],
  );
  assert.ok(cleanup, "the recreated team exposes its temporary cleanup lane");
  const recreated = surface.page().locator(
    `.team-surface[data-team="${secondary.team}"]`,
  );
  await recreated.waitFor({ timeout: TRANSITION_MS * 4 });
  const source = surface.locator(
    `.composer-shard--primary[data-shard-target-id="${secondary.lane}"] .composer-shard-prose`,
  );
  const target = recreated.locator(".composer-shard--primary").first();
  await source.dragTo(target, { targetPosition: { x: 10, y: 10 } });
  await waitForPage(page, "the joined member leaves the progressive team",
    TRANSITION_MS * 4, ({ lane, primary, secondary }) => (
      !document.querySelector(
        `.team-surface[data-team="${primary}"] `
          + `.composer-shard--primary[data-shard-target-id="${lane}"]`,
      )
        && Boolean(document.querySelector(
          `.team-surface[data-team="${secondary}"] `
            + `.composer-shard--primary[data-shard-target-id="${lane}"]`,
        ))
    ), { lane: secondary.lane, primary: primaryTeam, secondary: secondary.team });

  await reply(fixture, ["lane", "remove", secondary.lane]);
  await reply(fixture, ["lane", "remove", cleanup.lane]);
  await waitForPage(page, "the topology proof restores the original progressive team",
    TRANSITION_MS * 4, ({ primary, secondary }) => (
      document.querySelector(
        `.team-surface[data-team="${primary}"]`,
      )?.querySelectorAll(".team-agent").length === 4
        && !document.querySelector(`.team-surface[data-team="${secondary}"]`)
    ), { primary: primaryTeam, secondary: secondary.team });
}

async function answerReplay(history, requests, generation, plane) {
  const [firstLane, secondLane, thirdLane] = history.lanes;
  const next = history.requestCount();
  history.answer(requests.get(secondLane), history.pages.second);
  history.answer(requests.get(thirdLane), history.pages.third);
  history.answer(requests.get(firstLane), history.pages.firstReplay);

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
  console,
  activityReplay,
  inspectOneShot,
) {
  assert.equal(typeof inspectOneShot, "function", "pagination supplies an integrated proof sink");
  const surface = console.locator(".team-surface").first();
  const team = await surface.getAttribute("data-team");
  assert.ok(team, "the progressive proof owns one durable team identity");
  const plane = surface.locator(".lane-mosaic-plane");
  const history = activityReplay.progressiveHistory();
  const teamWorktrees = await readTeamWorktrees(console);
  const fixture = teamWorktrees[0];
  const [firstLane, secondLane, thirdLane] = history.lanes;
  assert.ok(history.lanes.includes(team), "the retained fixture includes the team host lane");

  history.sendHeads("initial");
  const initialRequests = await history.waitForRequests(history.lanes);
  await waitForCard(page, history.keys.thirdHead, "three unequal retained heads render");
  assert.equal(initialRequests.size, 3, "three member-local requests are outstanding together");

  let nextRequest = history.requestCount();
  history.answer(initialRequests.get(firstLane), history.pages.empty);
  const firstSparseContinuation = await history.waitForRequest(
    firstLane,
    history.records.firstHead.locus,
    history.generation,
    nextRequest,
  );
  let outstanding = history.requestsForGeneration(history.generation)
    .filter((request) => !request.answered);
  assert.equal(new Set(outstanding.map((request) => request.lane)).size, outstanding.length,
    "an empty completion re-arms only its lane and keeps one request in flight per lane");

  nextRequest = history.requestCount();
  history.answer(firstSparseContinuation, history.pages.empty);
  const firstVisibleContinuation = await history.waitForRequest(
    firstLane,
    history.records.firstHead.locus,
    history.generation,
    nextRequest,
  );
  outstanding = history.requestsForGeneration(history.generation)
    .filter((request) => !request.answered);
  assert.equal(new Set(outstanding.map((request) => request.lane)).size, outstanding.length,
    "successive empty pages advance the server while sibling requests remain singular");

  nextRequest = history.requestCount();
  history.answer(initialRequests.get(thirdLane), history.pages.empty);
  const thirdSparseContinuation = await history.waitForRequest(
    thirdLane,
    history.records.thirdHead.locus,
    history.generation,
    nextRequest,
  );
  outstanding = history.requestsForGeneration(history.generation)
    .filter((request) => !request.answered);
  assert.equal(new Set(outstanding.map((request) => request.lane)).size, outstanding.length,
    "a second lane independently re-arms its unchanged sparse cursor");

  nextRequest = history.requestCount();
  history.answer(thirdSparseContinuation, history.pages.empty);
  const thirdVisibleContinuation = await history.waitForRequest(
    thirdLane,
    history.records.thirdHead.locus,
    history.generation,
    nextRequest,
  );
  outstanding = history.requestsForGeneration(history.generation)
    .filter((request) => !request.answered);
  assert.equal(new Set(outstanding.map((request) => request.lane)).size, outstanding.length,
    "successive sparse completions remain independent across both lanes");

  const epochExcluded = await plane.locator(".mosaic-message-card").evaluateAll(
    (cards, retained) => cards
      .filter((card) => card.dataset.mosaicCardKey !== retained)
      .map((card) => card.dataset.mosaicCardKey),
    history.keys.boundary,
  );
  const epochProbe = await armPlacementProbe(plane, epochExcluded);
  history.answer(thirdVisibleContinuation, history.pages.third);
  await waitForCard(page, history.keys.thirdAfterBoundary,
    "the compaction-crossing member history page renders");
  const epoch = await finishPlacementProbe(plane, epochProbe);
  assert.equal(epoch.transition, "epoch-replay",
    "a member page spanning the compaction boundary replays one structural epoch");
  assertPlacementStable(epochProbe, epoch, "the unaffected compaction divider");

  const eagerViewport = page.viewportSize();
  assert.ok(eagerViewport, "eager hydration has a viewport to restore");
  await page.setViewportSize({ width: eagerViewport.width, height: 568 });
  await waitForReady(page, "the eager-depth viewport settles");
  const backfillProbe = await armPlacementProbe(plane);
  history.answer(firstVisibleContinuation, history.pages.first);
  await waitForCard(
    page,
    history.keys.firstOlder,
    "the first lane backfills independently",
    TRANSITION_MS * 4,
  );
  const backfilled = await finishPlacementProbe(plane, backfillProbe);
  assert.equal(backfilled.transition, "backfill", "the older first page is a prefix backfill");
  assertPlacementStable(backfillProbe, backfilled, "the independent prefix backfill");
  assert.equal(
    history.records.firstOlder.activity,
    undefined,
    "the repeated history cursor is ordinary prose rather than a compaction boundary",
  );
  const eagerGeometry = await plane.evaluate((element, key) => {
    const card = element.querySelector(`[data-mosaic-card-key="${key}"]`);
    if (!card) throw new Error("the eager history cursor card is rendered");
    const viewport = element.getBoundingClientRect();
    const bounds = card.getBoundingClientRect();
    return {
      atEdge: element.scrollHeight - element.clientHeight - element.scrollTop <= 400,
      targetIsNear: bounds.top <= viewport.bottom + 400
        && bounds.bottom >= viewport.top - 400,
    };
  }, history.keys.firstOlder);
  assert.deepEqual(eagerGeometry, { atEdge: false, targetIsNear: false },
    "sub-25 hydration continues after the cursor leaves both geometry gates");
  const firstNext = await history.waitForRequest(firstLane, history.records.firstOlder.locus);
  await page.setViewportSize(eagerViewport);
  await waitForReady(page, "the integrated hydration viewport restores");

  const hiddenProbe = await armPlacementProbe(plane);
  await surface.getByRole("tab", { name: /^Board/ }).click();
  history.sendLive();
  await surface.getByRole("tab", { name: /^Agents/ }).click();
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
  await assertAgentHistoryDepth(plane, history.lanes, 25);
  const staged = await placement(plane);

  const linkedRoot = dirname(teamWorktrees[1]);
  await joinAndRemoveMember(
    page,
    fixture,
    [...teamWorktrees, join(linkedRoot, "lane-5"), join(linkedRoot, "lane-6")],
    surface,
    plane,
    team,
  );
  await assertChronology(plane, history.expectedKeys(), "team removal and join");

  await revealHistoryEdge(plane, history.keys.thirdBeforeBoundary);
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
  const replayTeam = replayPage.locator('[data-pane="agent-console"]');
  const restoredSurface = replayTeam.locator(
    `.team-surface[data-team="${team}"]`,
  );
  await restoredSurface.getByRole("tab", { name: /^Agents/ }).click();
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
