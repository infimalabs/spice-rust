import assert from "node:assert/strict";
import { basename } from "node:path";

const PIXEL = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=";

export async function assertActivityIdentityLabels(
  page,
  console,
  activityReplay,
  named,
  unnamed,
) {
  const ticker = console.locator(".ticker").first();
  for (const [member, detail] of [
    [named, "named activity identity witness"],
    [unnamed, "worktree activity identity witness"],
  ]) {
    const expected = member.name ?? basename(member.worktree);
    await activityReplay.injectAgentProse(page, member.lane, detail);
    await ticker.locator(".ticker-detail").filter({ hasText: detail }).waitFor();
    assert.equal(await ticker.locator(".ticker-prefix").innerText(), `${expected}:`);
    assert.equal(await ticker.locator(".ticker-detail").innerText(), detail);
    assert.equal(
      await ticker.evaluate((node) => node.scrollWidth <= node.clientWidth + 1),
      true,
      `${expected} remains inside the lineage ticker`,
    );
  }

  await activityReplay.injectAgentToolMedia(page, unnamed.lane, [{
    name: "activity.png",
    content_type: "image/png",
    data_url: `data:image/png;base64,${PIXEL}`,
  }], "cargo test completed");
  await ticker.filter({ hasText: "cargo test completed" }).waitFor();
  assert.equal(await ticker.locator(".ticker-prefix").innerText(), "cargo");
  assert.equal(await ticker.locator(".ticker-detail").innerText(), "test completed");
  assert.doesNotMatch(await ticker.innerText(), /(?:assistant|tool|command_execution):/);
}
