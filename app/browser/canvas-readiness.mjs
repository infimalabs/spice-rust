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
const agentGeometryOwners = Object.freeze({
  activity: ':scope > .team-pane[data-team-mode="Compose"] '
    + '> .team-mode-pane[data-mode="Compose"] > .team-compose > .team-activity-grid',
  committedCard: ':scope > .lane-mosaic-plane > .lane-mosaic-track '
    + '> .mosaic-message-card[data-layout-phase="committed"]',
  emptyActivity: ":scope > .mosaic-message-empty",
  emptyFleet: ':scope > .team-pane[data-team-mode="Compose"] > .empty',
  measuringCard: ':scope > .lane-mosaic-plane > .lane-mosaic-track '
    + '> .mosaic-message-card[data-layout-phase="measuring"]',
  plane: ":scope > .lane-mosaic-plane",
});
const axisPanes = new Map([
  ["Metrics", ["Effort", "Pressure", "Residue", "Consent Volume"]],
  ["Tasks", ["Tasks", "Claims", "Deferrals", "Reviews"]],
  ["Teams", ["Lanes", "Worktrees", "Lineage", "Occupancy", "Residents"]],
  ["History", [
    "Compaction",
    "Queues",
    "Directives",
    "Consent Disposition",
    "Trust",
    "Commands",
    "Recovery",
  ]],
]);

