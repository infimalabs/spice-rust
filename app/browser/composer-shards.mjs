import assert from "node:assert/strict";

import { TRANSITION_MS, waitForPageFunction } from "./canvas-fixture.mjs";

/** Exercise the quote-created shard, source-link, drawer, and scroll contract. */
export async function assertComposerShards({
  page,
  team,
  quoteLane,
  quoteActivity,
  seeded,
  nestedQuotePreview,
  nestedQuoteMessage,
  entirelyQuotedMessage,
}) {
  const nestedCard = quoteActivity
    .filter({ hasText: nestedQuotePreview })
    .getByRole("button", { name: "Quote this message" });
  const quoteStack = quoteLane.locator(".agent-composer-stack");
  const quotePrimary = quoteStack.locator(".composer-shard--primary");
  assert.equal(
    await quoteStack.locator(".composer-shard").count(),
    1,
    "an agent begins with exactly one primary composer shard",
  );
  assert.equal(
    await quotePrimary.locator(".agent-action-menu").count(),
    1,
    "the primary shard owns the agent hamburger",
  );

  const primarySources = [];
  for (const lane of [seeded.lane, seeded.quoteLane]) {
    const primaryLink = team.locator(
      `.team-agent[data-agent-lane="${lane}"] .composer-shard--primary .composer-shard-link`,
    );
    const primaryHref = await primaryLink.getAttribute("href");
    assert.match(
      primaryHref,
      /^#message-(?:agent|directive)-/,
      "a primary shard with activity links to a real source card",
    );
    const primarySource = page.locator(primaryHref);
    await primarySource.waitFor({ timeout: TRANSITION_MS });
    assert.equal(
      await primarySource.getAttribute("data-agent-lane"),
      lane,
      "the primary shard target belongs to that exact agent",
    );
    await primaryLink.click();
    assert.equal(await page.evaluate(() => window.location.hash), primaryHref);
    assert.equal(
      await primarySource.evaluate((card) => card.matches(":target")),
      true,
      "clicking the primary deep link targets its resident stream card",
    );
    primarySources.push(primaryHref);
  }
  assert.notEqual(
    primarySources[0],
    primarySources[1],
    "two agents in one lineage have distinct primary stream destinations",
  );

  await nestedCard.click();
  let previewShard = quoteLane
    .locator(".composer-shard--quote")
    .filter({ hasText: nestedQuotePreview });
  await previewShard.waitFor();
  assert.equal(
    await quoteStack.locator(".composer-shard").count(),
    2,
    "quoting a card mints a second composer shard in that agent stack",
  );
  let previewLabel = await previewShard.locator("header strong").innerText();
  assert.equal(previewLabel, nestedQuotePreview);
  assert.doesNotMatch(previewLabel, /[<>]/, "the producer preview contains no quote marker");
  assert.equal(
    await previewShard.getAttribute("title"),
    nestedQuoteMessage,
    "the complete nested message remains reachable from the shard",
  );
  assert.equal(
    await previewShard.locator(".agent-action-menu").count(),
    0,
    "a quote-created shard carries no hamburger",
  );
  assert.equal(
    await quoteStack.locator(".composer-shard-link").count(),
    await quoteStack.locator(".composer-shard").count(),
    "every primary or quote-created shard carries a top-left deep link",
  );

  const sourceLink = previewShard.locator("header > .composer-shard-link:first-child");
  const sourceHref = await sourceLink.getAttribute("href");
  assert.match(
    sourceHref,
    /^#message-(?:agent|directive)-/,
    "the draft shard links to its exact source card",
  );
  await sourceLink.click();
  assert.equal(await page.evaluate(() => window.location.hash), sourceHref);
  const linkedSource = page.locator(sourceHref);
  await linkedSource.waitFor();
  assert.equal(
    await linkedSource.evaluate((card) => card.matches(":target")),
    true,
    "following the draft deep link targets the quoted message card",
  );

  const drawerToggle = quotePrimary.locator(".agent-action-menu > summary");
  await drawerToggle.click();
  const drawer = quotePrimary.locator(".agent-action-drawer");
  await drawer.waitFor();
  assert.equal(await drawer.getAttribute("data-agent-drawer-target-id"), seeded.quoteLane);
  assert.deepEqual(
    await drawer.getByRole("button").allTextContents(),
    ["Focus message editor", "Renew this agent"],
    "the hamburger opens actions scoped to this agent",
  );
  assert.equal(
    await team.locator(".team-pane").getAttribute("data-team-mode"),
    "Compose",
    "the agent drawer does not switch to the team Info panel",
  );
  assert.equal(await team.locator(".team-detail-pane").count(), 0);
  await drawerToggle.click();

  await nestedCard.click();
  assert.equal(
    await quoteLane.locator(".composer-shard--quote").count(),
    1,
    "quoting the same message identity reveals its existing shard",
  );
  const repeatedQuoteId = await previewShard.getByRole("textbox").getAttribute("id");
  await waitForPageFunction(
    page,
    "repeated quote focuses its existing reply",
    TRANSITION_MS,
    (id) => document.activeElement?.id === id,
    repeatedQuoteId,
  );
  assert.equal(
    await previewShard
      .getByRole("textbox")
      .evaluate((textarea) => document.activeElement === textarea),
    true,
    "repeated quote identity focuses the existing reply",
  );

  await quoteActivity
    .filter({ hasText: "Borrowed context has no authored preview." })
    .getByRole("button", { name: "Quote this message" })
    .click();
  assert.equal(
    await quoteLane.locator(".composer-shard--quote").count(),
    2,
    "two distinct messages retain two simultaneous quote shards",
  );
  assert.equal(
    await quoteLane.locator(".agent-action-menu").count(),
    1,
    "only the primary shard keeps a hamburger as quote shards accumulate",
  );
  assert.equal(await quoteLane.locator(".composer-shard--quote .agent-action-menu").count(), 0);
  const stackOverflow = await quoteStack.evaluate((stack) => ({
    clientHeight: stack.clientHeight,
    scrollHeight: stack.scrollHeight,
    overflowY: getComputedStyle(stack).overflowY,
    shardCount: stack.querySelectorAll(".composer-shard").length,
  }));
  assert.equal(stackOverflow.shardCount, 3);
  // The excess height must also be reachable. Clipping the stack instead of
  // scrolling it leaves the height reading below completely unchanged, so that
  // reading alone cannot tell a scrollable stack from a truncated one.
  assert.ok(
    ["auto", "scroll"].includes(stackOverflow.overflowY),
    `the agent stack scrolls its own overflow rather than clipping it: ${JSON.stringify(stackOverflow)}`,
  );
  assert.ok(
    stackOverflow.scrollHeight > stackOverflow.clientHeight,
    `multiple draft shards overflow the bounded agent stack: ${JSON.stringify(stackOverflow)}`,
  );

  previewShard = quoteLane.locator(".composer-shard--quote").first();
  await previewShard.waitFor();
  previewLabel = await previewShard.locator("header strong").innerText();
  assert.equal(previewLabel, "quoted message", "an absent preview has a visible placeholder");
  assert.ok(previewLabel.trim(), "the absent-preview shard is not blank");
  assert.doesNotMatch(previewLabel, /[<>]/);
  assert.equal(await previewShard.getAttribute("title"), entirelyQuotedMessage);
}
