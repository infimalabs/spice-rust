import assert from "node:assert/strict";
import { join } from "node:path";

import { TRANSITION_MS, waitForPageFunction as waitForPage } from "./canvas-fixture.mjs";

const VIEWPORTS = Object.freeze([
  Object.freeze({ label: "desktop", width: 1000, height: 568 }),
  Object.freeze({ label: "narrow", width: 390, height: 720 }),
]);

async function waitForTracks(page, label) {
  await waitForPage(page, `${label} Mosaic tracks commit`, TRANSITION_MS, () => (
    [...document.querySelectorAll('[data-pane="agent-console"] .lane-mosaic-plane')]
      .every((plane) => {
        const track = plane.querySelector(":scope > .lane-mosaic-track");
        return plane.dataset.measurementPhase === "ready"
          && track
          && Math.abs(Number(plane.dataset.committedWidth) - track.clientWidth) <= 1;
      })
  ));
}

async function boundaryGeometry(console, edge) {
  return console.evaluate(async (root, requestedEdge) => {
    const px = (style, property) => Number.parseFloat(style[property]);
    const box = (node) => {
      const rect = node.getBoundingClientRect();
      return {
        bottom: rect.bottom,
        height: rect.height,
        left: rect.left,
        right: rect.right,
        top: rect.top,
        width: rect.width,
      };
    };
    const candidatesFor = (plane) => {
      const planeBox = plane.getBoundingClientRect();
      const maximum = plane.scrollHeight - plane.clientHeight;
      return [...plane.querySelectorAll(
        '[data-card-kind="agent-message"][data-layout-phase="committed"]',
      )].flatMap((card) => {
        const actions = card.querySelector(".message-actions");
        if (!actions || actions.querySelectorAll("button").length !== 3) return [];
        const actionsBox = actions.getBoundingClientRect();
        const center = actionsBox.top - planeBox.top + plane.scrollTop + actionsBox.height / 2;
        const target = requestedEdge === "top" ? center : center - plane.clientHeight;
        return target >= 1 && target <= maximum - 1 ? [{ actions, card, target }] : [];
      });
    };
    const planes = [...root.querySelectorAll(".lane-mosaic-plane")];
    const selected = planes.map((plane) => ({ plane, candidates: candidatesFor(plane) }))
      .find(({ candidates }) => candidates.length > 0);
    if (!selected) throw new Error(`${requestedEdge} boundary has no scrollable action footer`);
    const { plane, candidates } = selected;
    const witness = candidates[Math.floor(candidates.length / 2)];
    const activity = plane.closest(".team-activity-grid");
    const composer = activity.previousElementSibling;
    const surface = plane.closest(".team-surface");
    const track = plane.querySelector(":scope > .lane-mosaic-track");
    const activityStyle = getComputedStyle(activity);
    const planeStyle = getComputedStyle(plane);
    const trackStyle = getComputedStyle(track);
    const controls = composer.querySelector(".team-composer-controls");
    const agents = [...composer.querySelectorAll(".team-agent")];
    const controlFloor = Math.max(
      controls.getBoundingClientRect().bottom,
      ...agents.map((agent) => agent.getBoundingClientRect().bottom),
    );

    plane.scrollTop = witness.target;
    await new Promise((resolve) => requestAnimationFrame(() => requestAnimationFrame(resolve)));

    const planeBox = plane.getBoundingClientRect();
    const boundary = requestedEdge === "top" ? planeBox.top : planeBox.bottom;
    const inside = requestedEdge === "top" ? boundary + 2 : boundary - 2;
    const outside = requestedEdge === "top" ? boundary - 2 : boundary + 2;
    const buttons = [...witness.actions.querySelectorAll("button")].map((button) => {
      const bounds = button.getBoundingClientRect();
      const x = bounds.left + bounds.width / 2;
      return {
        ...box(button),
        insideHit: document.elementFromPoint(x, inside)?.closest("button") === button,
        outsideHit: document.elementFromPoint(x, outside)?.closest("button") === button,
      };
    });
    return {
      activity: box(activity),
      activityBorderTop: px(activityStyle, "borderTopWidth"),
      activityPadding: ["paddingTop", "paddingRight", "paddingBottom", "paddingLeft"]
        .map((property) => px(activityStyle, property)),
      boundary,
      buttons,
      cardKey: witness.card.dataset.mosaicCardKey,
      composer: box(composer),
      composerGap: activity.getBoundingClientRect().top - controlFloor,
      edge: requestedEdge,
      plane: {
        ...box(plane),
        clientWidth: plane.clientWidth,
        padding: ["paddingTop", "paddingRight", "paddingBottom", "paddingLeft"]
          .map((property) => px(planeStyle, property)),
        scrollTop: plane.scrollTop,
      },
      surfaceId: surface.dataset.lineageId,
      track: {
        ...box(track),
        clientWidth: track.clientWidth,
        marginLeft: px(trackStyle, "marginLeft"),
        marginRight: px(trackStyle, "marginRight"),
        offsetLeft: track.offsetLeft,
      },
    };
  }, edge);
}