export async function assertAgentWorkspaceGeometry(page, state) {
  await waitForPageFunction(
    page,
    `${state} populated spacing geometry commits`,
    TRANSITION_MS,
    ({ owners }) => [...document.querySelectorAll(".team-surface")].every((surface) => {
      if (!surface.hasAttribute("data-lineage-id")) return true;
      const activities = surface.querySelectorAll(owners.activity);
      if (activities.length !== 1) return true;
      const activity = activities[0];
      if (activity.dataset.empty !== "false") return true;
      const planes = activity.querySelectorAll(owners.plane);
      if (planes.length !== 1) return true;
      if (activity.querySelectorAll(owners.committedCard).length > 0) return true;
      return activity.querySelectorAll(owners.measuringCard).length === 0;
    }),
    { owners: agentGeometryOwners },
  );
  const geometry = await page.evaluate(({ owners, state }) => {
    const exactlyOne = (root, selector, label) => {
      const matches = root.querySelectorAll(selector);
      if (matches.length !== 1) {
        throw new Error(`${state} requires one ${label}, observed ${matches.length}`);
      }
      return matches[0];
    };
    const box = (element) => {
      const bounds = element.getBoundingClientRect();
      return {
        bottom: bounds.bottom,
        height: bounds.height,
        left: bounds.left,
        right: bounds.right,
        top: bounds.top,
      };
    };
    const style = (element) => {
      const computed = getComputedStyle(element);
      return {
        display: computed.display,
        flexDirection: computed.flexDirection,
        minHeight: computed.minHeight,
        overflowY: computed.overflowY,
      };
    };
    const shell = exactlyOne(document, ".shell--mosaic", "active Mosaic shell");
    const commandbar = exactlyOne(shell, ":scope > .commandbar--mosaic", "Mosaic command bar");
    const workspace = exactlyOne(shell, ":scope > .mosaic-workspace", "Mosaic workspace");
    const content = exactlyOne(workspace, ":scope > .content", "Mosaic content");
    const fleet = exactlyOne(content, ":scope > .lineage-fleet", "lineage fleet");
    const contentStyle = getComputedStyle(content);
    const commandbarStyle = getComputedStyle(commandbar);
    const surfaces = [...fleet.querySelectorAll(":scope > .team-surface")].map((surface) => {
      const exactlyOne = (root, selector, label) => {
        const matches = root.querySelectorAll(selector);
        if (matches.length !== 1) {
          throw new Error(`${state} requires one ${label}, observed ${matches.length}`);
        }
        return matches[0];
      };
      const populated = surface.hasAttribute("data-lineage-id");
      let activity = null;
      let plane = null;
      let cards = [];
      let floor;
      let owner;
      const pane = exactlyOne(surface, ":scope > .team-pane", "team pane");
      if (!populated) {
        const empty = exactlyOne(surface, owners.emptyFleet, "fleet-empty geometry owner");
        const committed = surface.querySelectorAll(
          '.mosaic-message-card[data-layout-phase="committed"]',
        );
        if (committed.length !== 0) {
          throw new Error(`${state} fleet-empty surface has ${committed.length} committed cards`);
        }
        floor = empty.getBoundingClientRect().bottom;
        owner = "fleet-empty";
      } else {
        activity = exactlyOne(surface, owners.activity, "lineage activity geometry owner");
        plane = exactlyOne(activity, owners.plane, "lineage Mosaic plane");
        cards = [...activity.querySelectorAll(owners.committedCard)];
        const emptyState = activity.dataset.empty;
        if (emptyState !== "true" && emptyState !== "false") {
          throw new Error(`${state} lineage activity has invalid data-empty=${emptyState}`);
        }
        if (emptyState === "true") {
          if (cards.length !== 0) {
            throw new Error(`${state} activity-empty surface has ${cards.length} committed cards`);
          }
          const empty = exactlyOne(activity, owners.emptyActivity, "activity-empty geometry owner");
          floor = empty.getBoundingClientRect().bottom;
          owner = "activity-empty";
        } else {
          if (cards.length === 0) {
            throw new Error(`${state} populated activity has no committed card geometry owner`);
          }
          floor = Math.max(...cards.map((card) => card.getBoundingClientRect().bottom));
          owner = "committed-cards";
        }
      }
      const surfaceBox = surface.getBoundingClientRect();
      const planeBox = plane?.getBoundingClientRect();
      const compose = activity?.parentElement ?? null;
      const cardExtent = cards.reduce(
        (extent, card) => Math.max(extent, card.offsetTop + card.offsetHeight),
        0,
      );
      return {
        box: box(surface),
        cards: cards.length,
        cardExtent,
        gap: surfaceBox.bottom - floor,
        owner,
        pane: style(pane),
        compose: compose ? style(compose) : null,
        activity: activity ? style(activity) : null,
        plane: planeBox ? {
          box: box(plane),
          clientHeight: plane.clientHeight,
          clientWidth: plane.clientWidth,
          horizontalOverflow: plane.scrollWidth - plane.clientWidth,
          overflowY: getComputedStyle(plane).overflowY,
          rows: plane.dataset.planeRows,
          scrollHeight: plane.scrollHeight,
        } : null,
        style: style(surface),
      };
    });
    return {
      chain: {
        content: style(content),
        fleet: style(fleet),
        shell: style(shell),
        workspace: style(workspace),
      },
      commandbar: box(commandbar),
      commandbarPaddingInline: [
        Number.parseFloat(commandbarStyle.paddingLeft),
        Number.parseFloat(commandbarStyle.paddingRight),
      ],
      content: box(content),
      contentPaddingInline: [
        Number.parseFloat(contentStyle.paddingLeft),
        Number.parseFloat(contentStyle.paddingRight),
      ],
      contentPaddingTop: Number.parseFloat(contentStyle.paddingTop),
      contentPaddingBottom: Number.parseFloat(contentStyle.paddingBottom),
      documentHeight: Math.max(document.documentElement.scrollHeight, document.body.scrollHeight),
      documentWidth: Math.max(document.documentElement.scrollWidth, document.body.scrollWidth),
      externalBodyLayout: [...document.body.children]
        .filter((element) => element.id !== "app")
        .reduce((height, element) => height + element.getBoundingClientRect().height, 0),
      fleet: box(fleet),
      fixed: getComputedStyle(shell).overflowY === "hidden",
      shell: box(shell),
      stacked: getComputedStyle(fleet).flexDirection === "column",
      surfaces,
      scrollY: window.scrollY,
      viewportHeight: innerHeight,
      viewportWidth: innerWidth,
      workspace: box(workspace),
    };
  }, { owners: agentGeometryOwners, state });

  assert.ok(geometry.surfaces.length > 0, `${state} renders at least one agent surface`);
  const predecessorEdges = geometry.viewportWidth <= 720
    ? { chromeInline: 8, frameBottom: 8, frameInline: 4 }
    : { chromeInline: 10, frameBottom: 10, frameInline: 10 };
  assert.deepEqual(
    geometry.commandbarPaddingInline,
    [predecessorEdges.chromeInline, predecessorEdges.chromeInline],
    `${state} keeps the predecessor command-bar edge inset: ${JSON.stringify(geometry)}`,
  );
  assert.deepEqual(
    geometry.contentPaddingInline,
    [predecessorEdges.frameInline, predecessorEdges.frameInline],
    `${state} keeps the predecessor frame side gutters: ${JSON.stringify(geometry)}`,
  );
  assert.equal(
    geometry.contentPaddingBottom,
    predecessorEdges.frameBottom,
    `${state} keeps the predecessor frame bottom gutter: ${JSON.stringify(geometry)}`,
  );
  assert.equal(geometry.contentPaddingTop, 0, `${state} does not double-pad below the command bar`);
  assert.ok(
    Math.abs(geometry.content.top - geometry.commandbar.bottom) <= 0.5,
    `${state} starts the frame directly below its chrome: ${JSON.stringify(geometry)}`,
  );
  assert.ok(
    Math.abs(geometry.fleet.left - geometry.content.left - predecessorEdges.frameInline) <= 0.5
      && Math.abs(geometry.content.right - geometry.fleet.right
        - predecessorEdges.frameInline) <= 0.5
      && Math.abs(geometry.content.bottom - geometry.fleet.bottom
        - predecessorEdges.frameBottom) <= 0.5,
    `${state} assigns each outer frame gutter exactly once: ${JSON.stringify(geometry)}`,
  );
  assert.ok(
    geometry.documentWidth <= geometry.viewportWidth + 1,
    `${state} has no document-level horizontal overflow: ${JSON.stringify(geometry)}`,
  );
  if (geometry.externalBodyLayout <= 1) {
    assert.ok(
      geometry.documentHeight <= Math.ceil(geometry.shell.bottom) + 1,
      `${state} document extent ends with the visible shell: ${JSON.stringify(geometry)}`,
    );
  }
  for (const surface of geometry.surfaces) {
    assert.ok(
      surface.box.left >= -0.5 && surface.box.right <= geometry.viewportWidth + 0.5,
      `${state} keeps each team surface inside the viewport: ${JSON.stringify(surface)}`,
    );
    if (surface.plane) {
      assert.equal(surface.plane.overflowY, "auto", `${state} keeps the Mosaic plane scrollable`);
      assert.ok(
        surface.plane.horizontalOverflow <= 1,
        `${state} keeps activity inside the plane width: ${JSON.stringify(surface)}`,
      );
      if (surface.cardExtent > surface.plane.clientHeight + 1) {
        assert.ok(
          surface.plane.scrollHeight > surface.plane.clientHeight
            && surface.plane.scrollHeight >= surface.cardExtent - 1,
          `${state} gives long history to the plane scroller: ${JSON.stringify(surface)}`,
        );
      }
    }
  }

  if (geometry.stacked) {
    for (let index = 1; index < geometry.surfaces.length; index += 1) {
      assert.ok(
        geometry.surfaces[index].box.top >= geometry.surfaces[index - 1].box.bottom - 1,
        `${state} stacks team surfaces without overlap: ${JSON.stringify(geometry.surfaces)}`,
      );
    }
  }
  if (!geometry.fixed) {
    for (const surface of geometry.surfaces) {
      assert.ok(
        surface.gap <= (surface.cards > 0 ? 24 : 2),
        `${state} stacked surface follows its content: ${JSON.stringify(surface)}`,
      );
    }
    return geometry;
  }

  if (geometry.externalBodyLayout <= 1) {
    assert.ok(
      geometry.documentHeight <= geometry.viewportHeight + 1,
      `${state} fixed document has no phantom vertical extent: ${JSON.stringify(geometry)}`,
    );
  }
  assert.equal(geometry.scrollY, 0, `${state} fixed shell clamps root scroll`);
  assert.ok(
    Math.abs(geometry.shell.top) <= 0.5
      && Math.abs(geometry.shell.bottom - geometry.viewportHeight) <= 0.5,
    `${state} fixed shell occupies the viewport: ${JSON.stringify(geometry)}`,
  );
  for (const [name, owner] of Object.entries(geometry.chain)) {
    assert.equal(owner.minHeight, "0px", `${state} ${name} releases its automatic minimum`);
  }
  assert.equal(geometry.chain.shell.overflowY, "hidden", `${state} shell owns the viewport clip`);
  assert.equal(geometry.chain.workspace.display, "flex", `${state} workspace participates in flex sizing`);
  assert.equal(geometry.chain.content.display, "flex", `${state} content participates in flex sizing`);
  assert.equal(geometry.chain.fleet.display, "flex", `${state} fleet participates in flex sizing`);
  const intendedBottom = geometry.content.bottom - geometry.contentPaddingBottom;
  for (const surface of geometry.surfaces) {
    assert.ok(
      Math.abs(surface.box.bottom - intendedBottom) <= 1,
      `${state} surface reaches the content lower inset: ${JSON.stringify({ geometry, surface })}`,
    );
    assert.equal(surface.style.display, "flex", `${state} team surface owns a flex column`);
    assert.equal(surface.style.minHeight, "100%", `${state} team surface fills the fleet block`);
    assert.equal(surface.pane.display, "flex", `${state} team pane participates in flex sizing`);
    assert.equal(surface.pane.minHeight, "0px", `${state} team pane releases its automatic minimum`);
    if (surface.compose) {
      assert.equal(surface.compose.display, "flex", `${state} Compose participates in flex sizing`);
      assert.equal(surface.compose.minHeight, "0px", `${state} Compose releases its automatic minimum`);
      assert.equal(surface.compose.overflowY, "hidden", `${state} Compose does not duplicate scrolling`);
      assert.equal(surface.activity.display, "flex", `${state} activity participates in flex sizing`);
      assert.equal(surface.activity.minHeight, "0px", `${state} activity releases its automatic minimum`);
      assert.ok(
        surface.plane.box.bottom <= surface.box.bottom + 0.5,
        `${state} Mosaic plane remains inside its team surface: ${JSON.stringify(surface)}`,
      );
    }
  }
  return geometry;
}

