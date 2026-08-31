import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import {
  reply,
  TRANSITION_MS,
  waitForPageFunction,
} from "./canvas-fixture.mjs";

/// Production resolves one destination per primary shard, in one order
/// (`primary_stream_link` in app/src/browser/watchband.rs): the last rendered
/// prose card owned by that exact lane, then the lineage's last prose card,
/// and only for a lineage with no message card the named stream default. Every
/// branch is read back off the live DOM because a hand-written anchor id is
/// precisely what went stale when that resolver landed.
export async function primaryDestination(team, lane) {
  const member = team.locator(`.team-agent[data-agent-lane="${lane}"]`);
  const surface = member.locator("xpath=ancestor::*[contains(@class, 'team-surface')][1]");
  const link = member.locator(".composer-shard--primary .composer-shard-link");
  const href = await link.getAttribute("href");
  const label = await link.getAttribute("aria-label");
  const rendered = await surface
    .locator(
      `.team-activity-grid .mosaic-message-card[data-agent-lane="${lane}"]`
        + '[data-card-kind="agent-message"]',
    )
    .evaluateAll((cards) => cards.map((card) => card.id));
  const lineageMessages = await surface.locator(
    '.team-activity-grid .mosaic-message-card[data-card-kind="agent-message"]',
  ).evaluateAll((cards) => cards.map((card) => card.id));
  if (rendered.length === 0) {
    if (lineageMessages.length === 0) {
      const stream = await surface.locator(".team-activity-grid").getAttribute("id");
      assert.equal(href, `#${stream}`);
      assert.equal(label, "Jump to lineage activity");
      assert.equal((await link.innerText()).trim(), "—", "only an empty stream renders a dash");
      return { link, href, rendered, lineageMessages };
    }
    assert.equal(href, `#${lineageMessages.at(-1)}`);
    assert.equal(label, "Jump to latest lineage message");
    assert.notEqual((await link.innerText()).trim(), "—");
    return { link, href, rendered, lineageMessages };
  }
  assert.equal(
    href,
    `#${rendered.at(-1)}`,
    `${lane} links to its latest agent message rather than another owned card`,
  );
  assert.match(
    label,
    /^Jump to latest .+ message$/,
    `${lane} announces its own latest prose message, not the lineage default`,
  );
  assert.notEqual((await link.innerText()).trim(), "—");
  return { link, href, rendered, lineageMessages };
}

/// A valid wire message may carry only a synthetic ordering instant. Replace
/// the current message snapshot with exactly that case so one composer selects
/// it as its own source and another must select it through lineage fallback.
/// Both headers and the card must expose one finite display chronology without
/// rewriting the frame as vendor-authored or Spice-observed.
export async function assertSyntheticComposerObservation(
  page,
  team,
  activityReplay,
  ownLane,
  fallbackLane,
) {
  assert.notEqual(ownLane, fallbackLane, "own and lineage-fallback composers are distinct");
  const text = `synthetic display chronology ${crypto.randomUUID()}`;
  await team.evaluate((surface, lanes) => {
    window.__syntheticComposerStates = [];
    const capture = () => {
      for (const lane of lanes) {
        const link = surface.querySelector(
          `.team-agent[data-agent-lane="${lane}"] .composer-shard--primary .composer-shard-age`,
        );
        const href = link?.getAttribute("href");
        const card = href?.startsWith("#") ? document.getElementById(href.slice(1)) : null;
        window.__syntheticComposerStates.push({
          lane,
          href,
          headerDisplay: link?.querySelector("[data-age-authored]")?.dataset.ageAuthored,
          cardDisplay: card?.querySelector("[data-age-authored]")?.dataset.ageAuthored,
        });
      }
    };
    capture();
    window.__syntheticComposerObserver = new MutationObserver(capture);
    window.__syntheticComposerObserver.observe(surface, {
      attributes: true,
      childList: true,
      subtree: true,
    });
  }, [ownLane, fallbackLane]);

  try {
    const witness = await activityReplay.injectSyntheticSnapshot(page, ownLane, text);
    const card = team.locator('[data-card-kind="agent-message"]').filter({ hasText: text });
    const cardId = await card.getAttribute("id");
    const expectedDisplay = Date.parse(witness.authored).toString();
    const cardDisplay = await card.locator("[data-age-authored]").getAttribute("data-age-authored");
    assert.equal(cardDisplay, expectedDisplay, "the card displays its stable synthetic instant");
    assert.ok(Number.isFinite(Number(cardDisplay)), "the synthetic card display time is finite");

    const states = [];
    for (const [lane, source] of [[ownLane, "own-agent"], [fallbackLane, "lineage fallback"]]) {
      const composer = team.locator(`.team-agent[data-agent-lane="${lane}"]`);
      const link = composer.locator(".composer-shard--primary .composer-shard-age");
      const age = link.locator("[data-age-authored]");
      const state = {
        source,
        href: await link.getAttribute("href"),
        label: (await age.innerText()).trim(),
        display: await age.getAttribute("data-age-authored"),
        prose: (await composer.locator(".composer-shard-prose").innerText()).trim(),
      };
      states.push(state);
      assert.equal(state.href, `#${cardId}`, `${source} header links to the synthetic card`);
      assert.equal(state.display, cardDisplay, `${source} header uses the linked card timestamp`);
      assert.notEqual(state.label, "—", `${source} header never renders a dash`);
      assert.match(state.label, /^(?:now|\d+[smhd])$/, `${source} header renders relative time`);
      assert.equal(state.prose, text, `${source} header prose comes from the same card`);
    }
    assert.deepEqual(
      states.map(({ href, display }) => ({ href, display })),
      Array.from({ length: 2 }, () => ({ href: `#${cardId}`, display: cardDisplay })),
      "own-agent and lineage fallback select one card and one display instant",
    );
    await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
      () => requestAnimationFrame(resolve),
    )));
    const observedStates = await page.evaluate(() => window.__syntheticComposerStates);
    assert.ok(observedStates.length >= 2, "both synthetic composer paths were observed");
    assert.ok(
      observedStates.every((state) => state.headerDisplay === state.cardDisplay),
      `no rendered header mixes a card link with another display time: ${
        JSON.stringify(observedStates)
      }`,
    );
  } finally {
    await page.evaluate(() => window.__syntheticComposerObserver?.disconnect());
    await activityReplay.restoreProseSnapshot(page);
  }
}

