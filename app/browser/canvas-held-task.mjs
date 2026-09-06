// Held-task and newest-activity proof for the real Canvas fixture.
import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import { TOOL_ACTIVITY_TEXT, toolCallId } from "./canvas-activity-records.mjs";

// Observe the one navigation tab the operator reference qualifies by a count.
//
// The Tasks badge renders nothing at zero
// (app/src/watchband.rs), so a fixture in which no lane holds a task shows
// no badge at all and the count stays unproven no matter how many tabs are
// clicked. `mode_badge` counts standings carrying a handle, so raising it means
// really claiming. This scenario reads the positive count and the held handle
// off the card, proves an admitted renewal moves the Tasks-row expiry countdown,
// then covers explicit release and natural lapse as separate badge-removal paths.
export async function assertHeldTaskCount(
  team,
  fixture,
  lanes,
  { artifacts, claimMs, reply, replyAsLane, restartAfterExpiry, transitionMs },
) {
  const [expiryLane, releaseLane] = lanes;
  const composerAge = team.locator(
    `.team-agent[data-agent-lane="${expiryLane}"] .composer-shard--primary .composer-shard-age`,
  );
  const composerProvenance = () => captureComposerProvenance(composerAge);
  const releaseComposerAge = team.locator(
    `.team-agent[data-agent-lane="${releaseLane}"] .composer-shard--primary .composer-shard-age`,
  );
  const releaseClaim = await claimAutomatically({
    claimMs,
    composerProvenance: () => captureComposerProvenance(releaseComposerAge),
    fixture,
    lane: releaseLane,
    artifacts: null,
    replyAsLane,
    team,
    transitionMs,
    verifyRenderedRenewal: false,
  });
  const claim = await claimAutomatically({
    claimMs,
    composerProvenance,
    fixture,
    lane: expiryLane,
    artifacts,
    replyAsLane,
    team,
    transitionMs,
    verifyRenderedRenewal: true,
  });
  await releaseClaimExplicitly({
    claim: releaseClaim,
    fixture,
    lane: releaseLane,
    reply,
    replyAsLane,
    team,
    transitionMs,
  });
  await lapseClaimNaturally({
    claim,
    composerProvenance,
    fixture,
    lane: expiryLane,
    reply,
    replyAsLane,
    restartAfterExpiry,
    team,
    transitionMs,
  });

  // The status strip follows one chronology across unlike transcript shapes.
  // The first one-shot fixture record is a real Claude tool_use block decoded
  // by the production capture path, not prose styled to resemble a tool.
  const ticker = team.locator(".ticker");
  await writeFile(join(fixture, ".spice-tool-trigger"), "emit\n");
  const toolLine = TOOL_ACTIVITY_TEXT;
  await ticker.filter({ hasText: toolLine }).waitFor({ timeout: 15_000 });
  const toolTicker = await captureTicker(ticker);
  assertTickerPresentation(toolTicker, toolLine, lanes[0], "tool", "tool call");
  assertComposerProvenance(
    await composerProvenance(), claim.provenance,
    "a tool call cannot retarget the prose header or its timestamp",
  );

  // The result of that same call arrives on a separate transcript record that
  // names only the vendor correlation id. The strip has to resolve it back to
  // the call, because `toolu_01...` is not something an operator can act on.
  await writeFile(join(fixture, ".spice-tool-result-trigger"), "emit\n");
  const resultLine = `${toolLine} succeeded`;
  await ticker.filter({ hasText: resultLine }).waitFor({ timeout: 15_000 });
  const resultTicker = await captureTicker(ticker);
  assertTickerPresentation(resultTicker, resultLine, lanes[0], "tool", "tool result");
  assert.ok(
    Number(resultTicker.authored) >= Number(toolTicker.authored),
    "the result status ages the result activity rather than the older tool call",
  );
  assertComposerProvenance(
    await composerProvenance(), claim.provenance,
    "a tool result cannot retarget the prose header or its timestamp",
  );

  // Falsification: the exact opaque line this task exists to remove must be
  // rejected by the same assertion the resolved line just passed. It is checked
  // without writing to the page, because overwriting a rendered node here
  // desynchronizes the client's diff and suppresses the next activity update.
  assert.throws(
    () => assertNewestLaneActivity(`${toolCallId} succeeded`, resultLine),
    /raw vendor tool identifier/,
    "the production assertion rejects a raw vendor identifier on the strip",
  );

  // The fake resident agent consumes this one-shot trigger in its real
  // transcript process. Capture then publishes a normal prose stream frame,
  // which is newer than the tool above and must replace it for the whole team.
  await writeFile(join(fixture, ".spice-prose-trigger"), "emit\n");
  const proseLine = "assistant: newest lane prose witness";
  await ticker.filter({ hasText: proseLine }).waitFor({ timeout: 15_000 });
  const proseTicker = await captureTicker(ticker);
  assertTickerPresentation(proseTicker, proseLine, lanes[0], "prose", "assistant prose");
  assert.ok(
    Number(proseTicker.authored) >= Number(resultTicker.authored),
    "the prose status ages the displayed prose rather than the prior tool result",
  );
  await assertModeLabelEllipsis(
    team,
    "desktop mode rail",
    artifacts && join(artifacts, "canvas-mode-label-desktop.png"),
  );
  await assertTickerAcrossModes(team, ticker, proseLine, lanes[0], "desktop assistant prose");

  if (artifacts) {
    const page = ticker.page();
    const viewport = page.viewportSize();
    await page.screenshot({
      path: join(artifacts, "canvas-lane-status-desktop.png"),
      fullPage: false,
    });
    await page.setViewportSize({ width: 480, height: viewport?.height ?? 900 });
    await assertModeLabelEllipsis(
      team,
      "narrow mode rail",
      join(artifacts, "canvas-mode-label-narrow.png"),
    );
    await assertTickerAcrossModes(team, ticker, proseLine, lanes[0], "narrow assistant prose");
    await page.screenshot({
      path: join(artifacts, "canvas-lane-status-narrow.png"),
      fullPage: false,
    });
    if (viewport) await page.setViewportSize(viewport);
  }

  // Falsification: feed the exact old receipt shape through the production
  // assertion. This remains a pure falsification so it
  // cannot destroy the structured age and attribution nodes the screenshot
  // and later Yew renders rely on.
  const restoredReceipt = "Lane snapshot: 4 lanes";
  assert.throws(
    () => assertNewestLaneActivity(restoredReceipt, proseLine),
    /newest lane activity/,
    "the production assertion rejects the former snapshot receipt",
  );
  assertTickerPresentation(
    await captureTicker(ticker), proseLine, lanes[0], "prose", "settled assistant prose",
  );
}

