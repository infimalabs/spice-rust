// What the agent console must look like, measured through the browser.
//
// Every assertion here reads one pane, [data-pane="agent-console"]: the mosaic
// lattice that lays out lane tracks, the roster pips that stand for admitted
// members, and the watchbands a seated team draws. They left canvas.spec.mjs
// together because they are the longest run of that file sharing one subject,
// and because the spec only ever calls four of them -- waitForMosaicReady is
// reached from inside this file alone and stays here unexported.
//
// The move was forced by shape, not taste. canvas.spec.mjs stood at 1498 lines
// against a gate that varies each file's elastic limit by the seed of whatever
// session publishes it -- 1475 to 1525, in eleven equally likely buckets. Five
// of those eleven sit below 1498, so 45% of commits touching the file would
// have breached the elastic limit and latched it to a hard 1000 lines for
// every lane at once, staying latched until someone cut it back under 1000.
// The file was not near a limit; it was past one, for nearly half of the
// sessions that might have published it. Moving these 448 lines takes the
// worst case from 23 lines over to 428 lines of headroom.
import assert from "node:assert/strict";
import { join } from "node:path";

import { assertPrimaryComposerDragging } from "./composer-dragging.mjs";
import { primaryDestination } from "./composer-shards.mjs";
import { assertAgentRenewalRequest } from "./control-reference.mjs";
import {
  artifacts,
  chooseCanvasView,
  reply,
  staleEpoch,
  CONNECT_MS,
  TRANSITION_MS,
  waitForLocator as waitForElement,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

export async function assertMosaicLattice(page, requireBothReadPaths) {
  await waitForMosaicReady(page);
  const snapshot = await page.locator('[data-pane="agent-console"]').evaluate((console) => ({
    activities: [...console.querySelectorAll(".team-activity-grid")].map((activity) => ({
      columnCount: getComputedStyle(activity).columnCount,
      columnWidth: getComputedStyle(activity).columnWidth,
    })),
    planes: [...console.querySelectorAll(".lane-mosaic-plane")].map((plane) => ({
      trackCount: Number(plane.dataset.trackCount),
      baseSpan: Number(plane.dataset.baseSpan),
      tallThreshold: Number(plane.dataset.tallThreshold),
      cards: [...plane.querySelectorAll(".mosaic-message-card")].map((card) => {
        const box = card.getBoundingClientRect();
        const style = getComputedStyle(card);
        return {
          kind: card.dataset.cardKind,
          lane: card.dataset.agentLane,
          creationIndex: Number(card.dataset.creationIndex),
          track: Number(card.dataset.track),
          span: Number(card.dataset.span),
          base: Number(card.dataset.base),
          row: Number(card.dataset.row),
          rows: Number(card.dataset.rows),
          reads: Number(card.dataset.measurementReads),
          heights: card.dataset.measuredHeights.split(",").filter(Boolean).map(Number),
          candidateSpans: card.dataset.candidateSpans.split(",").filter(Boolean).map(Number),
          columnStart: Number(style.gridColumnStart),
          columnEnd: style.gridColumnEnd,
          rowStart: Number(style.gridRowStart),
          rowEnd: style.gridRowEnd,
          box: {
            left: box.left,
            right: box.right,
            top: box.top,
            bottom: box.bottom,
          },
        };
      }),
    })),
  }));

  assert.ok(snapshot.activities.length > 0, "the live activity region is mounted");
  assert.ok(
    snapshot.activities.every(
      (activity) => activity.columnCount === "auto" && activity.columnWidth === "auto",
    ),
    "the live activity region uses no CSS multi-column flow",
  );
  const readPaths = new Set();
  const kinds = new Set();
  const placements = [];
  for (const plane of snapshot.planes) {
    assert.equal(plane.trackCount, 12, "the browser renders the twelve-track lattice");
    assert.deepEqual(
      plane.cards.map((card) => card.creationIndex),
      plane.cards.map((_, index) => index),
      "DOM order is the stable creation order submitted to Rust",
    );
    for (const card of plane.cards) {
      const wideSpan = Math.min(plane.trackCount, plane.baseSpan * 2);
      const wideEligible = wideSpan > plane.baseSpan && card.heights[0] >= plane.tallThreshold;
      const expectedSpans = wideEligible ? [plane.baseSpan, wideSpan] : [plane.baseSpan];
      assert.equal(card.reads, wideEligible ? 2 : 1, "height reads follow the bounded policy");
      assert.equal(card.heights.length, card.reads, "every instrumented read records one height");
      assert.deepEqual(card.candidateSpans, expectedSpans, "height alone admits the wide tier");
      assert.ok(card.track >= 0 && card.track + card.span <= plane.trackCount);
      assert.ok(card.rows >= 2);
      assert.equal(card.columnStart, card.track + 1);
      assert.equal(card.columnEnd, `span ${card.span}`);
      assert.equal(card.rowStart, card.row);
      assert.equal(card.rowEnd, `span ${card.rows}`);
      readPaths.add(card.reads);
      kinds.add(card.kind);
      placements.push(card);
    }
    for (let left = 0; left < plane.cards.length; left += 1) {
      for (let right = left + 1; right < plane.cards.length; right += 1) {
        const one = plane.cards[left].box;
        const two = plane.cards[right].box;
        const separated = one.right <= two.left + 0.5 || two.right <= one.left + 0.5
          || one.bottom <= two.top + 0.5 || two.bottom <= one.top + 0.5;
        assert.ok(separated, `Mosaic cards ${left} and ${right} do not overlap`);
      }
    }
  }
  assert.ok(kinds.has("agent-message"), "agent-authored cards use the measured policy");
  assert.ok(readPaths.has(1), "a short card takes exactly one DOM height read");
  if (requireBothReadPaths) {
    assert.ok(kinds.has("directive"), "pending directives use the measured policy");
    assert.ok(readPaths.has(2), "a tall card takes exactly two DOM height reads");
  }
  return placements;
}

async function waitForMosaicReady(page) {
  await waitForPage(page, "mosaic measurements settle", TRANSITION_MS, () => {
    const planes = [...document.querySelectorAll(
      '[data-pane="agent-console"] .lane-mosaic-plane',
    )];
    return planes.length > 0 && planes.every(
      (plane) => plane.dataset.measurementPhase === "ready",
    );
  });
}

export async function assertRosterPips(page, console, seated) {
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
  assert.equal(
    await page.locator("button.axis-button", { hasText: /^Mosaic/ }).count(),
    0,
    "analytic chrome keeps the layout implementation name private",
  );
  await chooseCanvasView(page, "Agents");
  await console.waitFor();
  await waitForMosaicReady(page);
}

export async function assertQueuedRosterPip(page, console, member) {
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

export async function assertPerTeamWatchbands(page, fixture, worktrees) {
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
  return { lineage: primary.lineage_id, policy: "drain" };
}