async function appendPrimaryDestinationProbe(team, fixture, lane, ordinal) {
  const text = `primary destination ordering witness ${ordinal} ${crypto.randomUUID()}`;
  await reply(fixture, ["steer", lane, text]);
  const inbox = await reply(fixture, ["inbox", lane]);
  const pending = inbox.body.lanes[0].pending.find((directive) => directive.text === text);
  assert.ok(pending, `the primary destination probe reached ${lane}`);
  assert.equal(
    await team.locator('[data-card-kind="operator-message"]').count(),
    0,
    "a newer direction never creates a standalone source card",
  );
  assert.equal(
    await team.locator(".mosaic-message-card").filter({ hasText: text }).count(),
    0,
    "unacknowledged direction text remains outside the activity plane",
  );
  return { directive: pending.directive };
}

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

export async function assertComposerTypingDoesNotRemeasureActivity(page, lane, composers) {
  const before = await activityMeasurementSnapshot(page, lane, "before typing");
  for (const [index, composer] of composers.entries()) {
    await composer
      .getByRole("textbox")
      .fill(`composer cold-plane witness ${index} ${crypto.randomUUID()}`);
  }
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  const typed = await activityMeasurementSnapshot(page, lane, "after typing", before.token);
  assert.deepEqual(
    typed,
    before,
    "typing in draft composers does not rerender or remeasure the Mosaic activity plane",
  );
  for (const composer of composers) {
    await composer.getByRole("textbox").fill("");
  }
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  const cleared = await activityMeasurementSnapshot(page, lane, "after clearing", before.token);
  assert.deepEqual(
    cleared,
    before,
    "clearing draft composers also leaves the Mosaic activity plane cold",
  );
}

