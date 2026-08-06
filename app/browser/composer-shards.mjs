import assert from "node:assert/strict";

export async function assertComposerShardContract(
  page,
  team,
  lane,
  activity,
  { laneId, nestedQuotePreview, nestedQuoteMessage, entirelyQuotedMessage },
) {
  await assertAgentDrawerDistinct(team, lane);
  const nestedSource = activity.filter({ hasText: nestedQuotePreview });
  let previewShard = await assertQuoteMintsShard(
    page,
    lane,
    nestedSource,
    nestedQuotePreview,
  );
  let previewLabel = await previewShard.locator("header strong").innerText();
  assert.equal(previewLabel, nestedQuotePreview);
  assert.doesNotMatch(previewLabel, /[<>]/, "the producer preview contains no quote marker");
  assert.equal(
    await previewShard.getAttribute("title"),
    nestedQuoteMessage,
    "the complete nested message remains reachable from the shard",
  );
  await nestedSource.getByRole("button", { name: "Quote this message" }).click();
  assert.equal(
    await lane.locator(".composer-shard--draft").count(),
    1,
    "quoting the same message identity reveals its existing shard",
  );
  const repeatedQuoteId = await previewShard.getByRole("textbox").getAttribute("id");
  await page.waitForFunction(
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

  await activity
    .filter({ hasText: "Borrowed context has no authored preview." })
    .getByRole("button", { name: "Quote this message" })
    .click();
  assert.equal(
    await lane.locator(".composer-shard--draft").count(),
    2,
    "two distinct messages retain two simultaneous draft shards",
  );
  previewShard = lane.locator(".composer-shard--draft").first();
  await previewShard.waitFor();
  previewLabel = await previewShard.locator("header strong").innerText();
  assert.equal(previewLabel, "quoted message", "an absent preview has a visible placeholder");
  assert.ok(previewLabel.trim(), "the absent-preview shard is not blank");
  assert.doesNotMatch(previewLabel, /[<>]/);
  assert.equal(await previewShard.getAttribute("title"), entirelyQuotedMessage);

  const thirdSource = team
    .locator(
      `.mosaic-message-card[data-card-kind="agent-message"]` +
        `[data-producer-target-id="${laneId}"]`,
    )
    .first();
  await thirdSource.getByRole("button", { name: "Quote this message" }).click();
  await assertShardStackScrolls(lane, 3);
}

export async function assertAgentDrawerDistinct(team, lane) {
  const laneId = await lane.getAttribute("data-agent-lane");
  assert.ok(laneId, "the composer stack retains its exact agent lane");
  const primary = lane.locator(".composer-shard--primary");
  const drawer = primary.locator(".agent-composer-drawer");
  assert.equal(await drawer.count(), 1, "the primary shard owns one agent drawer");
  assert.equal(await drawer.getAttribute("open"), null, "the agent drawer starts closed");

  await drawer.locator("summary").click();
  assert.notEqual(await drawer.getAttribute("open"), null, "the hamburger opens its drawer");
  const panel = drawer.locator(".agent-composer-drawer-panel");
  await panel.waitFor();
  const buttons = panel.locator(":scope > button");
  assert.equal(await buttons.count(), 2, "the drawer exposes agent-specific actions");
  assert.deepEqual(
    await buttons.evaluateAll((nodes) => nodes.map((node) => node.dataset.agentTargetId)),
    [laneId, laneId],
    "every drawer action remains bound to the hamburger's agent",
  );
  assert.equal(
    await team.locator(".team-pane").getAttribute("data-team-mode"),
    "Compose",
    "opening the agent drawer leaves the team in Compose",
  );
  assert.equal(
    await team.locator(".team-detail-pane").count(),
    0,
    "the agent drawer is not the team or lane Info panel",
  );

  const primaryTextarea = primary.locator("textarea[data-primary-composer-target-id]");
  const primaryTextareaId = await primaryTextarea.getAttribute("id");
  assert.ok(primaryTextareaId, "the drawer's composer target has a stable element identity");
  await panel.locator(".agent-focus-composer").click();
  await team.page().waitForFunction(
    (id) => document.activeElement?.id === id,
    primaryTextareaId,
  );
  assert.equal(
    await primaryTextarea.evaluate((textarea) => document.activeElement === textarea),
    true,
    "the drawer's message action targets its own primary composer",
  );
  await drawer.locator("summary").click();
}

export async function assertQuoteMintsShard(page, lane, sourceCard, sourceLabel) {
  const sourceId = await sourceCard.getAttribute("id");
  assert.ok(sourceId, "the quoted stream card has a stable fragment identity");
  const shards = lane.locator(".composer-shard");
  const before = await shards.count();

  await sourceCard.getByRole("button", { name: "Quote this message" }).click();
  const draft = lane.locator(".composer-shard--draft").filter({ hasText: sourceLabel });
  await draft.waitFor();
  assert.equal(
    await shards.count(),
    before + 1,
    "quoting one stream card mints exactly one composer shard",
  );
  assert.equal(
    await lane.locator(".composer-shard--primary .agent-composer-drawer").count(),
    1,
    "the primary shard retains its hamburger",
  );
  assert.equal(
    await lane.locator(".composer-shard--draft .agent-composer-drawer").count(),
    0,
    "quoted draft shards render no hamburger",
  );

  const links = shards.locator(".composer-shard-heading > a");
  assert.equal(await links.count(), await shards.count(), "every composer shard has a top-left link");
  const hrefs = await links.evaluateAll((nodes) => nodes.map((node) => node.getAttribute("href")));
  for (const href of hrefs) {
    const target = href ?? "";
    assert.match(target, /^#[a-z0-9-]+$/, "each shard link is a local stream fragment");
    assert.equal(
      await page.locator(target).count(),
      1,
      `the shard stream target ${target} exists once`,
    );
  }

  const draftLink = draft.locator(".composer-shard-heading > a");
  assert.equal(await draftLink.getAttribute("href"), `#${sourceId}`);
  await draftLink.click();
  assert.equal(await page.evaluate(() => location.hash), `#${sourceId}`);
  assert.equal(
    await sourceCard.evaluate((card) => card.matches(":target")),
    true,
    "following a draft shard link reaches the quoted message card",
  );
  return draft;
}

export async function assertShardStackScrolls(lane, minimumDrafts) {
  assert.ok(
    await lane.locator(".composer-shard--draft").count() >= minimumDrafts,
    `the agent stack contains at least ${minimumDrafts} quoted draft shards`,
  );
  const geometry = await lane.evaluate((stack) => ({
    clientHeight: stack.clientHeight,
    scrollHeight: stack.scrollHeight,
  }));
  assert.ok(
    geometry.scrollHeight > geometry.clientHeight + 1,
    `the shard stack overflows vertically (${geometry.scrollHeight} > ${geometry.clientHeight})`,
  );
  const scroll = await lane.evaluate((stack) => {
    const style = getComputedStyle(stack);
    stack.scrollTop = stack.scrollHeight;
    return {
      clientHeight: stack.clientHeight,
      scrollHeight: stack.scrollHeight,
      scrollTop: stack.scrollTop,
      display: style.display,
      overflowX: style.overflowX,
      overflowY: style.overflowY,
      height: style.height,
      maxHeight: style.maxHeight,
      children: [...stack.children].map((child) => {
        const box = child.getBoundingClientRect();
        return { className: child.className, top: box.top, bottom: box.bottom, height: box.height };
      }),
    };
  });
  assert.ok(
    scroll.scrollTop > 0,
    `the agent stack scrolls through shards (${JSON.stringify(scroll)})`,
  );
  await lane.evaluate((stack) => stack.scrollTo(0, 0));
}
