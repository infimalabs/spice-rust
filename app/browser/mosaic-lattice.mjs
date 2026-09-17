import assert from "node:assert/strict";

import {
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

export async function assertMosaicLattice(page, requireBothReadPaths) {
  await waitForMosaicReady(page);
  const snapshot = await page.locator('[data-pane="agent-console"]').evaluate((console) => ({
    activities: [...console.querySelectorAll(".team-activity-grid")].map((activity) => ({
      columnCount: getComputedStyle(activity).columnCount,
      columnWidth: getComputedStyle(activity).columnWidth,
    })),
    planes: [...console.querySelectorAll(".lane-mosaic-plane")].map((plane) => {
      const track = plane.querySelector(":scope > .lane-mosaic-track");
      const trackStyle = getComputedStyle(track);
      return {
        trackCount: Number(plane.dataset.trackCount),
        baseSpan: Number(plane.dataset.baseSpan),
        minLane: Number(plane.dataset.minLane),
        minCardRows: Number(plane.dataset.minCardRows),
        maxRow: Number(plane.dataset.maxRow),
        module: Number(plane.dataset.module),
        columnGap: Number.parseFloat(trackStyle.columnGap),
        edgeGutter: Number.parseFloat(trackStyle.marginLeft),
        rowGap: Number.parseFloat(trackStyle.rowGap),
        width: track.clientWidth,
        tallThreshold: Number(plane.dataset.tallThreshold),
        cards: [...plane.querySelectorAll(".mosaic-message-card")].map((card) => {
          const box = card.getBoundingClientRect();
          const style = getComputedStyle(card);
          const footer = card.querySelector(".message-footer")?.getBoundingClientRect();
          return {
            key: card.dataset.mosaicCardKey,
            kind: card.dataset.cardKind,
            lane: card.dataset.agentLane,
            creationIndex: Number(card.dataset.creationIndex),
            track: Number(card.dataset.track),
            span: Number(card.dataset.span),
            base: Number(card.dataset.base),
            row: Number(card.dataset.row),
            rows: Number(card.dataset.rows),
            reads: Number(card.dataset.measurementReads),
            measuredSpans: card.dataset.measuredSpans.split(",").filter(Boolean).map(Number),
            measuredWidths: card.dataset.measuredWidths.split(",").filter(Boolean).map(Number),
            heights: card.dataset.measuredHeights.split(",").filter(Boolean).map(Number),
            candidateSpans: card.dataset.candidateSpans.split(",").filter(Boolean).map(Number),
            measureSpan: Number(card.dataset.measureSpan),
            measureProperty: style.getPropertyValue("--mosaic-measure-span"),
            measurementActive: card.hasAttribute("data-mosaic-measurement-active"),
            tallWitness: card.querySelector("h2")?.textContent?.trim() === "Measured lattice witness",
            textOnly: card.querySelector(".message-image") === null,
            columnStart: Number(style.gridColumnStart),
            columnEnd: style.gridColumnEnd,
            rowStart: Number(style.gridRowStart),
            rowEnd: style.gridRowEnd,
            top: Number.parseFloat(style.top),
            height: Number.parseFloat(style.height),
            box: {
              left: box.left,
              right: box.right,
              top: box.top,
              bottom: box.bottom,
            },
            clientHeight: card.clientHeight,
            scrollHeight: card.scrollHeight,
            footerInset: footer ? box.bottom - footer.bottom : null,
            paddingBottom: Number.parseFloat(style.paddingBottom),
          };
        }),
      };
    }),
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
  let wideTextCommits = 0;
  for (const plane of snapshot.planes) {
    assert.equal(plane.trackCount, 12, "the browser renders the twelve-track lattice");
    assert.equal(plane.columnGap, 12, "the horizontal lattice keeps the predecessor gap");
    assert.equal(plane.rowGap, plane.columnGap, "the vertical lattice shares that grid rhythm");
    assert.equal(plane.minLane, 272, "the browser exposes the predecessor minimum lane width");
    assert.equal(plane.minCardRows, 3, "ordinary cards reserve three vertical modules");
    const creationIndices = plane.cards.map((card) => card.creationIndex);
    assert.ok(
      creationIndices.every((index, position) => Number.isSafeInteger(index)
        && (position === 0 || index > creationIndices[position - 1])),
      `DOM order follows unique monotonic Rust creation labels: ${JSON.stringify(creationIndices)}`,
    );
    for (const card of plane.cards) {
      assert.ok(card.key, "every Mosaic card exposes its stable identity");
      const measuredBaseSpan = card.candidateSpans[0];
      const wideSpan = Math.min(plane.trackCount, measuredBaseSpan * 2);
      const wideEligible = wideSpan > measuredBaseSpan && card.heights[0] >= plane.tallThreshold;
      const expectedSpans = wideEligible ? [measuredBaseSpan, wideSpan] : [measuredBaseSpan];
      const latchedSpans = card.measuredSpans.slice(expectedSpans.length);
      assert.ok(latchedSpans.length <= 1, "measurement performs at most one frozen-span read");
      assert.ok(
        latchedSpans.every((span) => !expectedSpans.includes(span)),
        "a frozen-span read is distinct from every current policy candidate",
      );
      const expectedMeasuredSpans = [...expectedSpans, ...latchedSpans];
      assert.equal(
        card.reads,
        expectedMeasuredSpans.length,
        `height reads follow the bounded policy, including at most one frozen-span read: ${JSON.stringify({
          card,
          expectedMeasuredSpans,
          expectedSpans,
          latchedSpans,
          plane: {
            baseSpan: plane.baseSpan,
            tallThreshold: plane.tallThreshold,
            width: plane.width,
          },
        })}`,
      );
      assert.deepEqual(
        card.measuredSpans,
        expectedMeasuredSpans,
        "every DOM read reports the span applied through the measurement layout channel",
      );
      assert.equal(card.measuredWidths.length, card.reads, "every DOM read records its real width");
      assert.equal(card.heights.length, card.reads, "every instrumented read records one height");
      assert.deepEqual(card.candidateSpans, expectedSpans, "height alone admits the wide tier");
      assert.equal(card.measureSpan, measuredBaseSpan, "measurement restores the base diagnostic span");
      assert.equal(
        Number(card.measureProperty),
        card.measureSpan,
        "the restored diagnostic and CSP-safe measurement property agree",
      );
      assert.equal(card.measurementActive, false, "measurement mode exits before committed paint");
      const trackWidth = (
        plane.width - plane.columnGap * (plane.trackCount - 1)
      ) / plane.trackCount;
      for (const [index, span] of card.measuredSpans.entries()) {
        const expectedWidth = trackWidth * span + plane.columnGap * (span - 1);
        assert.ok(
          Math.abs(card.measuredWidths[index] - expectedWidth) <= 1,
          `measurement read ${index} uses span ${span}'s rendered width: ${JSON.stringify({
            expectedWidth,
            plane,
            card,
          })}`,
        );
      }
      if (card.tallWitness && expectedSpans.length === 2) {
        assert.equal(card.candidateSpans.length, 2, "the tall text witness exercises both candidates");
        assert.ok(
          card.measuredWidths[1] > card.measuredWidths[0],
          "the tall text witness measures a genuinely wider second candidate",
        );
        assert.ok(
          card.heights[1] < card.heights[0],
          "the tall text witness becomes naturally shorter at double width",
        );
        const rowsFor = (height) => Math.max(
          plane.minCardRows,
          Math.ceil((height + plane.columnGap) / plane.module),
        );
        const baseRows = rowsFor(card.heights[0]);
        const wideRows = rowsFor(card.heights[1]);
        assert.ok(wideRows < baseRows, "the wide text witness has a smaller natural row count");
      }
      if (
        card.textOnly
          && card.candidateSpans.length >= 2
          && card.span === card.candidateSpans[1]
          && card.heights[1] < card.heights[0]
      ) {
        const wideRows = Math.max(
          plane.minCardRows,
          Math.ceil((card.heights[1] + plane.columnGap) / plane.module),
        );
        assert.equal(card.rows, wideRows, "a wide text card commits its measured wide row count");
        assert.ok(
          Math.abs(card.footerInset - card.paddingBottom) <= 1.1,
          `a wide text card leaves only its natural padding below the footer: ${JSON.stringify({
            footerInset: card.footerInset,
            key: card.key,
            paddingBottom: card.paddingBottom,
          })}`,
        );
        wideTextCommits += 1;
      }
      assert.ok(card.track >= 0 && card.track + card.span <= plane.trackCount);
      if (card.kind === "compaction") {
        assert.ok(card.rows >= 1, "a compaction divider keeps a nonzero row reservation");
      } else {
        assert.ok(card.rows >= plane.minCardRows);
      }
      assert.ok(
        card.box.right - card.box.left + 0.5 >= Math.min(plane.minLane, plane.width),
        `Mosaic card keeps the responsive minimum lane width: ${JSON.stringify(card)}`,
      );
      assert.equal(card.columnStart, card.track + 1);
      assert.equal(card.columnEnd, `span ${card.span}`);
      assert.equal(card.row, card.base, "the stable row instrument is the committed base");
      assert.ok(
        Math.abs(card.top - (
          plane.edgeGutter + (plane.maxRow - card.base - card.rows) * plane.module
        )) <= 0.5,
        `the plane frontier positions the stable card: ${JSON.stringify({ plane, card })}`,
      );
      assert.ok(
        Math.abs(card.height - (card.rows * plane.module - plane.columnGap)) <= 0.5,
        `the committed reservation fixes card height: ${JSON.stringify({ plane, card })}`,
      );
      assert.ok(
        card.clientHeight + 1 >= card.scrollHeight,
        `Mosaic card contains its own content: ${JSON.stringify(card)}`,
      );
      readPaths.add(expectedSpans.length);
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
  assert.ok(readPaths.has(1), "a short card takes exactly one policy height read");
  if (requireBothReadPaths) {
    assert.ok(readPaths.has(2), "a tall card takes exactly two policy height reads");
    assert.ok(wideTextCommits > 0, "a text card commits its shorter wide measurement");
  }
  return placements;
}

export async function waitForMosaicReady(page) {
  await waitForPage(page, "mosaic measurements settle", TRANSITION_MS, () => {
    const planes = [...document.querySelectorAll(
      '[data-pane="agent-console"] .lane-mosaic-plane',
    )];
    return planes.length > 0 && planes.every(
      (plane) => plane.dataset.measurementPhase === "ready"
        && plane.dataset.measurementPending === "0"
        && Math.abs(Number(plane.dataset.committedWidth)
          - plane.querySelector(":scope > .lane-mosaic-track").clientWidth) <= 1,
    );
  });
}