export async function assertComposerPointerSelection(page, lane, composer) {
  const textarea = composer.getByRole("textbox");
  const text = "alpha bravo charlie delta echo foxtrot";
  await textarea.fill(text);
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  const before = await activityMeasurementSnapshot(page, lane, "before pointer selection");
  const geometry = await textarea.evaluate((node) => {
    const style = getComputedStyle(node);
    const context = document.createElement("canvas").getContext("2d");
    context.font = style.font;
    const rect = node.getBoundingClientRect();
    const origin = rect.left + parseFloat(style.borderLeftWidth) + parseFloat(style.paddingLeft);
    const y = rect.top + parseFloat(style.borderTopWidth) + parseFloat(style.paddingTop)
      + parseFloat(style.lineHeight) / 2;
    const x = (offset) => origin + context.measureText(node.value.slice(0, offset)).width;
    return { y, alpha: x(2), bravo: x(8), charlie: x(15), end: x(36) };
  });
  const dragScope = await textarea.evaluate((node) => {
    window.__composerPointerEvents = [];
    for (const type of ["pointerdown", "mousedown", "click", "dblclick", "select", "pointerup"]) {
      node.addEventListener(type, () => window.__composerPointerEvents.push(type));
    }
    return {
      draggable: node.getAttribute("draggable"),
      draggableAncestor: node.closest('[draggable="true"]')?.className ?? null,
    };
  });
  assert.deepEqual(
    dragScope,
    { draggable: null, draggableAncestor: null },
    "the editable composer body is outside the native lane drag source",
  );

  await textarea.focus();
  await page.keyboard.press("ArrowLeft");
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  assert.deepEqual(
    await textarea.evaluate((node) => [node.selectionStart, node.selectionEnd]),
    [text.length - 1, text.length - 1],
    "keyboard navigation retains the composer caret",
  );
  await page.keyboard.press("Shift+ArrowLeft");
  assert.deepEqual(
    await textarea.evaluate((node) => ({
      range: [node.selectionStart, node.selectionEnd],
      text: node.value.slice(node.selectionStart, node.selectionEnd),
    })),
    {
      range: [text.length - 2, text.length - 1],
      text: text.slice(text.length - 2, text.length - 1),
    },
    "keyboard selection retains its native range",
  );

  await page.mouse.click(geometry.alpha, geometry.y);
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  const openingClick = await textarea.evaluate((node) => [node.selectionStart, node.selectionEnd]);
  assert.ok(
    openingClick[0] <= 4 && openingClick[0] === openingClick[1],
    `single click retains an opening caret: ${JSON.stringify(openingClick)}`,
  );

  await page.mouse.click(geometry.charlie, geometry.y);
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  const clicked = await textarea.evaluate((node) => [node.selectionStart, node.selectionEnd]);
  assert.ok(
    clicked[0] >= 12 && clicked[0] <= 18 && clicked[0] === clicked[1],
    `single click retains a caret near the clicked character: ${JSON.stringify(clicked)}`,
  );
  await page.keyboard.type("x");
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  assert.equal(
    await textarea.inputValue(),
    `${text.slice(0, clicked[0])}x${text.slice(clicked[0])}`,
    "ordinary typing remains at the pointer-positioned caret",
  );
  await textarea.fill(text);
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));

  await page.mouse.dblclick(geometry.bravo, geometry.y);
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  assert.equal(
    await textarea.evaluate((node) => node.value.slice(node.selectionStart, node.selectionEnd)),
    "bravo",
    "double click retains the clicked word selection",
  );

  await page.mouse.move(geometry.alpha, geometry.y);
  await page.mouse.down();
  await page.mouse.move(geometry.end, geometry.y, { steps: 12 });
  await page.mouse.up();
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
    () => requestAnimationFrame(resolve),
  )));
  const dragged = await textarea.evaluate((node) => ({
    start: node.selectionStart,
    end: node.selectionEnd,
    text: node.value.slice(node.selectionStart, node.selectionEnd),
  }));
  assert.ok(
    dragged.start <= 3 && dragged.end >= 34,
    `pointer drag retains the intended composer range: ${JSON.stringify(dragged)}`,
  );

  assert.deepEqual(
    await activityMeasurementSnapshot(
      page,
      lane,
      "after pointer selection",
      before.token,
    ),
    before,
    "composer selection gestures leave the Mosaic activity plane cold",
  );
  const events = await page.evaluate(() => window.__composerPointerEvents);
  for (const event of ["pointerdown", "mousedown", "click", "dblclick", "select", "pointerup"]) {
    assert.ok(events.includes(event), `the composer receives ${event}: ${JSON.stringify(events)}`);
  }
  await textarea.fill("");
}

async function activityMeasurementSnapshot(page, lane, label, expectedToken = null) {
  await page.waitForFunction(
    (target) => {
      const member = document.querySelector(`.team-agent[data-agent-lane="${target}"]`);
      const plane = member?.closest(".team-surface")?.querySelector(".lane-mosaic-plane");
      return plane?.dataset.measurementPhase === "ready";
    },
    await lane.getAttribute("data-agent-lane"),
  );
  const snapshot = await lane.evaluate((node, expected) => {
    const surface = node.closest(".team-surface");
    const plane = surface?.querySelector(".lane-mosaic-plane");
    if (!plane) return null;
    const token = expected ?? crypto.randomUUID();
    const cards = [...plane.querySelectorAll(".mosaic-message-card")];
    let retained = true;
    for (const card of cards) {
      if (expected && card.__mosaicTypingProbe !== token) retained = false;
      card.__mosaicTypingProbe = token;
    }
    return {
      token,
      retained,
      phase: plane.dataset.measurementPhase,
      pending: plane.dataset.measurementPending,
      commits: plane.dataset.planeCommits,
      cards: cards.map((card, order) => {
        const box = card.getBoundingClientRect();
        return {
          key: card.dataset.mosaicCardKey,
          order,
          phase: card.dataset.layoutPhase,
          lifecycle: card.classList.contains("frozen") ? "frozen" : "liquid",
          track: card.dataset.track,
          span: card.dataset.span,
          base: card.dataset.base,
          row: card.dataset.row,
          rows: card.dataset.rows,
          reads: card.dataset.measurementReads,
          heights: card.dataset.measuredHeights,
          spans: card.dataset.candidateSpans,
          box: { x: box.x, y: box.y, width: box.width, height: box.height },
        };
      }),
    };
  }, expectedToken);
  assert.ok(snapshot, `${label}: activity plane is present`);
  assert.ok(snapshot.cards.length > 0, `${label}: activity plane carries cards`);
  assert.ok(snapshot.retained, `${label}: every keyed activity keeps its DOM node`);
  return snapshot;
}