async function claimAutomatically({
  artifacts,
  claimMs,
  composerProvenance,
  fixture,
  lane,
  replyAsLane,
  team,
  transitionMs,
  verifyRenderedRenewal,
}) {
  const badge = team.getByRole("tab", { name: /^Tasks/ }).locator(".team-mode-badge");
  const lifetime = team.locator('.team-lifetime input[type="range"]');
  const handle = await waitForActiveClaim(fixture, lane, replyAsLane, claimMs * 4);
  assert.match(handle, /^\S+-\S+$/, `automatic allocation returns a task handle: ${handle}`);
  const firstProvenance = await waitForComposerProvenance(
    composerProvenance, transitionMs * 4,
  );
  const provenance = await waitForStableComposerProvenance(
    composerProvenance, firstProvenance, 2_000, transitionMs * 4,
  );
  assertComposerProvenance(provenance, provenance, "the automatic lease has one source");
  await badge.waitFor({ timeout: transitionMs * 4 });
  const heldBadgeCount = Number(await badge.innerText());
  assert.ok(
    Number.isInteger(heldBadgeCount) && heldBadgeCount >= 1,
    "Tasks counts at least the observed lane's held task",
  );
  const memberIndex = await memberIndexForLane(team, lane);
  const placeholder = await team.locator(`.team-agent[data-agent-lane="${lane}"]`)
    .getByRole("textbox").getAttribute("placeholder");
  const [heldHandle, phase] = placeholder.split("\n").at(-1).split(", ");
  assert.equal(heldHandle, handle, `the held card names the claimed task: ${placeholder}`);
  assert.ok(phase, `the held card names the phase it holds: ${placeholder}`);
  await team.getByRole("tab", { name: /^Tasks/ }).click();
  const taskRow = team.locator(`.team-task-row[data-task-handle="${handle}"]`);
  await taskRow.waitFor({ timeout: transitionMs * 4 });
  assert.equal(
    (await taskRow.locator(".team-task-title").innerText()).trim().length > 0,
    true,
    "the team task row carries the actor-facing task title",
  );
  assert.equal(
    (await taskRow.locator(".team-task-phase").innerText()).trim(),
    phase,
    "the team task row carries the authoritative workflow phase",
  );
  assert.equal(
    await taskRow.getAttribute("data-agent-lane"),
    lane,
    "the team task row remains attributed to the claiming member",
  );
  const cardGeometry = () => taskRow.evaluate((row) => {
    const bounds = row.getBoundingClientRect();
    const workspaceNode = row.closest(".team-task-workspace");
    const workspace = workspaceNode.getBoundingClientRect();
    const title = row.querySelector(".team-task-title");
    const handle = row.querySelector("code");
    const metadata = row.querySelector(".team-task-row-meta");
    return {
      box: [bounds.left, bounds.right, bounds.top, bounds.bottom, bounds.width],
      borderRadius: getComputedStyle(row).borderRadius,
      cardCount: workspaceNode.querySelectorAll(".team-task-row").length,
      contained: bounds.left >= workspace.left && bounds.right <= workspace.right,
      handle: handle.textContent,
      handleOverflow: handle.scrollWidth - handle.clientWidth,
      listTag: row.parentElement.tagName,
      member: row.dataset.agentLane,
      metadataOverflow: metadata.scrollWidth - metadata.clientWidth,
      phase: row.querySelector(".team-task-phase").textContent,
      title: title.textContent,
      titleWhiteSpace: getComputedStyle(title).whiteSpace,
      workspaceOverflow: workspaceNode.scrollWidth - workspaceNode.clientWidth,
    };
  });
  const assertCardGeometry = (geometry, viewport) => {
    assert.equal(geometry.listTag, "OL", `${viewport} active task remains a semantic list item`);
    assert.equal(geometry.titleWhiteSpace, "normal", `${viewport} task title wraps`);
    assert.ok(Number.parseFloat(geometry.borderRadius) > 0, `${viewport} task has a card frame`);
    assert.ok(
      geometry.cardCount > 0
        && geometry.contained
        && geometry.handleOverflow <= 0
        && geometry.metadataOverflow <= 0
        && geometry.workspaceOverflow <= 0,
      `${viewport} active task content remains contained: ${JSON.stringify(geometry)}`,
    );
  };
  const originalCardGeometry = await cardGeometry();
  assertCardGeometry(originalCardGeometry, "desktop");
  const cardViewport = taskRow.page().viewportSize();
  await taskRow.page().setViewportSize({ width: 390, height: cardViewport?.height ?? 844 });
  await taskRow.page().evaluate(() => new Promise((resolve) => requestAnimationFrame(resolve)));
  assertCardGeometry(await cardGeometry(), "narrow");
  if (cardViewport) await taskRow.page().setViewportSize(cardViewport);
  const taskTitle = (await taskRow.locator(".team-task-title").innerText()).trim();
  const taskAge = taskRow.locator(".team-task-age");
  const taskCountdown = await captureRelativeClock(taskAge);
  const initialExpiry = Number(taskCountdown.expiry);
  assert.ok(Number.isFinite(initialExpiry), "the task row carries its authoritative lease expiry");
  assert.ok(initialExpiry > Date.now(), "an active task counts down from a future expiry");
  assertCountdownClock(taskCountdown, initialExpiry, "the active Tasks row");
  if (artifacts) {
    await team.page().screenshot({
      path: join(artifacts, "canvas-tasks-active-claim.png"),
      fullPage: true,
    });
  }
  await team.getByRole("tab", { name: /^Metrics/ }).click();
  const metricExpiry = await captureRelativeClock(metricAge(team, memberIndex, "Expires"));
  assertCountdownClock(metricExpiry, initialExpiry, "the active Metrics expiry");
  const claimAge = await captureRelativeClock(metricAge(team, memberIndex, "Claim age"));
  assert.ok(
    claimAge.authored !== null && Number.isFinite(Number(claimAge.authored)),
    "claim age retains its authoritative lease-start instant",
  );
  assert.equal(claimAge.clock, "deadline", "claim age remains an elapsed clock");
  assert.equal(claimAge.expiry, null, "claim age cannot become an expiry countdown");
  await team.getByRole("tab", { name: /^Agents/ }).click();
  const promotedExpiry = await waitForPromotedClaimExpiry(
    fixture, lane, handle, replyAsLane, claimMs * 4,
  );
  const renewedExpiry = verifyRenderedRenewal
    ? await waitForClaimExpiry(fixture, lane, handle, replyAsLane, 35_000, promotedExpiry)
    : promotedExpiry;
  if (verifyRenderedRenewal) {
    assert.ok(
      renewedExpiry > promotedExpiry,
      "the resident heartbeat moves task lease expiry",
    );
    await team.getByRole("tab", { name: /^Tasks/ }).click();
    const renewed = await waitForRenderedTaskExpiry(
      team, handle, renewedExpiry, transitionMs * 4,
    );
    assert.ok(
      renewed.expiry >= renewedExpiry,
      "the rendered countdown follows the renewed authoritative expiry",
    );
    assert.equal(renewed.clock, "countdown", "renewal preserves countdown semantics");
    assert.equal(renewed.authored, null, "renewal cannot restore elapsed-since-renewal metadata");
    assert.match(renewed.rendered, /^(?:now|\d+[smhd]\+?)$/, "expiry countdown remains readable");
    assert.equal(renewed.title, taskTitle, "renewal preserves the actor-facing title");
    assert.equal(renewed.phase, phase, "renewal preserves the authoritative workflow phase");
    assert.equal(renewed.lane, lane, "renewal preserves the claiming member attribution");
    const renewedCardGeometry = await cardGeometry();
    assertCardGeometry(renewedCardGeometry, "renewed desktop");
    assertRelativeClockBox(
      renewedCardGeometry.box,
      originalCardGeometry.box,
      "authoritative renewal preserves the active task card box",
    );
    assert.deepEqual(
      {
        cardCount: renewedCardGeometry.cardCount,
        handle: renewedCardGeometry.handle,
        member: renewedCardGeometry.member,
        phase: renewedCardGeometry.phase,
        title: renewedCardGeometry.title,
      },
      {
        cardCount: originalCardGeometry.cardCount,
        handle: originalCardGeometry.handle,
        member: originalCardGeometry.member,
        phase: originalCardGeometry.phase,
        title: originalCardGeometry.title,
      },
      "authoritative renewal preserves card count and semantic identity",
    );
    assert.equal(
      Number(await badge.innerText()),
      heldBadgeCount,
      "renewal preserves the Tasks badge count",
    );
    await team.getByRole("tab", { name: /^Agents/ }).click();
  }
  assertComposerProvenance(
    await composerProvenance(), provenance,
    "automatic renewal cannot retarget provenance",
  );
  const heldStatus = parseTaskStatus(
    (await replyAsLane(fixture, lane, ["task", "status"])).body.text,
  );
  assert.equal(heldStatus.claim, handle, "lane status names the renewed automatic claim");
  return {
    badge, handle, heldBadgeCount, heldStatus, lifetime, memberIndex, provenance, renewedExpiry,
  };
}

