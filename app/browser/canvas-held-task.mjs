// Held-task and newest-activity proof for the real Canvas fixture.
import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import { TOOL_ACTIVITY_TEXT, toolCallId } from "./canvas-activity-records.mjs";

// Observe the one navigation tab the operator reference qualifies by a count.
//
// The reference image shows "Filters 27", and `badge` renders nothing at zero
// (app/src/watchband.rs), so a fixture in which no lane holds a task shows
// no badge at all and the count stays unproven no matter how many tabs are
// clicked. `mode_badge` counts standings carrying a handle, so raising it means
// really claiming: this takes the badge from absent to 1 and back, and reads
// the held handle off the card, which is the same third placeholder line the
// reference shows a card wearing.
export async function assertHeldTaskCount(team, fixture, lane, replyAsLane) {
  const composerAge = team.locator(
    `.team-agent[data-agent-lane="${lane}"] .composer-shard--primary .composer-shard-age`,
  );
  const composerProvenance = () => composerAge.evaluate((link) => ({
    href: link.getAttribute("href"),
    authored: link.querySelector("[data-age-authored]")?.dataset.ageAuthored,
  }));
  const beforeClaim = await composerProvenance();
  const badge = team.getByRole("tab", { name: /^Filters/ }).locator(".team-mode-badge");
  assert.equal(await badge.count(), 0, "Filters wears no badge while no lane holds a task");
  // Three independent things gate a held task and every one of them refuses as
  // the same `board::allocator-empty`, so each must be satisfied deliberately:
  //
  // 1. the board must carry an eligible task, and `task add` without an
  //    `--acceptance` starts in `plan`, which `workable` never selects;
  // 2. the lineage policy must subscribe to the allocator at all, and `steer`
  //    scopes selection to `TaskSubscriptionScope::Manual` (wire/src/intent.rs),
  //    which admits only
  //    operator-pinned projects -- this fixture pins none;
  // 3. under `drive` the scope is `ManualAndAutoProject`, and a project is
  //    learned automatically only from facts a *lane* wrote:
  //    `project_subscription_history` skips `Actor::Operator | Actor::Spice`
  //    outright (board/src/next.rs). A task the operator adds is therefore
  //    invisible to allocation even while it is otherwise perfectly eligible.
  //
  // So the task is added under the lane's own identity, which both makes it
  // eligible and teaches the lane's lineage the project it lives in.
  const lifetime = team.locator('.team-lifetime input[type="range"]');
  await lifetime.fill("1");
  await team.getByRole("button", { name: "Drive", exact: true }).waitFor();
  assert.deepEqual(await composerProvenance(), beforeClaim, "Drive cannot replace provenance");
  await replyAsLane(fixture, lane, [
    "task",
    "add",
    "Held task witness",
    "--project",
    "app.browser",
    "--acceptance",
    "the watchband counts the lane holding this task",
  ]);
  const won = await replyAsLane(fixture, lane, ["task", "next"]);
  // A claim answers with the task and the actor now holding it
  // (`ClaimAnswer` in wire/src/intent.rs), and an actor crosses as text under
  // a `lane:` prefix (`Actor::LANE` in wire/src/envelope.rs). Both are read back off the
  // render below, so this observation fails if the badge counts a standing the
  // claim never reached rather than passing on any task-shaped spelling.
  assert.equal(won.body.holder, `lane:${lane}`, "the claim binds to the lane that made it");
  await badge.waitFor();
  assert.equal(await badge.innerText(), "1", "Filters counts the lane now holding a task");
  const seat = team.locator(`.team-agent[data-agent-lane="${lane}"]`);
  const placeholder = await seat.getByRole("textbox").getAttribute("placeholder");
  const [handle, phase] = placeholder.split("\n").at(-1).split(", ");
  assert.equal(handle, won.body.task, `the held card names the claimed task: ${placeholder}`);
  assert.ok(phase, `the held card names the phase it holds: ${placeholder}`);
  assert.deepEqual(await composerProvenance(), beforeClaim, "claim creation preserves provenance");
  await replyAsLane(fixture, lane, ["task", "free", handle]);
  await badge.waitFor({ state: "detached" });
  // Lapse and release both remove the active claim from the browser standing.
  assert.deepEqual(await composerProvenance(), beforeClaim, "claim release preserves provenance");
  await lifetime.fill("0");
  await team.getByRole("button", { name: "Steer", exact: true }).waitFor();
  assert.deepEqual(await composerProvenance(), beforeClaim, "Steer cannot replace provenance");

  // The status strip follows one chronology across unlike transcript shapes.
  // The first one-shot fixture record is a real Claude tool_use block decoded
  // by the production capture path, not prose styled to resemble a tool.
  const ticker = team.locator(".ticker");
  await writeFile(join(fixture, ".spice-tool-trigger"), "emit\n");
  const toolLine = `tool: ${TOOL_ACTIVITY_TEXT}`;
  await ticker.filter({ hasText: toolLine }).waitFor({ timeout: 15_000 });
  assertNewestLaneActivity(await ticker.innerText(), toolLine);
  assert.deepEqual(
    await composerProvenance(),
    beforeClaim,
    "a tool call cannot retarget the prose header or its timestamp",
  );

  // The result of that same call arrives on a separate transcript record that
  // names only the vendor correlation id. The strip has to resolve it back to
  // the call, because `toolu_01...` is not something an operator can act on.
  await writeFile(join(fixture, ".spice-tool-result-trigger"), "emit\n");
  const resultLine = `${toolLine} succeeded`;
  await ticker.filter({ hasText: resultLine }).waitFor({ timeout: 15_000 });
  assertNewestLaneActivity(await ticker.innerText(), resultLine);
  assert.deepEqual(
    await composerProvenance(),
    beforeClaim,
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
