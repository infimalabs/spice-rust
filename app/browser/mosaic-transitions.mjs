import assert from "node:assert/strict";
import { join } from "node:path";

import {
  armPlacementProbe,
  assertMosaicAnchorPreserved,
  assertMosaicLattice,
  assertPlacementRetained,
  assertPlacementStable,
  assertRetainedMosaicIdentity,
  assertUnmoved,
  finishPlacementProbe,
  installPersistedMosaicImageRoutes,
  mosaicScrollSnapshot,
  scrollMosaicAwayFromTop,
  settleMosaicActivity,
  waitForMosaicReady,
} from "./agent-conversation-mosaic.mjs";
import {
  appendPrimaryCompactionBoundary,
  appendPrimaryEmptyCompactionBoundary,
  chooseCanvasView,
  monitorPage,
  TRANSITION_MS,
  triggerPrimaryAgentProse,
} from "./canvas-fixture.mjs";
import {
  assertCompactionDivider,
  assertNoUnavailableToolTerminal,
  waitForCompactionReady,
} from "./compaction-boundary-reference.mjs";
import { launchBrowser } from "./headless-launch.mjs";

const ACTIVITY_DEADLINE_MS = 15_000;
const TRANSITION_WIDE = Object.freeze({ width: 1440, height: 360 });
const TRANSITION_NARROW = Object.freeze({ width: 390, height: 360 });
const TRANSITION_PROSE_ROWS = 48;
const COMPACTED_TOOL_PREVIEW = "Bash: cargo test --locked -p spice";

function transitionProse(label, rows = TRANSITION_PROSE_ROWS) {
  return [
    `Mosaic transition witness ${label}`,
    ...Array.from(
      { length: rows },
      (_, index) => `- ${label} measured row ${index + 1}`,
    ),
  ].join("\n");
}

function authoredLocusSuffix(authored) {
  return `-${authored.offset}-${authored.length}-0`;
}

async function openFirefoxTransitionWitness(endpoint) {
  const browser = await launchBrowser("firefox");
  const context = await browser.newContext({
    colorScheme: "dark",
    reducedMotion: "reduce",
    viewport: TRANSITION_WIDE,
  });
  await installPersistedMosaicImageRoutes(context);
  const page = await context.newPage();
  const monitor = monitorPage(page);
  try {
    await page.goto(endpoint, { waitUntil: "networkidle" });
    await chooseCanvasView(page, "Lanes");
    const organization = page.locator('[data-pane="agent-console"]');
    await organization.getByRole("tab", { name: /^Agents/ }).click();
    const plane = organization.locator(".lane-mosaic-plane");
    await waitForMosaicReady(page);
    await settleMosaicActivity(page, plane);
    return {
      label: "Firefox reduced motion",
      reducedMotion: true,
      browser,
      context,
      monitor,
      page,
      plane,
      organization,
    };
  } catch (failure) {
    await context.close();
    await browser.close();
    throw failure;
  }
}

