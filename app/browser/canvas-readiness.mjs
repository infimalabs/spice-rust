import assert from "node:assert/strict";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import {
  chooseCanvasView,
  CONNECT_MS,
  TRANSITION_MS,
  waitForPageFunction,
} from "./canvas-fixture.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const artifacts = join(root, "artifacts/canvas");
const axisPanes = new Map([
  ["Magnitude", ["Effort", "Pressure", "Residue", "Consent Volume"]],
  ["Flow", ["Tasks", "Claims", "Deferrals", "Reviews"]],
  ["Topology", ["Lanes", "Worktrees", "Lineage", "Occupancy", "Residents"]],
  ["Chronology", [
    "Compaction",
    "Queues",
    "Directives",
    "Consent Disposition",
    "Trust",
    "Commands",
    "Recovery",
  ]],
]);

export async function assertAgentPageBottomSpacing(page, state) {
  const surfaces = await page.locator(".team-surface").evaluateAll((nodes) =>
    nodes.map((surface) => {
      const pane = surface.querySelector(":scope > .team-pane");
      const content = pane?.firstElementChild;
      const activity = pane?.querySelector(".team-activity-grid");
      const plane = activity?.querySelector(".lane-mosaic-plane");
      const cards = [...(plane?.querySelectorAll(
        '.mosaic-message-card[data-layout-phase="committed"]',
      ) ?? [])];
      const floor = cards.length > 0
        ? Math.max(...cards.map((card) => card.getBoundingClientRect().bottom))
        : content?.getBoundingClientRect().bottom;
      const surfaceBox = surface.getBoundingClientRect();
      const planeBox = plane?.getBoundingClientRect();
      return {
        content: content?.className ?? "absent",
        cards: cards.length,
        gap: floor ? surfaceBox.bottom - floor : Number.POSITIVE_INFINITY,
        plane: planeBox ? {
          clientHeight: plane.clientHeight,
          height: planeBox.height,
          overflowY: getComputedStyle(plane).overflowY,
          rows: plane.dataset.planeRows,
          scrollHeight: plane.scrollHeight,
        } : null,
      };
    }),
  );

  assert.ok(surfaces.length > 0, `${state} renders at least one agent surface`);
  for (const surface of surfaces) {
    assert.ok(
      surface.gap <= (surface.cards > 0 ? 24 : 2),
      `${state} has bounded space below ${surface.content}: ${JSON.stringify(surface)}`,
    );
    if (surface.gap < 0) {
      assert.ok(
        surface.plane?.overflowY === "auto"
          && surface.plane.scrollHeight > surface.plane.clientHeight,
        `${state} keeps overflowed activity scrollable: ${JSON.stringify(surface)}`,
      );
    }
  }
}

async function waitForAgentWidths(page, state) {
  await waitForPageFunction(page, `${state} Mosaic widths commit`, TRANSITION_MS, () => (
    [...document.querySelectorAll(".lane-mosaic-plane")].every((plane) => (
      plane.dataset.measurementPhase === "ready"
        && Math.abs(Number(plane.dataset.committedWidth) - plane.clientWidth) <= 1
    ))
  ));
}

export async function captureReportedAgentPage(page, state) {
  const previous = page.viewportSize();
  assert.ok(previous, `${state} has a viewport to restore`);
  await page.setViewportSize({ width: 862, height: 374 });
  await waitForAgentWidths(page, `${state} reported viewport`);
  await assertAgentPageBottomSpacing(page, `${state} reported viewport`);
  const containment = await page.evaluate(() => ({
    bodyWidth: document.body.scrollWidth,
    surfaces: [...document.querySelectorAll(".team-surface")].map((surface) => {
      const box = surface.getBoundingClientRect();
      return { left: box.left, right: box.right };
    }),
    viewportWidth: window.innerWidth,
  }));
  assert.ok(containment.bodyWidth <= containment.viewportWidth, `${state} has no page overflow`);
  assert.ok(
    containment.surfaces.every((surface) => (
      surface.left >= -0.5 && surface.right <= containment.viewportWidth + 0.5
    )),
    `${state} keeps every agent surface in the reported viewport: ${JSON.stringify(containment)}`,
  );
  await page.screenshot({ path: join(artifacts, "canvas-agents-reported-862x374.png") });
  await page.setViewportSize(previous);
  await waitForAgentWidths(page, `${state} restored viewport`);
}