async function releaseClaimExplicitly({
  claim,
  fixture,
  lane,
  reply,
  replyAsLane,
  team,
  transitionMs,
}) {
  const { badge, handle, heldStatus, lifetime, memberIndex } = claim;
  if (await lifetime.inputValue() !== "0") await lifetime.fill("0");
  await team.getByRole("button", { name: "Steer", exact: true }).waitFor();
  await waitForTaskPolicy(fixture, lane, "steer", reply, transitionMs);
  await team.getByRole("tab", { name: /^Tasks/ }).click();
  const taskRow = team.locator(`.team-task-row[data-task-handle="${handle}"]`);
  await taskRow.waitFor({ timeout: transitionMs });
  const queuedBefore = await team.locator(".team-direction-row").count();
  const preReleaseBadgeCount = Number(await badge.innerText());
  assert.ok(
    Number.isInteger(preReleaseBadgeCount) && preReleaseBadgeCount >= 2,
    "the two-lane team counts both claims before explicit release",
  );

  await replyAsLane(fixture, lane, ["task", "free", handle]);
  const releasedBadgeCount = await waitForBadgeBelow(
    badge, preReleaseBadgeCount, transitionMs * 4,
  );
  await taskRow.waitFor({ state: "detached", timeout: transitionMs * 4 });
  assert.equal(
    releasedBadgeCount,
    preReleaseBadgeCount - 1,
    "explicit release removes exactly the freed lane from the Tasks badge",
  );
  assert.equal(
    await team.locator(".team-direction-row").count(),
    queuedBefore,
    "explicit task release does not reinterpret queued direction",
  );
  await team.getByRole("tab", { name: /^Metrics/ }).click();
  const absentExpiry = await captureRelativeClock(metricAge(team, memberIndex, "Expires"));
  assert.equal(absentExpiry.label, "—", "a released lane has no remaining lease duration");
  assert.equal(absentExpiry.expiry, null, "a released lane carries no expiry instant");
  assert.equal(absentExpiry.authored, null, "an absent expiry cannot masquerade as an age");
  assert.equal(absentExpiry.clock, "countdown", "the vacant slot retains expiry semantics");
  assert.match(absentExpiry.slot ?? "", /^\d+$/, "the vacant expiry keeps its fixed-width slot");
  const releasedStatus = parseTaskStatus(
    (await replyAsLane(fixture, lane, ["task", "status"])).body.text,
  );
  assert.equal(releasedStatus.claim, null, "task free removes the lane's active claim");
  assert.equal(releasedStatus.stale, heldStatus.stale, "task free does not manufacture expiry");
}

