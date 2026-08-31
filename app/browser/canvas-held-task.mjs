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
// off the card, then proves natural lapse removes both without issuing free.
export async function assertHeldTaskCount(
  team,
  fixture,
  lane,
  { artifacts, claimMs, reply, replyAsLane, restartAfterExpiry, transitionMs },
) {
  const composerAge = team.locator(
    `.team-agent[data-agent-lane="${lane}"] .composer-shard--primary .composer-shard-age`,
  );
  const composerProvenance = () => captureComposerProvenance(composerAge);
  const claim = await claimAutomatically({
    claimMs,
    composerProvenance,
    fixture,
    lane,
    artifacts,
    replyAsLane,
    team,
    transitionMs,
  });
  await lapseClaimNaturally({
    claim,
    composerProvenance,
    fixture,
    lane,
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
  const toolLine = `tool: ${TOOL_ACTIVITY_TEXT}`;
  await ticker.filter({ hasText: toolLine }).waitFor({ timeout: 15_000 });
  assertNewestLaneActivity(await ticker.innerText(), toolLine);
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
  assertNewestLaneActivity(await ticker.innerText(), resultLine);
  assertComposerProvenance(
    await composerProvenance(), claim.provenance,
    "a tool result cannot retarget the prose header or its timestamp",
  );

  // Falsification: the exact opaque line this task exists to remove must be
  // rejected by the same assertion the resolved line just passed. It is checked
  // without writing to the page, because overwriting a rendered node here
  // desynchronizes the client's diff and suppresses the next activity update.
  assert.throws(
    () => assertNewestLaneActivity(`tool: ${toolCallId} succeeded`, resultLine),
    /raw vendor tool identifier/,
    "the production assertion rejects a raw vendor identifier on the strip",
  );

  // The fake resident agent consumes this one-shot trigger in its real
  // transcript process. Capture then publishes a normal prose stream frame,
  // which is newer than the tool above and must replace it for the whole team.
  await writeFile(join(fixture, ".spice-prose-trigger"), "emit\n");
  const proseLine = "assistant: newest lane prose witness";
  await ticker.filter({ hasText: proseLine }).waitFor({ timeout: 15_000 });
  assertNewestLaneActivity(await ticker.innerText(), proseLine);

  // Falsification: restore the exact old receipt shape in the production DOM
  // and run the same assertion. It must reject the regression before the
  // observed activity line is restored for the remainder of the fixture.
  const observed = await ticker.innerText();
  await ticker.evaluate((node) => {
    node.textContent = "Lane snapshot: 4 lanes";
  });
  const restoredReceipt = await ticker.innerText();
  assert.equal(
    restoredReceipt,
    "Lane snapshot: 4 lanes",
    "the old receipt is restored in the DOM",
  );
  assert.throws(
    () => assertNewestLaneActivity(restoredReceipt, proseLine),
    /newest lane activity/,
    "the production assertion rejects the former snapshot receipt",
  );
  await ticker.evaluate((node, text) => {
    node.textContent = text;
  }, observed);
  assertNewestLaneActivity(await ticker.innerText(), proseLine);
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
  if (artifacts) {
    await team.page().screenshot({
      path: join(artifacts, "canvas-tasks-active-claim.png"),
      fullPage: true,
    });
  }
  await team.getByRole("tab", { name: /^Compose/ }).click();
  const promotedExpiry = await waitForPromotedClaimExpiry(
    fixture, lane, handle, replyAsLane, claimMs * 4,
  );
  const renewedExpiry = await waitForClaimExpiry(
    fixture, lane, handle, replyAsLane, 35_000, promotedExpiry,
  );
  assert.ok(
    renewedExpiry > promotedExpiry,
    "the resident heartbeat moves task lease expiry",
  );
  assertComposerProvenance(
    await composerProvenance(), provenance,
    "automatic renewal cannot retarget provenance",
  );
  const heldStatus = parseTaskStatus(
    (await replyAsLane(fixture, lane, ["task", "status"])).body.text,
  );
  assert.equal(heldStatus.claim, handle, "lane status names the renewed automatic claim");
  return { badge, handle, heldBadgeCount, heldStatus, lifetime, provenance, renewedExpiry };
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
    badge, handle, heldBadgeCount, heldStatus, lifetime, provenance, renewedExpiry,
  } = claim;
  // Steer prevents the lapsed task from being automatically allocated again.
  // The resident is then stopped across the exact durable deadline, which also
  // proves no browser clock or task-free command manufactured the transition.
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
    Number.isInteger(preExpiryBadgeCount) && preExpiryBadgeCount >= heldBadgeCount,
    "the lineage still counts the observed claim before resident shutdown",
  );
  assertComposerProvenance(
    await composerProvenance(), provenance, "pre-expiry shutdown preserves provenance",
  );
  await restartAfterExpiry(stoppedExpiry + transitionMs);
  await team.getByRole("tab", { name: /^Compose/ }).first().click();
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
    last = await badge.count() === 0 ? 0 : Number(await badge.innerText());
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