async function resizeTransitionWitness(witness, viewport, label) {
  const before = await witness.plane.evaluate((element) => ({
    commits: Number(element.dataset.planeCommits),
    committedWidth: Number(element.dataset.committedWidth),
    keys: [...element.querySelectorAll(".mosaic-message-card")]
      .map((card) => card.dataset.mosaicCardKey),
  }));
  const current = witness.page.viewportSize();
  if (!current || current.width !== viewport.width || current.height !== viewport.height) {
    const widthChanges = !current || current.width !== viewport.width;
    let resizeToken = null;
    if (widthChanges) {
      resizeToken = await witness.plane.evaluate((element, expectedWidth) => {
        const token = crypto.randomUUID();
        let resolve;
        const committed = new Promise((done) => { resolve = done; });
        const observer = new MutationObserver(() => {
          if (Number(element.dataset.committedWidth) !== expectedWidth) resolve();
        });
        observer.observe(element, {
          attributes: true,
          attributeFilter: ["data-committed-width"],
        });
        element.__mosaicResizeCommit = { committed, observer, token };
        return token;
      }, before.committedWidth);
    }
    await witness.page.setViewportSize(viewport);
    if (resizeToken) {
      await witness.plane.evaluate((element, expected) => {
        const resize = element.__mosaicResizeCommit;
        if (!resize || resize.token !== expected.token) {
          throw new Error("the armed Mosaic resize notification changed");
        }
        let deadline;
        return Promise.race([
          resize.committed,
          new Promise((_, reject) => {
            deadline = setTimeout(
              () => reject(new Error("the Mosaic width commit notification did not arrive")),
              expected.deadlineMs,
            );
          }),
        ]).finally(() => {
          clearTimeout(deadline);
          resize.observer.disconnect();
          delete element.__mosaicResizeCommit;
        });
      }, { token: resizeToken, deadlineMs: TRANSITION_MS });
    } else {
      await witness.page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
        () => requestAnimationFrame(resolve),
      )));
    }
    await settleMosaicActivity(witness.page, witness.plane);
  }
  const after = await witness.plane.evaluate((element) => ({
    commits: Number(element.dataset.planeCommits),
    committedWidth: Number(element.dataset.committedWidth),
    transition: element.dataset.planeTransition,
    keys: [...element.querySelectorAll(".mosaic-message-card")]
      .map((card) => card.dataset.mosaicCardKey),
  }));
  assert.deepEqual(after.keys, before.keys, `${witness.label} ${label} retains DOM chronology`);
  if (after.committedWidth !== before.committedWidth) {
    assert.equal(after.commits, before.commits + 1, `${witness.label} ${label} commits once`);
    assert.equal(after.transition, "full-replay", `${witness.label} ${label} explicitly replays`);
  }
  assert.deepEqual(witness.page.viewportSize(), viewport, `${witness.label} reaches ${label}`);
  assert.equal(
    await witness.page.evaluate(() => matchMedia("(prefers-reduced-motion: reduce)").matches),
    witness.reducedMotion,
    `${witness.label} keeps its motion contract through ${label}`,
  );
  await assertMosaicLattice(witness.page, false);
}

async function resizeTransitionWitnesses(witnesses, viewport, label) {
  await Promise.all(witnesses.map(
    (witness) => resizeTransitionWitness(witness, viewport, label),
  ));
}

export async function settleAndArmTransitionWitness(witness, operations = {
  settle: settleMosaicActivity,
  scroll: scrollMosaicAwayFromTop,
  arm: armPlacementProbe,
}) {
  await operations.settle(witness.page, witness.plane);
  return {
    witness,
    scroll: await operations.scroll(witness.page, witness.plane),
    armed: await operations.arm(witness.plane),
  };
}

async function scrollAndArmTransitionWitnesses(witnesses) {
  return Promise.all(witnesses.map(async (witness) => ({
    witness,
    scroll: await scrollMosaicAwayFromTop(witness.page, witness.plane),
    armed: await armPlacementProbe(witness.plane),
  })));
}

async function waitForAuthoredCard(witness, source, lane, label) {
  const suffix = Number.isSafeInteger(source.offset) ? authoredLocusSuffix(source) : null;
  const matchText = source.text?.split("\n", 1)[0] ?? null;
  await witness.organization.evaluate((organization, expected) => new Promise((resolve, reject) => {
    const inspect = () => {
      const cards = [...organization.querySelectorAll(
        `.mosaic-message-card[data-agent-lane="${expected.lane}"]`,
      )].filter((card) => (
        expected.suffix
          ? card.dataset.mosaicCardKey?.endsWith(expected.suffix)
          : card.textContent?.includes(expected.text)
      ));
      const plane = organization.querySelector(".lane-mosaic-plane");
      return cards.length === 1
        && cards[0].dataset.layoutPhase === "committed"
        && plane?.dataset.measurementPhase === "ready"
        && plane.dataset.measurementPending === "0";
    };
    const cleanup = () => {
      clearTimeout(deadline);
      observer.disconnect();
    };
    const changed = () => {
      if (!inspect()) return;
      cleanup();
      resolve();
    };
    const observer = new MutationObserver(changed);
    observer.observe(organization, {
      attributes: true,
      attributeFilter: ["data-layout-phase", "data-measurement-phase", "data-measurement-pending"],
      childList: true,
      subtree: true,
    });
    const deadline = setTimeout(() => {
      cleanup();
      reject(new Error(`${expected.engine} did not commit ${expected.label}`));
    }, expected.deadlineMs);
    changed();
  }), {
    deadlineMs: ACTIVITY_DEADLINE_MS,
    engine: witness.label,
    label,
    lane,
    suffix,
    text: matchText,
  });
  const card = suffix
    ? witness.organization.locator(
      `.mosaic-message-card[data-agent-lane="${lane}"][data-mosaic-card-key$="${suffix}"]`,
    )
    : witness.organization.locator(
      `.mosaic-message-card[data-card-kind="agent-message"][data-agent-lane="${lane}"]`,
    ).filter({ hasText: matchText });
  assert.equal(await card.count(), 1, `${witness.label} renders ${label} exactly once`);
  // The test serializes every authored mutation after its initial quiet
  // barrier. Matching the exact new locus only after both its card and the
  // plane report committed/ready with no pending measurements is therefore
  // the completion boundary; another full cadence of silence proves no
  // additional state and multiplied this wait across every transition.
  return card;
}

