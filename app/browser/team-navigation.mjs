import assert from "node:assert/strict";

export const TEAM_DESTINATION_LABELS = Object.freeze([
  "Team",
  "Tasks",
  "Agents",
]);

export const GLOBAL_DESTINATION_LABELS = Object.freeze([
  ...TEAM_DESTINATION_LABELS,
]);

/** Prove every rendered team rail is closed over the three scoped modes. */
export async function assertTeamNavigation(scope, label) {
  const rails = scope.locator(".team-mode-rail");
  const railCount = await rails.count();
  assert.ok(railCount > 0, `${label} renders at least one team tablist`);
  for (let index = 0; index < railCount; index += 1) {
    const rail = rails.nth(index);
    const tabs = rail.getByRole("tab");
    assert.deepEqual(
      await rail.locator(".team-mode-label").allTextContents(),
      TEAM_DESTINATION_LABELS,
      `${label} team ${index + 1} exposes only scoped destinations in keyboard order`,
    );
    assert.equal(await tabs.count(), TEAM_DESTINATION_LABELS.length);
    assert.equal(
      await rail.locator('[role="tab"][aria-selected="true"]').count(),
      1,
      `${label} team ${index + 1} restores exactly one scoped selection`,
    );
  }
}

/** Prove global Canvas navigation contains only the retained destinations. */
export async function assertGlobalNavigation(page, label) {
  const navigation = page.getByRole("navigation", { name: "Canvas views" });
  assert.equal(await navigation.count(), 1, `${label} renders one global Canvas navigation`);
  assert.deepEqual(
    await navigation.locator(".pill-label").allTextContents(),
    GLOBAL_DESTINATION_LABELS,
    `${label} preserves the complete global destination order`,
  );
}