async function assertHorizontalComposerStrip(page, team) {
  const viewport = page.viewportSize();
  assert.ok(viewport, "the composer-strip fixture has a viewport to restore");
  const strip = team.locator(".team-surface .team-agents").first();
  try {
    await page.setViewportSize({ width: 520, height: viewport.height });
    const geometry = await strip.evaluate((node) => {
      const box = node.getBoundingClientRect();
      return {
        bodyWidth: document.body.scrollWidth,
        viewportWidth: innerWidth,
        left: box.left,
        right: box.right,
        clientWidth: node.clientWidth,
        scrollWidth: node.scrollWidth,
        overflowX: getComputedStyle(node).overflowX,
        columns: [...node.querySelectorAll(":scope > .team-agent")].map((agent) => {
          const column = agent.getBoundingClientRect();
          return { left: column.left, right: column.right, width: column.width };
        }),
      };
    });
    assert.ok(
      ["auto", "scroll"].includes(geometry.overflowX),
      `the primary composer row owns horizontal scrolling: ${JSON.stringify(geometry)}`,
    );
    assert.ok(
      geometry.scrollWidth > geometry.clientWidth,
      `primary composers overflow their strip instead of compressing: ${JSON.stringify(geometry)}`,
    );
    assert.ok(
      geometry.columns.every((column) => column.width >= 159),
      "every narrow-viewport composer retains its usable minimum width: "
        + JSON.stringify(geometry.columns),
    );
    assert.ok(
      geometry.bodyWidth <= geometry.viewportWidth,
      `composer overflow stays inside its own strip: ${JSON.stringify(geometry)}`,
    );

    await strip.evaluate((node) => node.scrollTo({ left: node.scrollWidth, behavior: "instant" }));
    await waitForPageFunction(
      page,
      "the primary composer strip scrolls east",
      TRANSITION_MS,
      () => document.querySelector(".team-surface .team-agents")?.scrollLeft > 0,
    );
    const eastern = await strip.evaluate((node) => {
      const stripBox = node.getBoundingClientRect();
      const last = node.querySelector(":scope > .team-agent:last-child")?.getBoundingClientRect();
      return {
        scrollLeft: node.scrollLeft,
        stripLeft: stripBox.left,
        stripRight: stripBox.right,
        last: last && { left: last.left, right: last.right },
      };
    });
    assert.ok(eastern.scrollLeft > 0, "the strip records an eastward scroll position");
    assert.ok(
      eastern.last &&
        eastern.last.left >= eastern.stripLeft - 1 &&
        eastern.last.right <= eastern.stripRight + 1,
      `eastward scrolling reveals the final primary composer: ${JSON.stringify(eastern)}`,
    );
    await strip.evaluate((node) => node.scrollTo({ left: 0, behavior: "instant" }));
  } finally {
    await page.setViewportSize(viewport);
  }
}