/// Prove the connected canvas is visible, navigable, and within its viewport.
export async function assertCanvas(page, scheme) {
  try {
    await waitForPageFunction(
      page,
      "canvas publishes its first cursor",
      CONNECT_MS,
      () => /cursor \d+/.test(document.querySelector(".bar-meta")?.textContent ?? ""),
    );
  } catch (error) {
    const body = await page.locator("body").innerText().catch(() => "<body unavailable>");
    throw new Error(`canvas did not come online; body=${JSON.stringify(body)}`, { cause: error });
  }
  assert.equal(
    await page.locator(".canvas-menu > summary:visible").count(),
    1,
    "every Canvas page exposes one primary Spice menu",
  );
  assert.ok(
    await page.locator('[data-pane="agent-console"]').count() <= 1,
    "the active view never mounts duplicate lineage consoles",
  );
  assert.equal(
    await page.locator("button.axis-button", { hasText: /^Mosaic/ }).count(),
    0,
    "the internal packing name is not a user-facing view",
  );
  assert.equal(
    await page.locator("button.axis-button", { hasText: /^Agents/ }).count(),
    1,
    "one Agents entry reaches the one conversation surface",
  );
  const pixels = await page.evaluate(() => {
    const root = document.querySelector("#app");
    const canvas = document.querySelector(".content");
    const rootStyle = root ? getComputedStyle(root) : null;
    const canvasBox = canvas?.getBoundingClientRect();
    const workspaceBox = document.querySelector(".workspace")?.getBoundingClientRect();
    const barBox = document.querySelector(".commandbar")?.getBoundingClientRect();
    return {
      appChildren: root?.childElementCount ?? 0,
      bodyWidth: document.body.scrollWidth,
      viewportWidth: window.innerWidth,
      canvasHeight: Math.round(canvasBox?.height ?? 0),
      ink: rootStyle?.color ?? "",
      background: getComputedStyle(document.body).backgroundColor,
      barOverlap: (barBox?.bottom ?? 0) > (workspaceBox?.top ?? 0) + 1,
      overflowingControls: [...document.querySelectorAll("button, .pane-chip")]
        .filter((element) => {
          const style = getComputedStyle(element);
          return style.display !== "none" && style.visibility !== "hidden";
        })
        .filter((element) => element.scrollWidth > element.clientWidth + 1)
        .map((element) => element.textContent?.trim() ?? element.className),
    };
  });
  assert.ok(pixels.appChildren > 0, "Wasm mounted a nonblank app");
  assert.ok(pixels.canvasHeight > 0, "primary canvas has visible content");
  assert.ok(pixels.bodyWidth <= pixels.viewportWidth, "page has no viewport overflow");
  assert.notEqual(pixels.ink, pixels.background, "foreground and background pixels differ");
  assert.equal(pixels.barOverlap, false, "the workspace starts below the command bar");
  assert.deepEqual(pixels.overflowingControls, [], "controls and pane labels fit their bounds");

  for (const [axis, panes] of axisPanes) {
    await chooseCanvasView(page, axis);
    await page.getByRole("heading", { name: axis, exact: true }).waitFor();
    assert.deepEqual(
      await page.locator(".pane-chip").allTextContents(),
      panes,
      `${axis} renders its complete declared pane registry`,
    );
  }
  await page.keyboard.press("Tab");
  assert.ok(await page.locator(":focus-visible").count(), "keyboard focus remains visible");
  if ((page.viewportSize()?.width ?? 1000) <= 560) {
    await chooseCanvasView(page, "Flow");
    await page.locator(".workspace").evaluate((element) => element.scrollTo(0, element.scrollHeight));
    assert.ok(await page.getByRole("button", { name: "Add note" }).isVisible());
    await page.locator(".workspace").evaluate((element) => element.scrollTo(0, 0));
  }
  await chooseCanvasView(page, "Agents");
  await page.locator('[data-pane="agent-console"]').waitFor();
  await assertAgentPageBottomSpacing(page, `${scheme} Agents page`);
  await page.screenshot({ path: join(artifacts, `canvas-${scheme}.png`), fullPage: true });
}