async function appendedCardSnapshot(plane, card) {
  const key = await card.getAttribute("data-mosaic-card-key");
  return plane.evaluate((element, expectedKey) => {
    const cards = [...element.querySelectorAll(".mosaic-message-card")];
    const appended = cards.find((candidate) => candidate.dataset.mosaicCardKey === expectedKey);
    if (!appended) throw new Error(`the appended Mosaic card disappeared: ${expectedKey}`);
    const box = appended.getBoundingClientRect();
    const style = getComputedStyle(appended);
    return {
      key: expectedKey,
      order: cards.indexOf(appended),
      count: cards.length,
      base: Number(appended.dataset.base),
      rows: Number(appended.dataset.rows),
      box: { x: box.x, y: box.y, width: box.width, height: box.height },
      otherTops: cards
        .filter((candidate) => candidate !== appended)
        .map((candidate) => candidate.getBoundingClientRect().top),
      transitionDuration: style.transitionDuration,
      transitionProperty: style.transitionProperty,
    };
  }, key);
}

function assertVisuallyTopmost(snapshot, label) {
  assert.equal(snapshot.order, snapshot.count - 1, `${label} appends the newest DOM node last`);
  assert.ok(
    snapshot.otherTops.every((top) => snapshot.box.y <= top + 1),
    `${label} paints the newest appended node topmost: ${JSON.stringify(snapshot)}`,
  );
}

async function assertSharedProseAppend(
  witnesses,
  worktree,
  lane,
  label,
  preserveScroll = true,
  proseRows = TRANSITION_PROSE_ROWS,
) {
  const probes = preserveScroll
    ? await scrollAndArmTransitionWitnesses(witnesses)
    : await Promise.all(witnesses.map(async (witness) => ({
      witness,
      scroll: null,
      armed: await armPlacementProbe(witness.plane),
    })));
  const authored = await triggerPrimaryAgentProse(worktree, transitionProse(label, proseRows));
  const cards = await Promise.all(probes.map(({ witness }) => (
    waitForAuthoredCard(witness, authored, lane, label)
  )));
  const keys = await Promise.all(cards.map((card) => card.getAttribute("data-mosaic-card-key")));
  assert.equal(keys[1], keys[0], `${label} reaches both engines as one server event`);
  for (const [index, { witness, scroll, armed }] of probes.entries()) {
    const finished = await finishPlacementProbe(witness.plane, armed);
    const card = await appendedCardSnapshot(witness.plane, cards[index]);
    const proof = `${witness.label} ${label}`;
    if (scroll) assertUnmoved(armed, finished, proof);
    else assertPlacementRetained(armed, finished, proof);
    assert.equal(finished.commits, armed.commits + 1, `${proof} commits exactly once`);
    assert.equal(finished.transition, "append", `${proof} takes the append path`);
    assert.ok(finished.maxRow > armed.maxRow, `${proof} advances the upper frontier`);
    assert.ok(finished.planeRows > armed.planeRows, `${proof} grows the plane extent`);
    assert.ok(finished.scrollHeight > armed.scrollHeight, `${proof} grows the scroll extent`);
    assertVisuallyTopmost(card, proof);
    if (witness.reducedMotion) {
      assert.equal(card.transitionDuration, "0s", `${proof} suppresses the liquid tween`);
      assert.equal(card.transitionProperty, "none", `${proof} transitions no property`);
    } else {
      assert.equal(card.transitionDuration, "0.18s", `${proof} retains the liquid tween`);
      assert.equal(card.transitionProperty, "opacity", `${proof} tweens only opacity`);
    }
    if (scroll) {
      assertMosaicAnchorPreserved(
        scroll,
        await mosaicScrollSnapshot(witness.plane),
        `${proof} while scrolled`,
      );
    }
    await assertMosaicLattice(witness.page, false);
  }
  return cards;
}

