import assert from "node:assert/strict";

import { TRANSITION_MS, waitForPageFunction } from "./canvas-fixture.mjs";

/// Production resolves one destination per primary shard, in one order
/// (`primary_stream_link` in app/src/browser/watchband.rs): the last rendered
/// card owned by that exact lane, and only when the lane has rendered no card
/// at all the explicitly named lineage-stream default. Both branches are read
/// back off the live DOM rather than spelled out here, because a hand-written
/// anchor id is precisely what went stale when that resolver landed.
async function primaryDestination(team, lane) {
  const link = team.locator(
    `.team-agent[data-agent-lane="${lane}"] .composer-shard--primary .composer-shard-link`,
  );
  const href = await link.getAttribute("href");
  const label = await link.getAttribute("aria-label");
  const rendered = await team
    .locator(`.team-activity-grid .mosaic-message-card[data-agent-lane="${lane}"]`)
    .evaluateAll((cards) => cards.map((card) => card.id));
  if (rendered.length === 0) {
    const stream = await team.locator(".team-activity-grid").getAttribute("id");
    assert.match(stream, /^lineage-stream-/, "the no-card default names the lineage stream");
    assert.equal(href, `#${stream}`, `${lane} renders no card and takes the lineage default`);
    assert.equal(label, "Jump to lineage activity", `${lane} announces the lineage default`);
    return { link, href, rendered };
  }
  assert.equal(
    href,
    `#${rendered.at(-1)}`,
    `${lane} links to the latest card it owns rather than merely one of them`,
  );
  assert.match(
    label,
    /^Jump to latest .+ activity$/,
    `${lane} announces its own latest activity, not the lineage default`,
  );
  return { link, href, rendered };
}

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
    const { link, href, rendered } = await primaryDestination(team, lane);
    assert.ok(rendered.length > 1, `${lane} owns several cards, so "latest" is a real choice`);
    assert.match(
      href,
      /^#message-(?:agent|directive)-/,
      "a primary shard with activity links to a real source card",
    );
    const primarySource = page.locator(href);
    // Counted rather than waited for, so a destination nothing renders names
    // this link here instead of expiring as an anonymous locator timeout.
    assert.equal(
      await primarySource.count(),
      1,
      `the ${lane} primary deep link resolves to exactly one rendered card`,
    );
    assert.equal(
      await primarySource.getAttribute("data-agent-lane"),
      lane,
      "the primary shard target belongs to that exact agent",
    );
    await link.click();
    assert.equal(await page.evaluate(() => window.location.hash), href);
    assert.equal(
      await primarySource.evaluate((card) => card.matches(":target")),
      true,
      "clicking the primary deep link targets its resident stream card",
    );
    primarySources.push(href);
  }
  assert.notEqual(
    primarySources[0],
    primarySources[1],
    "two agents in one lineage have distinct primary stream destinations",
  );
  // The two lanes above are the ones with seeded conversation. Sweeping the
  // rest holds every seated agent to the same resolver, so an agent that has
  // rendered no card of its own is held to the lineage default here rather
  // than going unmeasured.
  const seatedLanes = await team
    .locator(".team-agent")
    .evaluateAll((agents) => agents.map((agent) => agent.dataset.agentLane));
  assert.ok(seatedLanes.length > 2, "the lineage seats more agents than the two proven above");
  for (const lane of seatedLanes) await primaryDestination(team, lane);

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