async function lapseClaimNaturally({
  claim,
  composerProvenance,
  fixture,
  lane,
  reply,
  replyAsLane,
  restartAfterExpiry,
  team,
  transitionMs,
}) {
  const {
    badge, handle, heldStatus, lifetime, provenance, renewedExpiry,
  } = claim;
  // Steer prevents the lapsed task from being automatically allocated again.
  // The resident is then stopped across the exact durable deadline, which also
  // proves no browser clock or task-free command manufactured the transition.
  await team.getByRole("tab", { name: /^Agents/ }).click();
  await lifetime.fill("0");
  await team.getByRole("button", { name: "Steer", exact: true }).waitFor();
  await waitForTaskPolicy(fixture, lane, "steer", reply, transitionMs);
  await clearLaneInbox(fixture, lane, reply);
  const stoppedExpiry = await readClaimExpiry(fixture, lane, handle, replyAsLane);
  assert.ok(stoppedExpiry >= renewedExpiry, "shutdown uses the final heartbeat deadline");
  assert.ok(
    stoppedExpiry - Date.now() > 85_000,
    "shutdown begins well before the next automatic heartbeat",
  );
  const preExpiryBadgeCount = Number(await badge.innerText());
  assert.ok(
    Number.isInteger(preExpiryBadgeCount) && preExpiryBadgeCount >= 1,
    "the lineage still counts the observed claim before resident shutdown",
  );
  assertComposerProvenance(
    await composerProvenance(), provenance, "pre-expiry shutdown preserves provenance",
  );
  await team.getByRole("tab", { name: /^Tasks/ }).click();
  const taskAge = team
    .locator(`.team-task-row[data-task-handle="${handle}"] .team-task-age`);
  await restartAfterExpiry(
    stoppedExpiry + transitionMs,
    () => observeCountdownThroughExpiry(taskAge, stoppedExpiry, transitionMs),
  );
  await team.getByRole("tab", { name: /^Agents/ }).first().click();
  const lapsedBadgeCount = await waitForBadgeBelow(
    badge, preExpiryBadgeCount, transitionMs * 4,
  );
  assert.ok(
    lapsedBadgeCount < preExpiryBadgeCount,
    "Tasks loses at least the observed lane's naturally expired task",
  );
  const lapsedStatus = parseTaskStatus(
    (await replyAsLane(fixture, lane, ["task", "status"])).body.text,
  );
  assert.equal(lapsedStatus.claim, null, "natural expiry removes the lane's active claim");
  assert.equal(
    lapsedStatus.stale,
    heldStatus.stale + 1,
    "natural expiry records one stale claim; task free would not",
  );
  assertComposerProvenance(
    await composerProvenance(), provenance, "natural task expiry preserves provenance",
  );
  const lanes = await reply(fixture, ["lane"]);
  const standing = lanes.body.lanes.find((candidate) => candidate.lane === lane);
  assert.ok(standing?.occupant, "resident restart restores the lapsed lane occupant");
  assertComposerProvenance(
    await composerProvenance(), provenance, "restoring the occupant preserves provenance",
  );
}