export async function assertAgentSpacingRejectsFallbackOnlyMarkup(page, state) {
  const marker = "fallback-only-spacing-owner";
  const console = page.locator('[data-pane="agent-console"]');
  await console.evaluate((host, fallbackMarker) => {
    const surface = document.createElement("section");
    surface.className = "team-surface";
    surface.dataset.lineageId = fallbackMarker;
    surface.dataset.fallbackSpacingOwner = fallbackMarker;
    const pane = document.createElement("div");
    pane.className = "team-pane";
    pane.dataset.teamMode = "Compose";
    const mode = document.createElement("div");
    mode.className = "team-mode-pane";
    mode.dataset.mode = "Compose";
    const compose = document.createElement("div");
    compose.className = "team-compose";
    const activity = document.createElement("div");
    activity.className = "team-activity-grid";
    activity.dataset.empty = "false";
    const plane = document.createElement("div");
    plane.className = "lane-mosaic-plane";
    const fallback = document.createElement("div");
    fallback.className = "message-body";
    fallback.textContent = "synthetic fallback";
    plane.append(fallback);
    activity.append(plane);
    compose.append(activity);
    mode.append(compose);
    pane.append(mode);
    surface.append(pane);
    host.append(surface);
  }, marker);
  try {
    await assert.rejects(
      assertAgentWorkspaceGeometry(page, state),
      /populated activity has no committed card geometry owner/,
      `${state} refuses fallback-only synthetic geometry`,
    );
  } finally {
    await page.locator(`[data-fallback-spacing-owner="${marker}"]`).evaluate((surface) => {
      surface.remove();
    });
  }
}