async function assertSharedPrefixBackfill(witnesses, worktree, lane) {
  const probes = await Promise.all(
    witnesses.map((witness) => settleAndArmTransitionWitness(witness)),
  );
  const authored = await triggerPrimaryAgentProse(
    worktree,
    transitionProse("old prefix backfill"),
    "2001-01-01T00:00:00.000Z",
  );
  const cards = await Promise.all(probes.map(({ witness }) => (
    waitForAuthoredCard(witness, authored, lane, "old prefix backfill")
  )));
  const keys = await Promise.all(cards.map((card) => card.getAttribute("data-mosaic-card-key")));
  assert.equal(keys[1], keys[0], "old prefix backfill reaches both engines as one server event");
  for (const [index, { witness, scroll, armed }] of probes.entries()) {
    const finished = await finishPlacementProbe(witness.plane, armed);
    const card = await appendedCardSnapshot(witness.plane, cards[index]);
    const label = `${witness.label} old prefix backfill`;
    assertPlacementStable(armed, finished, label);
    assert.equal(finished.commits, armed.commits + 1, `${label} commits exactly once`);
    assert.equal(finished.transition, "backfill", `${label} takes the backfill path`);
    assert.equal(finished.maxRow, armed.maxRow, `${label} keeps the upper frontier fixed`);
    assert.ok(finished.planeRows > armed.planeRows, `${label} extends the lower frontier`);
    assert.ok(finished.scrollHeight > armed.scrollHeight, `${label} grows the scroll extent`);
    assert.equal(card.order, 0, `${label} inserts the older DOM node first`);
    assert.ok(
      card.base < Math.min(...armed.cards.map((candidate) => Number(candidate.base))),
      `${label} places old history below every committed base: ${JSON.stringify(card)}`,
    );
    assertMosaicAnchorPreserved(
      scroll,
      await mosaicScrollSnapshot(witness.plane),
      `${label} while scrolled`,
    );
    await assertMosaicLattice(witness.page, false);
  }
}

