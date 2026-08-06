import assert from "node:assert/strict";

/** Prove two complete lineage consoles tile one viewport and scroll alone. */
export async function assertTiledWorkspace(page, fleet, screenshot) {
  const surfaces = fleet.locator(".team-surface");
  assert.equal(await surfaces.count(), 2, "two lineages render two team consoles");
  const geometry = await surfaces.evaluateAll((nodes) => ({
    viewport: document.documentElement.clientWidth,
    consoles: nodes.map((node) => {
      const bounds = node.getBoundingClientRect();
      return {
        left: bounds.left,
        right: bounds.right,
        top: bounds.top,
        width: bounds.width,
        weight: Number(node.dataset.lineageWeight),
        toolbars: node.querySelectorAll(".team-toolbar").length,
        tabs: node.querySelectorAll('[role="tab"]').length,
        agents: node.querySelectorAll(".team-agent").length,
        lifetimes: node.querySelectorAll(".team-lifetime").length,
        sends: node.querySelectorAll(".team-send").length,
        tickers: node.querySelectorAll(".ticker").length,
        activity: node.querySelectorAll(".team-activity-grid").length,
      };
    }),
  }));
  assert.deepEqual(
    geometry.consoles.map(({ toolbars, tabs, agents, lifetimes, sends, tickers, activity }) =>
      [toolbars, tabs, agents, lifetimes, sends, tickers, activity]),
    [[1, 4, 4, 1, 1, 1, 1], [1, 4, 4, 1, 1, 1, 1]],
    "each console owns its complete four-agent chrome",
  );
  assert.deepEqual(
    geometry.consoles.map(({ agents, weight }) => weight === agents),
    [true, true],
    "each lineage's flex weight remains its independent admitted agent count",
  );
  const [left, right] = geometry.consoles;
  assert.ok(
    left.left >= 0 && right.right <= geometry.viewport + 1,
    "both consoles fit the viewport",
  );
  assert.ok(left.right <= right.left + 1, "sibling consoles never overlap");
  assert.ok(Math.abs(left.top - right.top) <= 1, "sibling consoles share one tile row");
  assert.ok(
    Math.abs(left.width / left.weight - right.width / right.weight) <= 1,
    "sibling console widths remain proportional to lineage weight",
  );
  await page.screenshot({ path: screenshot, fullPage: false });

  const scrollers = fleet.locator(".team-compose");
  await scrollers.first().evaluate((node) => {
    node.style.maxHeight = "120px";
    node.scrollTop = node.scrollHeight;
  });
  assert.ok(await scrollers.first().evaluate((node) => node.scrollTop > 0));
  assert.equal(await scrollers.nth(1).evaluate((node) => node.scrollTop), 0);
  await scrollers.first().evaluate((node) => {
    node.style.maxHeight = "";
    node.scrollTop = 0;
  });
}