async function waitForAgentWidths(page, state) {
  await waitForPageFunction(page, `${state} Mosaic widths commit`, TRANSITION_MS, () => (
    [...document.querySelectorAll(".lane-mosaic-plane")].every((plane) => (
      plane.dataset.measurementPhase === "ready"
        && Math.abs(Number(plane.dataset.committedWidth)
          - plane.querySelector(":scope > .lane-mosaic-track").clientWidth) <= 1
    ))
  ));
}

export async function captureReportedAgentPage(page, state) {
  const previous = page.viewportSize();
  assert.ok(previous, `${state} has a viewport to restore`);
  const viewports = [
    ["tall desktop", { width: 2018, height: 1728 }],
    ["short desktop", { width: 1000, height: 568 }],
    ["reported stacked", { width: 862, height: 374 }],
    ["mobile stacked", { width: 390, height: 844 }],
  ];
  try {
    for (const [label, viewport] of viewports) {
      await page.setViewportSize(viewport);
      await waitForAgentWidths(page, `${state} ${label}`);
      await assertAgentWorkspaceGeometry(page, `${state} ${label}`);
      if (label === "reported stacked") {
        await page.screenshot({ path: join(artifacts, "canvas-agents-reported-862x374.png") });
      }
    }
  } finally {
    await page.setViewportSize(previous);
    await waitForAgentWidths(page, `${state} restored viewport`);
  }
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
  assert.equal(await page.locator(".canvas-navigation").count(), 0, "navigation stays collapsed");
  await page.getByLabel("Open Spice Canvas menu").click();
  assert.equal(
    await page.locator(".canvas-navigation button.axis-button", { hasText: /^Agents/ }).count(),
    1,
    "one disclosed Agents entry reaches the one conversation surface",
  );
  await page.getByLabel("Open Spice Canvas menu").click();
  const pixels = await page.evaluate(() => {
    const root = document.querySelector("#app");
    const canvas = document.querySelector(".content");
    const rootStyle = root ? getComputedStyle(root) : null;
    const canvasBox = canvas?.getBoundingClientRect();
    const workspaceBox = document.querySelector(".workspace:not([hidden])")?.getBoundingClientRect();
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
        .filter((element) => !element.matches(".message-agent-name"))
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
    const workspace = page.locator(".workspace:not([hidden])");
    await workspace.evaluate((element) => element.scrollTo(0, element.scrollHeight));
    assert.ok(await page.getByRole("button", { name: "Add note" }).isVisible());
    await workspace.evaluate((element) => element.scrollTo(0, 0));
  }
  await chooseCanvasView(page, "Agents");
  await page.locator('[data-pane="agent-console"]').waitFor();
  await assertAgentWorkspaceGeometry(page, `${scheme} Agents page`);
  await page.screenshot({ path: join(artifacts, `canvas-${scheme}.png`), fullPage: true });
}