/** Exercise the quote-created shard, source-link, drawer, and scroll contract. */
export async function assertComposerShards({
  page,
  fixture,
  team,
  quoteLane,
  quoteActivity,
  seeded,
  nestedQuotePreview,
  nestedQuoteMessage,
  entirelyQuotedMessage,
  latestPrimaryProse,
  activityReplay,
}) {
  const nestedCard = quoteActivity
    .filter({ hasText: nestedQuotePreview })
    .getByRole("button", { name: "Quote this message" });
  const quoteStack = quoteLane.locator(".agent-composer-stack");
  const quotePrimary = quoteStack.locator(".composer-shard--primary");
  const initialQuoteLaneFrame = await quoteLane.evaluate((lane) => {
    const box = lane.getBoundingClientRect();
    return { width: box.width, height: box.height };
  });
  const primaryShard = team.locator(
    `.team-agent[data-agent-lane="${seeded.lane}"] .composer-shard--primary`,
  );
  // Located by the class the heading wears rather than as a strong under the
  // header: the renew control lives in this header's drawer and carries a
  // strong of its own, so a descendant tag match resolves to two elements.
  const primaryHeadings = team.locator(
    ".team-agent .composer-shard--primary .composer-shard-heading > .composer-shard-prose",
  );
  assert.equal(
    await primaryHeadings.count(),
    seeded.primaryHeadings.length,
    "every seated lane exposes one primary prose heading",
  );
  // Every lane's agent names itself by its own worktree basename, so each
  // expectation below is a different string and the sweep fails if one lane's
  // standing is rendered into another lane's heading.
  const observedHeadings = [];
  for (const expected of seeded.primaryHeadings) {
    const heading = team.locator(
      `.team-agent[data-agent-lane="${expected.lane}"] `
        + ".composer-shard--primary .composer-shard-heading > .composer-shard-prose",
    );
    const prose = await heading.innerText();
    assert.equal(
      prose,
      expected.prose,
      `COMPOSE-1kKbxl51: the ${expected.lane} primary heading keeps its latest exact-lane prose`,
    );
    assert.equal(
      await heading.getAttribute("title"),
      expected.prose,
      `COMPOSE-1kKbxl51: the ${expected.lane} primary heading title keeps its complete prose`,
    );
    observedHeadings.push(prose);
  }
  assert.equal(
    new Set(observedHeadings).size,
    seeded.primaryHeadings.length,
    "primary prose headings remain pairwise distinct across seated lanes",
  );
  // The agent name is required to stay discoverable from this heading rather
  // than dropped, and it still is -- not as the heading text, which the
  // reference shows as prose, but as the label of the control the heading ends
  // with, so the name is reachable without leaving the header.
  assert.equal(
    await primaryShard.locator(".agent-action-menu > summary").getAttribute("aria-label"),
    `Actions for ${seeded.agentName}`,
    "the agent name stays discoverable from the heading, in its actions control",
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
  await assertHorizontalComposerStrip(page, team);

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
  const headingChildren = await quoteHeading.locator(":scope > *").evaluateAll((children) =>
    children.map((child) => ({
      className: child.className,
      hasAge: child.classList.contains("composer-shard-age"),
      hasLink: child.classList.contains("composer-shard-link"),
    })),
  );
  assert.deepEqual(
    headingChildren.map((child) =>
      child.className.replace(/ (?:queued|running|running-stale|bound|unbound)$/, "")
    ),
    [
      "composer-shard-link composer-shard-age",
      "composer-shard-prose",
      "composer-shard-image-indicators",
      "agent-action-menu",
    ],
    "the primary header orders age, prose, image indicators, then its hamburger",
  );
  assert.ok(
    headingChildren[0].hasAge && headingChildren[0].hasLink,
    `the primary header age keeps its required link and age classes: ${
      JSON.stringify(headingChildren)
    }`,
  );

  const activityChrome = await team
    .locator(".team-agent .composer-shard--primary .composer-shard-age")
    .evaluateAll((ages) => ages.map((age) => {
      const lane = age.closest(".team-agent")?.dataset.agentLane;
      const pip = document.querySelector(`.team-light[data-lane-light-target-id="${lane}"]`);
      return {
        label: age.textContent?.trim(),
        status: age.getAttribute("data-lane-status"),
        color: getComputedStyle(age).color,
        pipMissing: !pip,
        pipColor: pip ? getComputedStyle(pip).backgroundColor : null,
        pipStatus: pip?.getAttribute("data-lane-status"),
        authored: age.querySelector(".composer-shard-age-label")?.dataset.ageAuthored,
        clock: age.querySelector(".composer-shard-age-label")?.dataset.ageClock,
      };
    }));
  assert.ok(activityChrome.length >= 2, "multiple composer activity ages are visible");
  assert.ok(
    activityChrome.every((age) => /^(?:now|\d+[smhd])$/.test(age.label)),
    `every composer activity age comes from a selected message card: ${
      JSON.stringify(activityChrome)
    }`,
  );
  assert.ok(
    activityChrome.every((age) => age.label !== "—" && Number.isFinite(Number(age.authored))),
    `every composer in the nonempty lineage carries a message instant: ${
      JSON.stringify(activityChrome)
    }`,
  );
  assert.ok(
    activityChrome.every((age) => age.clock === "deadline"),
    `every message-backed composer age advances on the shared browser clock: ${
      JSON.stringify(activityChrome)
    }`,
  );
  const linkedHeaderAges = await team
    .locator(".team-agent .composer-shard--primary .composer-shard-age")
    .evaluateAll((ages) => ages.map((age) => {
      const href = age.getAttribute("href");
      const card = href?.startsWith("#") ? document.getElementById(href.slice(1)) : null;
      return {
        href,
        headerAuthored: age.querySelector(".composer-shard-age-label")?.dataset.ageAuthored,
        cardAuthored: card?.querySelector("[data-age-authored]")?.dataset.ageAuthored,
        cardKind: card?.getAttribute("data-card-kind"),
      };
    }).filter((age) => age.cardKind === "agent-message"));
  assert.ok(
    linkedHeaderAges.length > 0,
    `at least one composer heading links to a user-facing agent card: ${
      JSON.stringify(linkedHeaderAges)
    }`,
  );
  assert.ok(
    linkedHeaderAges.every((age) => age.headerAuthored === age.cardAuthored),
    `composer heading timestamps come from their linked latest message card in every lane state: ${
      JSON.stringify(linkedHeaderAges)
    }`,
  );
  assert.ok(
    activityChrome.every((age) => !age.pipMissing),
    `each composer activity age has a matching pip: ${JSON.stringify(activityChrome)}`,
  );
  assert.ok(
    activityChrome.every((age) => age.status === age.pipStatus),
    `composer activity ages expose the same status as their pip: ${JSON.stringify(activityChrome)}`,
  );
  assert.ok(
    activityChrome.every((age) => age.color === age.pipColor),
    `composer activity age color follows the corresponding pip: ${JSON.stringify(activityChrome)}`,
  );

  const primarySources = [];
  const destinationLanes = [seeded.lane, seeded.quoteLane];
  const destinationProbes = [];
  for (const [index, lane] of destinationLanes.entries()) {
    destinationProbes.push(await appendPrimaryDestinationProbe(team, fixture, lane, index + 1));
  }
  for (const lane of destinationLanes) {
    const { link, href, rendered } = await primaryDestination(team, lane);
    assert.ok(rendered.length, `${lane} owns an agent message before its newer directive`);
    assert.match(
      href,
      /^#message-agent-/,
      "a primary shard with agent prose links to a real agent-message source card",
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
    assert.equal(
      await primarySource.getAttribute("data-card-kind"),
      "agent-message",
      "a newer directive never displaces the primary shard's message destination",
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
  for (const probe of destinationProbes) {
    await reply(fixture, ["steer", "cancel", probe.directive]);
  }

  const latestAuthoredQuoteWitness = await activityReplay.injectAgentProse(
    page,
    seeded.quoteLane,
    nestedQuoteMessage,
  );
  await activityReplay.injectAgentProse(
    page,
    seeded.quoteLane,
    entirelyQuotedMessage,
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
  const verticalStack = await quoteStack.evaluate((stack) => {
    const primary = stack.querySelector(".composer-shard--primary")?.getBoundingClientRect();
    const quoteShard = stack.querySelector(".composer-shard--quote");
    const quote = quoteShard?.getBoundingClientRect();
    return {
      owner: stack.dataset.agentComposerStack,
      primary: primary && {
        top: primary.top,
        left: primary.left,
        right: primary.right,
        width: primary.width,
        height: primary.height,
      },
      quote: quote && {
        top: quote.top,
        bottom: quote.bottom,
        left: quote.left,
        right: quote.right,
        width: quote.width,
        height: quote.height,
        owner: quoteShard.closest(".team-agent")?.dataset.agentLane,
        target: quoteShard.dataset.quoteTargetId,
      },
    };
  });
  assert.ok(
    verticalStack.quote &&
      verticalStack.primary &&
      verticalStack.quote.bottom <= verticalStack.primary.top + 1,
    `the quoted draft stacks above its owning primary: ${JSON.stringify(verticalStack)}`,
  );
  assert.deepEqual(
    [verticalStack.quote?.owner, verticalStack.quote?.target],
    [verticalStack.owner, verticalStack.owner],
    "the vertical quote shard remains visibly and structurally owned by its agent",
  );
  assert.ok(
    Math.abs(verticalStack.quote.left - verticalStack.primary.left) < 1 &&
      Math.abs(verticalStack.quote.right - verticalStack.primary.right) < 1,
    `the quote and primary retain one aligned ownership column: ${JSON.stringify(verticalStack)}`,
  );
  assert.ok(
    Math.abs(verticalStack.quote.width - verticalStack.primary.width) < 1 &&
      Math.abs(verticalStack.quote.height - verticalStack.primary.height) < 1,
    `the quote and primary use the same fixed outer frame: ${JSON.stringify(verticalStack)}`,
  );
  let previewLabel = await previewShard.locator("header strong").innerText();
  assert.equal(previewLabel, nestedQuotePreview);
  assert.doesNotMatch(previewLabel, /[<>]/, "the producer preview contains no quote marker");
  assert.equal(
    await previewShard.getAttribute("title"),
    nestedQuoteMessage,
    "the complete nested message remains reachable from the shard",
  );
  const overflowingReply = Array.from(
    { length: 16 },
    (_, index) => `independently scrolling quote reply line ${index + 1}`,
  ).join("\n");
  const overflowingTextarea = previewShard.getByRole("textbox");
  const quoteFrameBeforeOverflow = await previewShard.evaluate((shard) => {
    const box = shard.getBoundingClientRect();
    return { width: box.width, height: box.height };
  });
  await overflowingTextarea.fill(overflowingReply);
  const quoteOverflow = await previewShard.evaluate((shard) => {
    const box = shard.getBoundingClientRect();
    const textarea = shard.querySelector("textarea");
    return {
      frame: { width: box.width, height: box.height },
      textarea: {
        clientHeight: textarea.clientHeight,
        scrollHeight: textarea.scrollHeight,
        overflowY: getComputedStyle(textarea).overflowY,
      },
    };
  });
  assert.deepEqual(
    quoteOverflow.frame,
    quoteFrameBeforeOverflow,
    "overflowing reply content does not resize its quote composer frame",
  );
  assert.ok(
    ["auto", "scroll"].includes(quoteOverflow.textarea.overflowY) &&
      quoteOverflow.textarea.scrollHeight > quoteOverflow.textarea.clientHeight,
    `the overflowing quote reply scrolls inside its fixed composer: ${JSON.stringify(quoteOverflow)}`,
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
  const latestAuthoredQuoteCard = team.locator(
    `[data-mosaic-card-key="${latestAuthoredQuoteWitness.key}"]`,
  );
  const latestAuthoredQuoteCardId = await latestAuthoredQuoteCard.getAttribute("id");
  assert.equal(
    primaryHref,
    `#${latestAuthoredQuoteCardId}`,
    "quote-only borrowed context does not displace the latest authored assistant message",
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

  const arrivalPrimary = team.locator(
    `.team-agent[data-agent-lane="${seeded.lane}"] .composer-shard--primary`,
  );
  const previousArrival = await arrivalPrimary.locator(".composer-shard-age").evaluate((link) => ({
    href: link.getAttribute("href"),
    authored: link.querySelector("[data-age-authored]")?.dataset.ageAuthored,
  }));
  await arrivalPrimary.evaluate((primary) => {
    window.__composerProvenanceStates = [];
    const capture = () => {
      const link = primary.querySelector(".composer-shard-age");
      const href = link?.getAttribute("href");
      const card = href?.startsWith("#") ? document.getElementById(href.slice(1)) : null;
      window.__composerProvenanceStates.push({
        href,
        headerAuthored: link?.querySelector("[data-age-authored]")?.dataset.ageAuthored,
        cardAuthored: card?.querySelector("[data-age-authored]")?.dataset.ageAuthored,
      });
    };
    capture();
    new MutationObserver(capture).observe(primary, {
      attributes: true,
      childList: true,
      subtree: true,
    });
  });
  const arrivalCards = team.locator(
    `.mosaic-message-card[data-agent-lane="${seeded.lane}"]`
      + '[data-card-kind="agent-message"]',
  );
  const arrivalOrdinal = await arrivalCards.count();
  await writeFile(join(seeded.teamWorktrees[0], ".spice-prose-trigger"), "emit\n");
  const arrivalCard = arrivalCards.nth(arrivalOrdinal);
  await arrivalCard.waitFor({ timeout: 10_000 });
  assert.match(await arrivalCard.innerText(), /newest lane prose witness/);
  await waitForPageFunction(
    page,
    "the primary composer atomically retargets to the new message provenance",
    TRANSITION_MS,
    ({ lane, previous }) => {
      const link = document.querySelector(
        `.team-agent[data-agent-lane="${lane}"] .composer-shard--primary .composer-shard-age`,
      );
      const href = link?.getAttribute("href");
      const card = href?.startsWith("#") ? document.getElementById(href.slice(1)) : null;
      return href !== previous
        && link?.querySelector("[data-age-authored]")?.dataset.ageAuthored
          === card?.querySelector("[data-age-authored]")?.dataset.ageAuthored;
    },
    { lane: seeded.lane, previous: previousArrival.href },
  );
  const arrived = await arrivalPrimary.locator(".composer-shard-age").evaluate((link) => {
    const href = link.getAttribute("href");
    const card = href?.startsWith("#") ? document.getElementById(href.slice(1)) : null;
    const age = link.querySelector("[data-age-authored]");
    return {
      href,
      label: age?.textContent?.trim(),
      headerAuthored: age?.dataset.ageAuthored,
      cardAuthored: card?.querySelector("[data-age-authored]")?.dataset.ageAuthored,
    };
  });
  assert.ok(Number(arrived.headerAuthored) > Number(previousArrival.authored));
  assert.equal(arrived.headerAuthored, arrived.cardAuthored);
  assert.ok(
    (await page.evaluate(() => window.__composerProvenanceStates))
      .every((state) => state.headerAuthored === state.cardAuthored),
    "no observed render mixes an old link with a new age",
  );
  assert.equal(await sourceLink.getAttribute("href"), sourceHref, "the quoted source stays pinned");
  await waitForPageFunction(
    page,
    "the new primary message age advances on the shared browser clock",
    5_000,
    ({ lane, label }) => document.querySelector(
      `.team-agent[data-agent-lane="${lane}"] .composer-shard--primary [data-age-authored]`,
    )?.textContent?.trim() !== label,
    { lane: seeded.lane, label: arrived.label },
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
  const stackOverflow = await quoteStack.evaluate((stack) => {
    const lane = stack.closest(".team-agent");
    const laneBox = lane.getBoundingClientRect();
    const primary = stack.querySelector(".composer-shard--primary").getBoundingClientRect();
    const quotes = [...stack.querySelectorAll(".composer-shard--quote")].map((shard) => {
      const box = shard.getBoundingClientRect();
      return {
        label: shard.querySelector("header strong")?.textContent.trim(),
        top: box.top,
        bottom: box.bottom,
        width: box.width,
        height: box.height,
      };
    });
    return {
      clientHeight: stack.clientHeight,
      scrollHeight: stack.scrollHeight,
      overflowY: getComputedStyle(stack).overflowY,
      shardCount: stack.querySelectorAll(".composer-shard").length,
      lane: { width: laneBox.width, height: laneBox.height },
      primary: {
        top: primary.top,
        bottom: primary.bottom,
        width: primary.width,
        height: primary.height,
      },
      quotes,
    };
  });
  assert.equal(stackOverflow.shardCount, 3);
  assert.deepEqual(
    stackOverflow.quotes.map((quote) => quote.label),
    [nestedQuotePreview, "quoted message"],
    "quote shards stack by age with the newest immediately above the primary",
  );
  assert.deepEqual(
    stackOverflow.lane,
    initialQuoteLaneFrame,
    "accumulating quote composers does not extend the surrounding agent frame",
  );
  for (const quote of stackOverflow.quotes) {
    assert.ok(
      Math.abs(quote.width - stackOverflow.primary.width) < 1 &&
        Math.abs(quote.height - stackOverflow.primary.height) < 1,
      `every quote retains the primary composer's outer dimensions: ${JSON.stringify(stackOverflow)}`,
    );
  }
  const orderedFrames = [...stackOverflow.quotes, stackOverflow.primary];
  assert.ok(
    orderedFrames.every(
      (frame, index) =>
        index === orderedFrames.length - 1 || frame.bottom <= orderedFrames[index + 1].top + 1,
    ),
    `oldest quote, newest quote, and primary do not overlap: ${JSON.stringify(stackOverflow)}`,
  );
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

  const viewport = page.viewportSize();
  assert.ok(viewport, "the quote-frame fixture has a viewport to restore");
  try {
    await page.setViewportSize({ width: 520, height: viewport.height });
    await quoteLane.scrollIntoViewIfNeeded();
    const narrow = await quoteStack.evaluate((stack) => {
      const stackBox = stack.getBoundingClientRect();
      const primary = stack.querySelector(".composer-shard--primary").getBoundingClientRect();
      const quotes = [...stack.querySelectorAll(".composer-shard--quote")].map((shard) => {
        const box = shard.getBoundingClientRect();
        return {
          width: box.width,
          height: box.height,
          label: shard.querySelector("header strong")?.textContent.trim(),
          textareaWidth: shard.querySelector("textarea")?.clientWidth,
          textareaHeight: shard.querySelector("textarea")?.clientHeight,
        };
      });
      return {
        bodyWidth: document.body.scrollWidth,
        viewportWidth: innerWidth,
        stack: { top: stackBox.top, bottom: stackBox.bottom },
        primary: { width: primary.width, height: primary.height },
        quotes,
      };
    });
    assert.ok(
      narrow.bodyWidth <= narrow.viewportWidth,
      `narrow quote composers do not widen the document: ${JSON.stringify(narrow)}`,
    );
    assert.ok(
      narrow.quotes.every(
        (quote) =>
          Math.abs(quote.width - narrow.primary.width) < 1 &&
          Math.abs(quote.height - narrow.primary.height) < 1 &&
          quote.textareaWidth > 0 &&
          quote.textareaHeight > 0 &&
          quote.label,
      ),
      `narrow quote composers remain equal, editable, and nonblank: ${JSON.stringify(narrow)}`,
    );
    const reachability = await quoteStack.evaluate(async (stack) => {
      stack.scrollTo({ top: 0, behavior: "instant" });
      await new Promise((resolve) => requestAnimationFrame(resolve));
      const stackBox = stack.getBoundingClientRect();
      const oldest = stack.querySelector(".composer-shard--quote").getBoundingClientRect();
      stack.scrollTo({ top: stack.scrollHeight, behavior: "instant" });
      await new Promise((resolve) => requestAnimationFrame(resolve));
      const primary = stack.querySelector(".composer-shard--primary").getBoundingClientRect();
      return {
        top: { stack: stackBox.top, quote: oldest.top },
        bottom: { stack: stackBox.bottom, primary: primary.bottom },
      };
    });
    assert.ok(
      reachability.top.quote >= reachability.top.stack - 1 &&
        reachability.bottom.primary <= reachability.bottom.stack + 1,
      `the bounded narrow stack can reveal both oldest quote and displaced primary: ${JSON.stringify(reachability)}`,
    );
  } finally {
    await page.setViewportSize(viewport);
  }

  previewShard = quoteLane.locator(".composer-shard--quote").last();
  await previewShard.waitFor();
  previewLabel = await previewShard.locator("header strong").innerText();
  assert.equal(previewLabel, "quoted message", "an absent preview has a visible placeholder");
  assert.ok(previewLabel.trim(), "the absent-preview shard is not blank");
  assert.doesNotMatch(previewLabel, /[<>]/);
  assert.equal(await previewShard.getAttribute("title"), entirelyQuotedMessage);
}
