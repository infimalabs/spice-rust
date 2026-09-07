import assert from "node:assert/strict";

export const TEAM_DESTINATION_LABELS = Object.freeze([
  "Metrics",
  "Team",
  "Tasks",
  "Agents",
]);

export const GLOBAL_DESTINATION_LABELS = Object.freeze([
  "History",
  ...TEAM_DESTINATION_LABELS,
]);

/** Prove every rendered team rail is closed over the four scoped modes. */
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
      await rail.getByRole("tab", { name: /^History(?:\s|$)/ }).count(),
      0,
      `${label} team ${index + 1} has no History accessibility target`,
    );
    assert.equal(
      await rail.locator('[title="History"], .team-mode-label:text-is("History")').count(),
      0,
      `${label} team ${index + 1} has no History label, control, or badge owner`,
    );
    assert.equal(
      await rail.locator('[role="tab"][aria-selected="true"]').count(),
      1,
      `${label} team ${index + 1} restores exactly one scoped selection`,
    );
  }
}

/** Prove History belongs to the global Canvas navigation and nowhere else. */
export async function assertGlobalHistoryNavigation(page, label) {
  const navigation = page.getByRole("navigation", { name: "Canvas views" });
  assert.equal(await navigation.count(), 1, `${label} renders one global Canvas navigation`);
  assert.deepEqual(
    await navigation.locator(".pill-label").allTextContents(),
    GLOBAL_DESTINATION_LABELS,
    `${label} preserves the complete global destination order`,
  );
  assert.equal(
    await navigation.getByRole("button", { name: /^History(?:\s|$)/ }).count(),
    1,
    `${label} exposes exactly one global History destination`,
  );
  assert.equal(
    await page.locator(".team-mode-rail").getByRole("tab", { name: /^History(?:\s|$)/ }).count(),
    0,
    `${label} exposes no team-scoped History destination`,
  );
}