export async function assertCompactionBoundary(
  page,
  fixture,
  organization,
  seeded,
  artifacts,
  postHiddenMosaic,
) {
  assert.ok(postHiddenMosaic, "compaction receives the post-hidden continuity witness");
  const originalViewport = page.viewportSize();
  const firefox = await openFirefoxTransitionWitness(page.url());
  const chromium = {
    label: "Chromium no preference",
    reducedMotion: false,
    page,
    plane: organization.locator(".lane-mosaic-plane"),
    organization,
  };
  const witnesses = [chromium, firefox];
  const primaryWorktree = seeded.organizationWorktrees[0];
  let complete = false;
  try {
    await waitForMosaicReady(page);
    await settleMosaicActivity(page, chromium.plane);
    await resizeTransitionWitnesses(witnesses, TRANSITION_WIDE, "wide viewport");
    await assertSharedProseAppend(
      witnesses,
      primaryWorktree,
      seeded.lane,
      "wide pre-compaction append",
      false,
    );
    await resizeTransitionWitnesses(witnesses, TRANSITION_NARROW, "narrow viewport");
    await assertRetainedMosaicIdentity(
      chromium.plane,
      postHiddenMosaic,
      "post-hidden narrow geometry replay",
    );
    await assertSharedProseAppend(
      witnesses,
      primaryWorktree,
      seeded.lane,
      "narrow pre-compaction append",
    );
    await assertSharedPrefixBackfill(witnesses, primaryWorktree, seeded.lane);

    const probes = await scrollAndArmTransitionWitnesses(witnesses);
    const authoredBoundary = await appendPrimaryCompactionBoundary(fixture);
    const dividers = await Promise.all(probes.map(({ witness }) => (
      waitForAuthoredCard(witness, authoredBoundary, seeded.lane, "compaction boundary")
    )));
    const boundaryKeys = [];
    for (const [index, { witness, scroll, armed }] of probes.entries()) {
      const divider = dividers[index];
      const finished = await finishPlacementProbe(witness.plane, armed);
      const card = await appendedCardSnapshot(witness.plane, divider);
      const label = `${witness.label} narrow compaction boundary`;
      assertUnmoved(armed, finished, label);
      assert.equal(finished.commits, armed.commits + 1, `${label} commits exactly once`);
      assert.equal(finished.transition, "append", `${label} takes the append path`);
      assert.ok(finished.maxRow > armed.maxRow, `${label} advances the upper frontier`);
      assertVisuallyTopmost(card, label);
      await waitForCompactionReady(witness.page, divider, label);
      const laneAccent = await witness.organization
        .locator(`.organization-agent[data-agent-lane="${seeded.lane}"]`)
        .getAttribute("data-accent");
      assert.equal(await divider.getAttribute("data-accent"), laneAccent, `${label} uses its lane accent`);
      assert.equal(
        await witness.organization.locator('[data-card-kind="compaction"]').count(),
        1,
        `${label} renders exactly once`,
      );
      assertMosaicAnchorPreserved(
        scroll,
        await mosaicScrollSnapshot(witness.plane),
        `${label} while scrolled`,
      );
      boundaryKeys.push(card.key);
      await assertMosaicLattice(witness.page, false);
    }
    assert.equal(boundaryKeys[1], boundaryKeys[0], "both engines ingest the same compaction locus");
    await assertRetainedMosaicIdentity(
      chromium.plane,
      postHiddenMosaic,
      "the later narrow compaction boundary",
    );
    assert.ok(
      boundaryKeys[0].endsWith(authoredLocusSuffix(authoredBoundary)),
      `both engines render the exact authored compaction locus: ${JSON.stringify({
        boundaryKey: boundaryKeys[0],
        authoredBoundary,
      })}`,
    );

    await assertSharedProseAppend(
      witnesses,
      primaryWorktree,
      seeded.lane,
      "narrow post-compaction append",
    );
    for (const [index, witness] of witnesses.entries()) {
      await assertCompactionDivider(
        dividers[index],
        `${witness.label} narrow compaction boundary`,
        seeded.agentName,
      );
      await assertNoUnavailableToolTerminal(
        dividers[index],
        `${witness.label} narrow compaction boundary`,
        seeded.agentName,
        COMPACTED_TOOL_PREVIEW,
      );
    }

    const emptyProbes = await scrollAndArmTransitionWitnesses(witnesses);
    const authoredEmptyBoundary = await appendPrimaryEmptyCompactionBoundary(fixture);
    const emptyDividers = await Promise.all(emptyProbes.map(({ witness }) => (
      waitForAuthoredCard(
        witness,
        authoredEmptyBoundary,
        seeded.lane,
        "empty compaction boundary",
      )
    )));
    const emptyBoundaryKeys = [];
    for (const [index, { witness, scroll, armed }] of emptyProbes.entries()) {
      const divider = emptyDividers[index];
      const finished = await finishPlacementProbe(witness.plane, armed);
      const card = await appendedCardSnapshot(witness.plane, divider);
      const label = `${witness.label} narrow empty compaction boundary`;
      assertUnmoved(armed, finished, label);
      assert.equal(finished.commits, armed.commits + 1, `${label} commits exactly once`);
      assert.equal(finished.transition, "append", `${label} takes the append path`);
      assert.ok(finished.maxRow > armed.maxRow, `${label} advances the upper frontier`);
      assertVisuallyTopmost(card, label);
      await waitForCompactionReady(witness.page, divider, label);
      const laneAccent = await witness.organization
        .locator(`.organization-agent[data-agent-lane="${seeded.lane}"]`)
        .getAttribute("data-accent");
      assert.equal(await divider.getAttribute("data-accent"), laneAccent, `${label} uses its lane accent`);
      assert.equal(
        await witness.organization.locator('[data-card-kind="compaction"]').count(),
        2,
        `${label} renders both distinct boundaries exactly once`,
      );
      assertMosaicAnchorPreserved(
        scroll,
        await mosaicScrollSnapshot(witness.plane),
        `${label} while scrolled`,
      );
      emptyBoundaryKeys.push(card.key);
      await assertMosaicLattice(witness.page, false);
    }
    assert.equal(
      emptyBoundaryKeys[1],
      emptyBoundaryKeys[0],
      "both engines ingest the same empty compaction locus",
    );
    assert.notEqual(
      emptyBoundaryKeys[0],
      boundaryKeys[0],
      "the pending-call-free epoch has a distinct compaction identity",
    );
    assert.ok(
      emptyBoundaryKeys[0].endsWith(authoredLocusSuffix(authoredEmptyBoundary)),
      `both engines render the exact authored empty compaction locus: ${JSON.stringify({
        boundaryKey: emptyBoundaryKeys[0],
        authoredBoundary: authoredEmptyBoundary,
      })}`,
    );

    await assertSharedProseAppend(
      witnesses,
      primaryWorktree,
      seeded.lane,
      "narrow post-empty-compaction append",
    );
    for (const [index, witness] of witnesses.entries()) {
      await assertCompactionDivider(
        dividers[index],
        `${witness.label} narrow unresolved compaction boundary after empty boundary`,
        seeded.agentName,
      );
      await assertNoUnavailableToolTerminal(
        dividers[index],
        `${witness.label} narrow unresolved compaction boundary after empty boundary`,
        seeded.agentName,
        COMPACTED_TOOL_PREVIEW,
      );
      await assertCompactionDivider(
        emptyDividers[index],
        `${witness.label} narrow empty compaction boundary`,
        seeded.agentName,
      );
      await assertNoUnavailableToolTerminal(
        emptyDividers[index],
        `${witness.label} narrow empty compaction boundary`,
        seeded.agentName,
      );
    }
    await resizeTransitionWitnesses(witnesses, TRANSITION_WIDE, "restored wide viewport");
    for (const [index, witness] of witnesses.entries()) {
      await assertCompactionDivider(
        dividers[index],
        `${witness.label} wide compaction boundary`,
        seeded.agentName,
      );
      await assertNoUnavailableToolTerminal(
        dividers[index],
        `${witness.label} wide compaction boundary`,
        seeded.agentName,
        COMPACTED_TOOL_PREVIEW,
      );
      await assertCompactionDivider(
        emptyDividers[index],
        `${witness.label} wide empty compaction boundary`,
        seeded.agentName,
      );
      await assertNoUnavailableToolTerminal(
        emptyDividers[index],
        `${witness.label} wide empty compaction boundary`,
        seeded.agentName,
      );
    }
    await assertSharedProseAppend(
      witnesses,
      primaryWorktree,
      seeded.lane,
      "wide post-compaction append",
      true,
      TRANSITION_PROSE_ROWS * 5,
    );
    await dividers[0].scrollIntoViewIfNeeded();
    await page.screenshot({
      path: join(artifacts, "canvas-compaction-boundary.png"),
      fullPage: true,
    });
    await dividers[1].scrollIntoViewIfNeeded();
    await firefox.page.screenshot({
      path: join(artifacts, "firefox-live-compaction-boundary.png"),
      fullPage: true,
    });
    firefox.monitor.assertClean();
    complete = true;
    return {
      unresolved: boundaryKeys[0],
      empty: emptyBoundaryKeys[0],
    };
  } finally {
    if (originalViewport && page.viewportSize()
      && (page.viewportSize().width !== originalViewport.width
        || page.viewportSize().height !== originalViewport.height)) {
      await page.setViewportSize(originalViewport);
      if (complete) {
        await waitForMosaicReady(page);
        await assertMosaicLattice(page, false);
      }
    }
    await firefox.context.close();
    await firefox.browser.close();
  }
}
