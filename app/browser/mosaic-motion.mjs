import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import { installActivityReplay } from "./activity-replay.mjs";
import {
  chooseCanvasView,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";
import {
  assertMosaicAnchorPreserved,
  mosaicScrollSnapshot,
  scrollMosaicAwayFromTop,
} from "./agent-conversation-mosaic.mjs";

/// The tween `app/mosaic.css` declares on a liquid card, spelled the way
/// Chromium reports it back. `getComputedStyle` never answers the `none`
/// keyword: `transition: none` resolves to a zero duration, so the duration is
/// what the assertion can actually read.
const TWEENED = "0.18s";
const SETTLED = "0s";

/// Tall enough that steering it always extends the lattice by whole rows, and
/// byte-identical between the two runs so their compensation deltas are
/// directly comparable rather than merely both non-zero.
const PROBE_HEADLINE = "mosaic motion preference probe";
const PROBE_ROWS = [
  ...Array.from({ length: 72 }, (_, index) => `- motion row ${index + 1}`),
];

async function waitForMosaicReady(page) {
  await waitForPage(page, "mosaic measurements settle", TRANSITION_MS, () => {
    const planes = [...document.querySelectorAll(
      '[data-pane="agent-console"] .lane-mosaic-plane',
    )];
    return planes.length > 0 && planes.every(
      (plane) => plane.dataset.measurementPhase === "ready"
        && plane.dataset.measurementPending === "0",
    );
  });
}

/// One reading of a motion preference: what a liquid card's transition computes
/// to, and what the plane did to its own scroll offset when it grew.
///
/// Both halves are taken from the same page under the same preference on
/// purpose. Reading them from separate runs would let a fix that disables the
/// tween by disabling the whole card lifecycle pass both halves separately
/// while failing the invariant they exist to guard together.
async function readMotion(browser, endpoint, reducedMotion, artifacts, worktree) {
  const context = await browser.newContext({
    reducedMotion,
    viewport: { width: 1440, height: 900 },
  });
  try {
    const activityReplay = await installActivityReplay(context);
    const page = await context.newPage();
    await page.goto(endpoint, { waitUntil: "networkidle" });
    await chooseCanvasView(page, "Chat");
    const consolePane = page.locator('[data-pane="agent-console"]');
    await consolePane.getByRole("tab", { name: /^Chat/ }).click();
    const lane = await consolePane.locator(".team-agent").first().getAttribute("data-agent-lane");
    assert.ok(lane, "the motion probe found a seated lane to steer");
    const sourceProse = activityReplay.nextAgentProse(lane);
    await writeFile(join(worktree, ".spice-prose-trigger"), "emit\n");
    await sourceProse;
    await waitForMosaicReady(page);

    const plane = consolePane.locator(".lane-mosaic-plane").first();
    const liquid = plane.locator(".mosaic-message-card.liquid").first();
    await liquid.waitFor({ timeout: TRANSITION_MS });
    const transition = await liquid.evaluate((card) => {
      const style = getComputedStyle(card);
      return {
        duration: style.transitionDuration,
        property: style.transitionProperty,
      };
    });

    if (reducedMotion === "reduce") {
      // Captured here rather than after the probe: the probe shrinks the
      // viewport to 360 to force a scrolled pane, which clips the cards this
      // artifact exists to show settled.
      await page.screenshot({
        path: join(artifacts, "canvas-mosaic-reduced-motion.png"),
        fullPage: true,
      });
    }

    // The restored scenario used to inherit enough transcript height to make
    // the plane scroll. Current packing is denser, so establish that state
    // explicitly before measuring the compensation caused by the next card.
    const overflowHeadline = `${PROBE_HEADLINE} overflow seed ${reducedMotion}`;
    await activityReplay.injectAgentProse(
      page,
      lane,
      [overflowHeadline, ...PROBE_ROWS].join("\n"),
    );
    await waitForMosaicReady(page);

    // The plane has to be away from its leading edge before it grows, or the
    // correction has nothing to preserve and the probe would pass on a build
    // that dropped it entirely.
    await page.setViewportSize({ width: 1440, height: 360 });
    await waitForMosaicReady(page);
    const before = await scrollMosaicAwayFromTop(page, plane);

    const probeHeadline = `${PROBE_HEADLINE} ${reducedMotion}`;
    let grown = false;
    for (let index = 1; index <= 4 && !grown; index += 1) {
      await activityReplay.injectAgentProse(
        page,
        lane,
        [`${probeHeadline} ${index}`, ...PROBE_ROWS].join("\n"),
      );
      await waitForMosaicReady(page);
      grown = Number(await plane.getAttribute("data-max-row")) > before.maxRow;
    }
    assert.ok(grown, "bounded committed agent activity grows the Mosaic plane");
    const after = await mosaicScrollSnapshot(plane);
    assertMosaicAnchorPreserved(before, after, `${reducedMotion} motion compensation`);

    return {
      transition,
      rows: after.maxRow - before.maxRow,
      scrolled: after.scrollTop - before.scrollTop,
      module: after.module,
    };
  } finally {
    await context.close();
  }
}

/// SUR-09 of ~/devel/spice-shape, and case 77 of its CASES.md: reduced motion
/// is set, so no tweens, and compensation still lands.
///
/// The second clause is the one the corpus ranks as quietly lost, and it is the
/// reason this runs the whole probe under both preferences instead of only
/// asserting the transition. The natural way to honour a motion preference is
/// to skip the scroll adjustment too, which answers a reader who asked for less
/// motion by moving them away from what they were reading -- a worse failure
/// than the tween they objected to, and one no transition assertion can see.
export async function assertMotionPreference(browser, endpoint, artifacts, worktree) {
  const settled = await readMotion(browser, endpoint, "reduce", artifacts, worktree);
  const tweened = await readMotion(browser, endpoint, "no-preference", artifacts, worktree);

  assert.equal(
    settled.transition.duration,
    SETTLED,
    "a liquid card carries no tween when the reader asked for reduced motion",
  );
  assert.equal(
    settled.transition.property,
    "none",
    "reduced motion leaves no transitioned property at all, not a zero-length one",
  );
  // Without this the first assertion is vacuous: a card that never reaches the
  // liquid rule at all, or a stylesheet that stopped loading, would report a
  // zero duration under both preferences and still look like a pass.
  assert.equal(
    tweened.transition.duration,
    TWEENED,
    "the same card still tweens when no motion preference is expressed",
  );
  assert.equal(
    tweened.transition.property,
    "opacity",
    "the tween that reduced motion suppresses is the opacity fade, not some other property",
  );

  assert.ok(settled.rows > 0 && tweened.rows > 0, "both probes actually grew the plane");
  assert.equal(settled.module, tweened.module, "both runs measure the same lattice module");

  // Pinning each delta to the rows and module it must equal is what catches a
  // correction that is halved, rounded, or dropped. Comparing the two runs
  // alone cannot: a change that halved the correction everywhere would keep
  // them equal to each other and still lose the reader's place.
  for (const [label, run] of [["reduce", settled], ["no-preference", tweened]]) {
    assert.equal(
      run.scrolled,
      run.rows * run.module,
      `${label} compensates by exactly the added lattice rows: ${JSON.stringify(run)}`,
    );
  }

  // The two runs share one live resident, so the second one steers into a plane
  // that already holds the first one's card and does not grow by the same rows
  // -- measured at 16 against 10. Their raw deltas are therefore not comparable
  // and asserting otherwise would only pin the fixture's own ordering. What is
  // comparable, and what the invariant is actually about, is the correction per
  // row of growth: cross-multiplied here so it stays exact integer arithmetic.
  assert.equal(
    settled.scrolled * tweened.rows,
    tweened.scrolled * settled.rows,
    `reduced motion corrects by the same amount per grown row: ${JSON.stringify({
      settled,
      tweened,
    })}`,
  );
}