async function waitForComposerProvenance(composerProvenance, timeout) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    last = await composerProvenance();
    if (
      /^#\S+$/.test(last.href ?? "")
      && Number.isFinite(Number(last.authored))
      && last.authored === last.cardAuthored
    ) return last;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for composer provenance: ${JSON.stringify(last)}`);
}

async function waitForStableComposerProvenance(
  composerProvenance,
  initial,
  quietMs,
  timeout,
) {
  const deadline = Date.now() + timeout;
  let stableSince = Date.now();
  let last = initial;
  while (Date.now() < deadline) {
    await new Promise((resolve) => setTimeout(resolve, 100));
    const current = await composerProvenance();
    if (current.href !== last.href || current.authored !== last.authored) {
      stableSince = Date.now();
      last = current;
    }
    if (Date.now() - stableSince >= quietMs) return current;
  }
  throw new Error(`composer provenance did not remain stable for ${quietMs}ms`);
}

async function memberIndexForLane(team, lane) {
  const memberLanes = await team.locator(".team-agent").evaluateAll(
    (members) => members.map((member) => member.dataset.agentLane),
  );
  const index = memberLanes.indexOf(lane);
  assert.ok(index >= 0, `the rendered lineage contains member ${lane}: ${memberLanes}`);
  return index;
}

function metricAge(team, memberIndex, label) {
  return team.locator(".team-detail-card").nth(memberIndex)
    .getByText(label, { exact: true }).locator("..").locator(".claim-age");
}

async function captureRelativeClock(age) {
  return age.evaluate((node) => {
    const bounds = node.getBoundingClientRect();
    return {
      authored: node.getAttribute("data-age-authored"),
      box: [bounds.width, bounds.height],
      clock: node.getAttribute("data-age-clock"),
      expiry: node.getAttribute("data-expiry"),
      label: node.textContent?.trim(),
      slot: node.getAttribute("data-age-slot"),
    };
  });
}

function assertCountdownClock(actual, expiry, message) {
  assert.equal(actual.expiry, String(expiry), `${message} carries the authoritative expiry`);
  assert.equal(actual.clock, "countdown", `${message} uses countdown semantics`);
  assert.equal(actual.authored, null, `${message} carries no elapsed-age instant`);
  assert.match(actual.label ?? "", /^(?:now|\d+[smhd]\+?)$/, `${message} is compact relative time`);
}

async function observeCountdownThroughExpiry(age, expiry, transitionMs) {
  const before = await captureRelativeClock(age);
  const renderedExpiry = Number(before.expiry);
  assert.ok(
    Number.isFinite(renderedExpiry) && renderedExpiry >= expiry,
    "the stopped resident preserves the final rendered authoritative expiry",
  );
  assertCountdownClock(before, renderedExpiry, "the stopped resident's task countdown");
  const near = await waitForCountdown(
    age,
    (snapshot) => snapshot.label === "1s",
    renderedExpiry - Date.now() + transitionMs,
  );
  const expired = await waitForCountdown(
    age,
    (snapshot) => Date.now() >= renderedExpiry && snapshot.label === "now",
    transitionMs * 2,
  );
  assert.notEqual(near.label, before.label, "the offline countdown visibly decreases");
  assertCountdownClock(near, renderedExpiry, "the near-expiry task countdown");
  assertCountdownClock(expired, renderedExpiry, "the expired task countdown");
  assertRelativeClockBox(near.box, before.box, "the countdown slot does not move at one second");
  assertRelativeClockBox(expired.box, before.box, "the countdown slot does not move at expiry");
  assert.equal(near.slot, before.slot, "the near-expiry slot keeps its width contract");
  assert.equal(expired.slot, before.slot, "the expired slot keeps its width contract");
  return renderedExpiry + transitionMs;
}

function assertRelativeClockBox(actual, expected, message) {
  assert.ok(
    actual.every((value, index) => Math.abs(value - expected[index]) <= 0.5),
    `${message}: expected ${expected}, observed ${actual}`,
  );
}

async function waitForCountdown(age, accept, timeout) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    last = await captureRelativeClock(age);
    if (accept(last)) return last;
    await new Promise((resolve) => setTimeout(resolve, 50));
  }
  throw new Error(`timed out waiting for task countdown state: ${JSON.stringify(last)}`);
}

async function waitForRenderedTaskExpiry(team, handle, minimum, timeout) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    const row = team.locator(`.team-task-row[data-task-handle="${handle}"]`);
    if (await row.count() === 1) {
      const age = row.locator(".team-task-age");
      last = {
        expiry: Number(await age.getAttribute("data-expiry")),
        clock: await age.getAttribute("data-age-clock"),
        authored: await age.getAttribute("data-age-authored"),
        rendered: (await age.innerText()).trim(),
        title: (await row.locator(".team-task-title").innerText()).trim(),
        phase: (await row.locator(".team-task-phase").innerText()).trim(),
        lane: await row.getAttribute("data-agent-lane"),
      };
      if (Number.isFinite(last.expiry) && last.expiry >= minimum) return last;
    }
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(
    `timed out waiting for rendered expiry at least ${minimum}: ${JSON.stringify(last)}`,
  );
}

async function waitForPromotedClaimExpiry(fixture, lane, handle, replyAsLane, timeout) {
  const deadline = Date.now() + timeout;
  let last = 0;
  while (Date.now() < deadline) {
    last = await readClaimExpiry(fixture, lane, handle, replyAsLane);
    if (last - Date.now() > 60_000) return last;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for promoted lease for ${handle}; last=${last}`);
}