function assertBoundaryGeometry(geometry, label) {
  assert.deepEqual(geometry.activityPadding, [0, 0, 0, 0], `${label} activity has zero inset`);
  assert.deepEqual(geometry.plane.padding, [0, 0, 0, 0], `${label} scroll viewport has zero inset`);
  assert.ok(
    Math.abs(geometry.plane.left - geometry.activity.left) <= 0.5
      && Math.abs(geometry.plane.right - geometry.activity.right) <= 0.5
      && Math.abs(geometry.plane.bottom - geometry.activity.bottom) <= 0.5
      && Math.abs(geometry.plane.top - geometry.activity.top - geometry.activityBorderTop) <= 0.5,
    `${label} scroll viewport reaches every activity edge: ${JSON.stringify(geometry)}`,
  );
  assert.equal(Math.round(geometry.composerGap), 10, `${label} preserves composer geometry`);
  assert.ok(
    Math.abs(geometry.composer.bottom - geometry.activity.top) <= 0.5,
    `${label} viewport starts directly at the composer edge`,
  );
  assert.equal(Math.round(geometry.track.marginLeft), 11, `${label} keeps the card gutter in the track`);
  assert.equal(Math.round(geometry.track.marginRight), 11, `${label} keeps the card gutter in the track`);
  assert.equal(Math.round(geometry.track.offsetLeft), 11, `${label} starts the card track after its gutter`);
  assert.ok(
    Math.abs(
      geometry.track.clientWidth + geometry.track.marginLeft + geometry.track.marginRight
        - geometry.plane.clientWidth,
    ) <= 1,
    `${label} track gutters consume content width without shrinking the viewport`,
  );
  assert.equal(geometry.buttons.length, 3, `${label} exercises play, quote, and copy controls`);
  for (const button of geometry.buttons) {
    assert.ok(
      button.top < geometry.boundary && button.bottom > geometry.boundary,
      `${label} control crosses the exact viewport edge: ${JSON.stringify(button)}`,
    );
    assert.equal(button.insideHit, true, `${label} control remains interactive up to the edge`);
    assert.equal(button.outsideHit, false, `${label} control does not overlap beyond the edge`);
  }
}

export async function captureMessageStreamBoundaries(page, console, artifacts, engine) {
  const originalViewport = page.viewportSize();
  assert.ok(originalViewport, `${engine} boundary coverage has a fixed viewport`);
  try {
    for (const viewport of VIEWPORTS) {
      await page.setViewportSize(viewport);
      await waitForTracks(page, `${engine} ${viewport.label}`);
      for (const edge of ["top", "bottom"]) {
        const label = `${engine} ${viewport.label} ${edge}`;
        const geometry = await boundaryGeometry(console, edge);
        assertBoundaryGeometry(geometry, label);
        await console.locator(
          `.team-surface[data-lineage-id="${geometry.surfaceId}"]`,
        ).screenshot({
          path: join(artifacts, `${engine}-message-stream-${edge}-${viewport.label}.png`),
        });
      }
    }
  } finally {
    await page.setViewportSize(originalViewport);
    await waitForTracks(page, `${engine} restored viewport`);
  }
}
