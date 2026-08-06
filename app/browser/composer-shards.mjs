import assert from "node:assert/strict";

import { TRANSITION_MS, waitForPageFunction } from "./canvas-fixture.mjs";

/// A draft shard shows the operator what they typed and nothing built from it:
/// no live Markdown render, no preview pane, no rendered quote. Counting zero
/// previews only means something once the shards are known to be there, so the
/// expected shard count and each editable draft are anchored first -- over a
/// surface that failed to render, every count would be zero and pass.
export async function assertNoDraftPreview(scope, expectedShards) {
  const shards = scope.locator(".composer-shard");
  assert.equal(
    await shards.count(),
    expectedShards,
    "the draft shards under test are present before their previews are counted",
  );
  for (let index = 0; index < expectedShards; index += 1) {
    const shard = shards.nth(index);
    assert.equal(
      await shard.getByRole("textbox").count(),
      1,
      `draft shard ${index} still edits its draft`,
    );
    assert.equal(
      await shard.getByLabel("Markdown preview").count(),
      0,
      `draft shard ${index} carries no Markdown preview`,
    );
    assert.equal(
      await shard.locator(".composer-markdown, .markdown-body, blockquote").count(),
      0,
      `draft shard ${index} renders no built Markdown`,
    );
  }
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
  latestPrimaryProse,
}) {
  const nestedCard = quoteActivity
    .filter({ hasText: nestedQuotePreview })
    .getByRole("button", { name: "Quote this message" });
  const quoteStack = quoteLane.locator(".agent-composer-stack");
  const quotePrimary = quoteStack.locator(".composer-shard--primary");
  const primaryHeading = team.locator(
    `.team-agent[data-agent-lane="${seeded.lane}"] .composer-shard--primary .composer-shard-heading > strong`,
  );
  assert.equal(
    await primaryHeading.innerText(),
    seeded.primaryProse,
    "COMPOSE-1kKbxl51: the seeded primary heading leads with its latest exact-lane agent prose",
  );
  assert.equal(
    await primaryHeading.getAttribute("title"),
    seeded.primaryProse,
    "COMPOSE-1kKbxl51: the complete seeded-lane prose survives one-line truncation",
  );
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
  assert.equal(
    await quotePrimary.locator(":scope > .agent-renew-request").count(),
    0,
    "the primary shard exposes no renewal action outside its hamburger drawer",
  );

  const quoteHeading = quotePrimary.locator(".composer-shard-heading");
  const quoteProse = quoteHeading.locator(".composer-shard-prose");
  assert.equal(
    await quoteProse.innerText(),
    latestPrimaryProse,
    "the primary header shows the latest exact-lane agent response",
  );
  assert.equal(
    await quoteProse.getAttribute("title"),
    latestPrimaryProse,
    "the complete prose remains available when the one-line header truncates it",
  );
  assert.notEqual(await quoteProse.innerText(), "Dee", "the header is prose-led, not agent-named");
  assert.deepEqual(
    await quoteHeading.locator(":scope > *").evaluateAll((children) =>
      children.map((child) => child.className),
    ),
    [
      "composer-shard-link composer-shard-age",
      "composer-shard-prose",
      "composer-shard-image-indicators",
      "agent-action-menu",
    ],
    "the primary header orders age, prose, image indicators, then its hamburger",
  );

  const activityChrome = await team
    .locator(".team-agent .composer-shard--primary .composer-shard-age")
    .evaluateAll((ages) => ages.map((age) => ({
      accent: age.closest(".team-agent")?.dataset.accent,
      color: getComputedStyle(age).color,
      label: age.textContent?.trim(),
    })));
  assert.ok(activityChrome.length >= 2, "multiple composer activity ages are visible");
  assert.ok(
    activityChrome.every((age) => /^(?:now|\d+[smhd])$/.test(age.label)),
    `every composer activity age is relative: ${JSON.stringify(activityChrome)}`,
  );
  assert.ok(
    new Set(activityChrome.map((age) => age.accent)).size >= 2,
    "the fixture exposes multiple composer identity accent slots",
  );
  assert.ok(
    new Set(activityChrome.map((age) => age.color)).size >= 2,
    "composer activity ages visibly retain their agents' identity colors",
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
    await previewShard.locator(".agent-renew-request").count(),
    0,
    "a quote-created shard carries no renewal action",
  );
  assert.equal(
    await quoteStack.locator(".composer-shard-link").count(),
    await quoteStack.locator(".composer-shard").count(),
    "every primary or quote-created shard carries a top-left deep link",
  );
  // Counting the links leaves the primary shard's own link unproven: pointed at
  // a bare "#" it still counts, so the target has to be followed the same way
  // the draft shard's is below.
  const primaryLink = quotePrimary.locator("header > .composer-shard-link:first-child");
  const primaryHref = await primaryLink.getAttribute("href");
  assert.equal(
    primaryHref,
    primarySources[1],
    "the primary shard retains this agent's proven stream destination",
  );
  await primaryLink.click();
  assert.equal(await page.evaluate(() => window.location.hash), primaryHref);
  const primaryAnchor = page.locator(primaryHref);
  // Counted rather than waited for, so a link pointing at an id nothing renders
  // names itself here instead of expiring as an anonymous locator timeout.
  assert.equal(
    await primaryAnchor.count(),
    1,
    "the primary deep link resolves to a live source card",
  );
  assert.equal(
    await primaryAnchor.evaluate((anchor) => anchor.matches(":target")),
    true,
    "following the primary deep link targets that source card",
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
  const drawer = quotePrimary.locator(".agent-action-drawer");
  const drawerRenewal = drawer.locator(".agent-renew-request");
  assert.equal(await drawer.isVisible(), false, "the agent action drawer starts closed");
  assert.equal(
    await drawerRenewal.isVisible(),
    false,
    "agent renewal is not visible while the hamburger drawer is closed",
  );
  await drawerToggle.click();
  await drawer.waitFor();
  assert.equal(await drawer.getAttribute("data-agent-drawer-target-id"), seeded.quoteLane);
  assert.deepEqual(
    (await drawer.getByRole("button").allInnerTexts()).map((text) => text.replaceAll("\n", " ")),
    ["Focus message editor", "Renew this agent request renewal"],
    "the hamburger opens actions scoped to this agent",
  );
  assert.equal(
    await team.locator(".team-pane").getAttribute("data-team-mode"),
    "Compose",
    "the agent drawer does not switch to the team Info panel",
  );
  assert.equal(await team.locator(".team-detail-pane").count(), 0);
  await drawerToggle.click();
  assert.equal(await drawer.isVisible(), false, "closing the hamburger hides its agent actions");
  assert.equal(
    await drawerRenewal.isVisible(),
    false,
    "renewal is exclusive to the open hamburger drawer",
  );

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
  assert.equal(await quoteLane.locator(".composer-shard--quote .agent-renew-request").count(), 0);
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
  await assertNoDraftPreview(quoteStack, 3);

  previewShard = quoteLane.locator(".composer-shard--quote").first();
  await previewShard.waitFor();
  previewLabel = await previewShard.locator("header strong").innerText();
  assert.equal(previewLabel, "quoted message", "an absent preview has a visible placeholder");
  assert.ok(previewLabel.trim(), "the absent-preview shard is not blank");
  assert.doesNotMatch(previewLabel, /[<>]/);
  assert.equal(await previewShard.getAttribute("title"), entirelyQuotedMessage);
}