async function waitForBadgeBelow(badge, before, timeout) {
  const deadline = Date.now() + timeout;
  let last = before;
  while (Date.now() < deadline) {
    const labels = await badge.allTextContents();
    last = labels.length === 0 ? 0 : Number(labels[0]);
    if (Number.isInteger(last) && last < before) return last;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for held-task badge below ${before}; last=${last}`);
}

async function captureComposerProvenance(composerAge) {
  return composerAge.evaluate((link) => {
    const href = link.getAttribute("href");
    const age = link.querySelector("[data-age-authored]");
    const card = href?.startsWith("#") ? document.getElementById(href.slice(1)) : null;
    return {
      href,
      authored: age?.dataset.ageAuthored,
      cardAuthored: card?.querySelector("[data-age-authored]")?.dataset.ageAuthored,
      rendered: age?.textContent?.trim(),
      clock: age?.dataset.ageClock,
    };
  });
}

function assertComposerProvenance(actual, expected, message) {
  assert.match(actual.href ?? "", /^#\S+$/, `${message}: source link remains present`);
  assert.ok(Number.isFinite(Number(actual.authored)), `${message}: source timestamp remains finite`);
  assert.deepEqual(
    { href: actual.href, authored: actual.authored },
    { href: expected.href, authored: expected.authored },
    message,
  );
  assert.equal(actual.authored, actual.cardAuthored, `${message}: link and card timestamps agree`);
  assert.match(actual.rendered, /^(?:now|\d+[smhd])$/, `${message}: relative time remains visible`);
  assert.equal(actual.clock, "deadline", `${message}: relative time uses the message clock`);
}

async function clearLaneInbox(fixture, lane, reply) {
  const inbox = await reply(fixture, ["inbox", lane]);
  for (const directive of inbox.body.lanes[0].pending) {
    await reply(fixture, ["steer", "cancel", directive.directive]);
  }
  const quiet = await reply(fixture, ["inbox", lane]);
  assert.deepEqual(quiet.body.lanes[0].pending, [], "no pending direction can consume vacancy");
}

async function readClaimExpiry(fixture, lane, handle, replyAsLane) {
  const shown = await replyAsLane(fixture, lane, ["task", "show", handle]);
  const match = shown.body.text.match(new RegExp(`^claim lane:${lane} until (.+)$`, "m"));
  const expiry = Date.parse(match?.[1] ?? "");
  assert.ok(Number.isFinite(expiry), `task show names ${handle} lease expiry: ${shown.body.text}`);
  return expiry;
}

async function waitForClaimExpiry(
  fixture,
  lane,
  handle,
  replyAsLane,
  timeout,
  after,
) {
  const deadline = Date.now() + timeout;
  let last = 0;
  while (Date.now() < deadline) {
    last = await readClaimExpiry(fixture, lane, handle, replyAsLane);
    if (last > after) return last;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for ${handle} lease expiry after ${after}; last=${last}`);
}

async function waitForActiveClaim(fixture, lane, replyAsLane, timeout) {
  const deadline = Date.now() + timeout;
  let last = "";
  while (Date.now() < deadline) {
    last = (await replyAsLane(fixture, lane, ["task", "status"])).body.text;
    const status = parseTaskStatus(last);
    if (status.claim) return status.claim;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(`timed out waiting for ${lane} automatic claim: ${JSON.stringify(last)}`);
}

async function waitForTaskPolicy(fixture, lane, policy, reply, timeout) {
  const deadline = Date.now() + timeout;
  let last;
  while (Date.now() < deadline) {
    last = (await reply(fixture, ["lane"])).body;
    const row = last.lanes.find((candidate) => candidate.lane === lane);
    const subscription = last.subscriptions.find(
      (candidate) => candidate.lineage_id === row?.lineage_id,
    );
    if (subscription?.policy === policy) return;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(
    `timed out waiting for ${lane} ${policy} task policy: ${JSON.stringify(last)}`,
  );
}

function parseTaskStatus(text) {
  const claim = text.match(/^claim (.+)$/m)?.[1];
  const stale = Number(text.match(/^stale (\d+)$/m)?.[1]);
  assert.ok(claim, `task status names a claim field: ${JSON.stringify(text)}`);
  assert.ok(Number.isInteger(stale), `task status names a stale count: ${JSON.stringify(text)}`);
  return { claim: claim === "-" ? null : claim, stale };
}

function assertNewestLaneActivity(actual, expected) {
  assert.doesNotMatch(
    actual,
    /(?:Board|Lane) snapshot|Steering inbox/,
    "newest lane activity excludes protocol snapshot receipts",
  );
  assert.doesNotMatch(
    actual,
    /toolu_/,
    "newest lane activity never shows a raw vendor tool identifier",
  );
  assert.equal(actual, expected, "newest lane activity wins across tool and prose shapes");
}

async function captureTicker(ticker) {
  return ticker.evaluate((node) => {
    const surface = node.closest(".team-surface");
    const lane = node.dataset.agentLane;
    const agent = surface?.querySelector(`.team-agent[data-agent-lane="${lane}"]`);
    const pip = surface?.querySelector(`.team-light[data-lane-light-target-id="${lane}"]`);
    const age = node.querySelector(".ticker-age");
    const prefix = node.querySelector(".ticker-prefix");
    const detail = node.querySelector(".ticker-detail");
    const toolbar = surface?.querySelector(":scope > .team-toolbar");
    const pane = surface?.querySelector(":scope > .team-pane");
    const bounds = node.getBoundingClientRect();
    const surfaceBounds = surface?.getBoundingClientRect();
    const toolbarBounds = toolbar?.getBoundingClientRect();
    const paneBounds = pane?.getBoundingClientRect();
    const ageBounds = age?.getBoundingClientRect();
    const style = getComputedStyle(node);
    const surfaceStyle = surface ? getComputedStyle(surface) : null;
    return {
      accent: node.dataset.accent,
      agentAccent: agent?.dataset.accent,
      aria: node.getAttribute("aria-label"),
      authored: age?.dataset.ageAuthored,
      ageClock: age?.dataset.ageClock,
      ageColor: age ? getComputedStyle(age).color : null,
      ageInlineStartInset: ageBounds ? ageBounds.left - bounds.left : null,
      ageLabel: age?.textContent?.trim(),
      className: node.className,
      directTickers: surface
        ? Array.from(surface.children).filter((child) => child.matches(".ticker")).length
        : 0,
      lane,
      line: [prefix?.textContent?.trim(), detail?.textContent?.trim()]
        .filter(Boolean).join(" "),
      mode: pane?.dataset.teamMode,
      nestedTickers: surface?.querySelectorAll(".team-mode-pane .ticker").length ?? 0,
      nonOverlapping: Boolean(
        toolbarBounds
          && paneBounds
          && bounds.top >= toolbarBounds.bottom - 1
          && bounds.bottom <= paneBounds.top + 1,
      ),
      overflowsInline: node.scrollWidth > node.clientWidth + 1,
      paddingInlineEnd: style.paddingInlineEnd,
      paddingInlineStart: style.paddingInlineStart,
      pipColor: pip ? getComputedStyle(pip).backgroundColor : null,
      pipStatus: pip?.dataset.laneStatus,
      prefixColor: prefix ? getComputedStyle(prefix).color : null,
      sourceColor: agent?.querySelector(".composer-shard-prose")
        ? getComputedStyle(agent.querySelector(".composer-shard-prose")).color
        : null,
      status: node.dataset.laneStatus,
      surfaceAgeInlineStartInset: ageBounds && surfaceBounds
        ? ageBounds.left - surfaceBounds.left
        : null,
      surfaceBorderInlineStart: surfaceStyle
        ? Number.parseFloat(surfaceStyle.borderInlineStartWidth)
        : null,
      underToolbar: toolbar?.nextElementSibling === node && node.nextElementSibling === pane,
      withinSurface: Boolean(
        surfaceBounds
          && bounds.left >= surfaceBounds.left - 1
          && bounds.right <= surfaceBounds.right + 1
          && bounds.top >= surfaceBounds.top - 1,
      ),
    };
  });
}

async function assertTickerAcrossModes(team, ticker, expected, lane, message) {
  for (const [label, mode] of [
    ["Tasks", "Tasks"],
    ["Metrics", "Metrics"],
    ["Team", "Team"],
    ["Agents", "Lanes"],
  ]) {
    await team.getByRole("tab", { name: new RegExp(`^${label}`) }).click();
    const actual = await captureTicker(ticker);
    assert.equal(
      actual.mode,
      mode,
      `${message}: ${label} is the active mode`,
    );
    assertTickerPresentation(
      actual,
      expected,
      lane,
      "prose",
      `${message}: ${label}`,
    );
  }
}

async function assertModeLabelEllipsis(team, message, screenshot) {
  const label = team.locator(".team-mode-label").first();
  const original = await label.textContent();
  const long = "Agents mode label that stays inside its allotted tab without moving controls";
  const before = await label.evaluate(modeLabelGeometry);
  assert.equal(before.title, original, `${message}: short label remains available as a title`);
  try {
    await label.evaluate((node, text) => {
      node.textContent = text;
    }, long);
    const after = await label.evaluate(modeLabelGeometry);
    assert.equal(
      await team.getByRole("tab", { name: new RegExp(`^${long}`) }).count(),
      1,
      `${message}: the full clipped label remains the tab's accessible name`,
    );
    assert.equal(after.textOverflow, "ellipsis", `${message}: long label declares an ellipsis`);
    assert.equal(after.overflowX, "hidden", `${message}: long label clips inside its tab`);
    assert.equal(after.whiteSpace, "nowrap", `${message}: long label stays on one line`);
    assert.ok(after.scrollWidth > after.clientWidth, `${message}: long label exercises truncation`);
    assert.equal(after.contained, true, `${message}: long label remains inside its tab`);
    assert.equal(after.nonOverlapping, true, `${message}: mode tabs do not overlap`);
    assert.deepEqual(after.controls, before.controls, `${message}: labels move no controls`);
    if (screenshot) await label.page().screenshot({ path: screenshot, fullPage: false });
  } finally {
    await label.evaluate((node, text) => {
      node.textContent = text;
    }, original);
  }
  assert.equal(await label.textContent(), original, `${message}: short label is unchanged`);
}

function modeLabelGeometry(node) {
  const button = node.parentElement;
  const surface = node.closest(".team-surface");
  const bounds = node.getBoundingClientRect();
  const buttonBounds = button.getBoundingClientRect();
  const modes = [...surface.querySelectorAll(".team-mode")]
    .map((mode) => mode.getBoundingClientRect());
  const controls = [surface.querySelector(".team-mode-rail"), surface.querySelector(".team-menu")]
    .map((control) => {
      const rect = control.getBoundingClientRect();
      return [rect.left, rect.top, rect.width, rect.height];
    });
  const style = getComputedStyle(node);
  return {
    clientWidth: node.clientWidth,
    contained: bounds.left >= buttonBounds.left && bounds.right <= buttonBounds.right,
    controls,
    nonOverlapping: modes.every((mode, index) => index === 0 || modes[index - 1].right <= mode.left),
    overflowX: style.overflowX,
    scrollWidth: node.scrollWidth,
    textOverflow: style.textOverflow,
    title: node.title,
    whiteSpace: style.whiteSpace,
  };
}

function assertTickerPresentation(actual, expected, lane, activityClass, message) {
  assertNewestLaneActivity(actual.line, expected);
  assert.equal(actual.lane, lane, `${message}: status names the producing lane`);
  assert.match(actual.className, new RegExp(`(?:^| )${activityClass}(?: |$)`));
  assert.equal(actual.directTickers, 1, `${message}: lineage owns one direct status row`);
  assert.equal(actual.nestedTickers, 0, `${message}: mode panes own no duplicate status row`);
  assert.equal(actual.underToolbar, true, `${message}: status sits between toolbar and pane`);
  assert.equal(actual.nonOverlapping, true, `${message}: status overlaps neither sibling`);
  assert.equal(actual.withinSurface, true, `${message}: status remains inside its lineage width`);
  assert.equal(actual.paddingInlineStart, "6px", `${message}: status starts 6px from its own edge`);
  assert.equal(actual.paddingInlineEnd, "10px", `${message}: status retains its 10px trailing inset`);
  assert.ok(
    Math.abs(actual.ageInlineStartInset - 6) <= 0.5,
    `${message}: age begins at the declared 6px inset`,
  );
  assert.ok(
    Math.abs(
      actual.surfaceAgeInlineStartInset - (actual.surfaceBorderInlineStart + 6),
    ) <= 0.5,
    `${message}: age begins 6px inside the lineage border`,
  );
  assert.equal(actual.overflowsInline, false, `${message}: status has no horizontal overflow`);
  assert.match(actual.ageLabel ?? "", /^(?:now|\d+[smhd])$/, `${message}: age is relative`);
  assert.ok(Number.isFinite(Number(actual.authored)), `${message}: age carries activity time`);
  assert.equal(actual.ageClock, "deadline", `${message}: age uses the shared ticking clock`);
  assert.equal(actual.status, actual.pipStatus, `${message}: age and source pip share lane status`);
  assert.equal(actual.ageColor, actual.pipColor, `${message}: age and source pip share color`);
  assert.equal(actual.accent, actual.agentAccent, `${message}: ticker selects the source accent`);
  assert.equal(actual.prefixColor, actual.sourceColor, `${message}: prefix carries source color`);
  assert.ok(actual.aria?.endsWith(expected), `${message}: accessible name includes agent and status`);
}
