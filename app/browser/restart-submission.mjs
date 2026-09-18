// A resident restart leaves the composer honest about what it can submit.
//
// The surface keeps its last admitted runtime on screen through the outage and
// through the restarted resident's hydration, so the team console keeps looking
// live. Before BROWSER-dStDhMQS1 nothing below the runtime consulted the
// connection: the mosaic drew no connection state at all, and each composer
// derived its target from the remembered occupant and epoch alone, so a draft
// submitted during the outage was cleared, parked in the submission queue, sent
// on the first frame of the restarted generation with a witness that
// generation never seated, and refused with `steer::residency-moved` or
// `steer::witness-moved` -- a "Message was not sent" the operator read as the
// surface having stopped working until a reload gave it a runtime with no
// past. Two things are pinned here: the outage is visible where the composer
// is, and a draft with no live target stays a draft.

import assert from "node:assert/strict";

import {
  RECONNECT_MS,
  TRANSITION_MS,
  reply,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

/// One lane's composer inside the live team console.
function laneComposer(page, lane) {
  return page
    .locator('[data-pane="agent-console"]')
    .locator(`.team-agent[data-agent-lane="${lane}"]`);
}

/// What the composer says it can do with its current draft.
function submissionState(composer) {
  return composer.evaluate((node) => ({
    wakeAndSend: node.dataset.wakeAndSend,
    eligible: node.querySelector(".composer-shard--primary")?.dataset.commandSubmitEligible,
    draft: node.querySelector("textarea")?.value,
    send: node.closest("form")?.querySelector(".team-send")?.disabled,
    issue: node.querySelector(".composer-issue")?.textContent ?? null,
    submissions: node.closest(".shell")?.dataset.submissionCount,
    status: document.querySelector(".commandbar .status")?.className ?? null,
  }));
}

/// While the resident is gone, the mosaic says so and the draft goes nowhere.
///
/// Called with the page already reporting offline in the live team view. The
/// draft is typed, submitted through the keyboard and the form, and must still
/// be in the composer afterwards with nothing queued: a submission parked now
/// could only be refused later. Returns the draft so the restart half can send
/// exactly the text the outage refused.
export async function assertOutageKeepsDraft(page, lane) {
  const composer = laneComposer(page, lane);
  const textbox = composer.locator(".composer-shard--primary").getByRole("textbox");
  const send = composer.locator("xpath=ancestor::form[1]").locator(".team-send");
  const draft = `outage draft ${crypto.randomUUID()}`;
  await textbox.fill(draft);
  const before = await submissionState(composer);
  assert.deepEqual(
    { wakeAndSend: before.wakeAndSend, eligible: before.eligible, send: before.send },
    { wakeAndSend: "false", eligible: "false", send: true },
    `an offline composer offers no submission: ${JSON.stringify(before)}`,
  );
  assert.match(
    before.status ?? "",
    /\boffline\b/,
    `the live team view carries the offline state beside its composers: ${JSON.stringify(before)}`,
  );
  await textbox.focus();
  await page.keyboard.press("Control+Enter");
  await send.click({ force: true });
  const after = await submissionState(composer);
  assert.deepEqual(
    { draft: after.draft, submissions: after.submissions, issue: after.issue },
    { draft, submissions: "0", issue: null },
    `an offline submission leaves the draft in place and queues nothing: ${JSON.stringify(after)}`,
  );
  return draft;
}

/// Once the restarted resident is online the same draft submits without a
/// reload, under the generation that is serving now.
///
/// The composer regains its target in the render that reports online, so the
/// eligibility wait is a transition. The direction is then proven twice: the
/// `steer::sent` fact arrives on the socket of the restarted generation, and
/// the durable inbox carries the exact text.
export async function assertRestartedResidentAcceptsDraft(page, fixture, lane, draft, epoch) {
  const composer = laneComposer(page, lane);
  const textbox = composer.locator(".composer-shard--primary").getByRole("textbox");
  await waitForPage(
    page,
    "the restarted resident returns the composer's submission target",
    TRANSITION_MS,
    (laneId) => {
      const member = document.querySelector(`.team-agent[data-agent-lane="${laneId}"]`);
      return member?.querySelector(".composer-shard--primary")?.dataset.commandSubmitEligible === "true";
    },
    lane,
    { observed: () => submissionState(composer) },
  );
  assert.equal(await textbox.inputValue(), draft, "the outage draft survives the restart");
  await page.evaluate(({ laneId, text, generation }) => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    window.__restartDirectionFact = new Promise((resolve, reject) => {
      socket.addEventListener("message", async (event) => {
        const bytes = typeof event.data === "string"
          ? event.data
          : event.data instanceof Blob
            ? await event.data.text()
            : new TextDecoder().decode(event.data);
        const frame = JSON.parse(bytes);
        if (frame.stream === "fact"
          && frame.fact?.fact?.kind === "steer::sent"
          && frame.fact.fact.lane === laneId
          && frame.fact.fact.text === text) {
          if (frame.epoch !== generation) {
            reject(new Error(`direction was sent under ${frame.epoch}, not ${generation}`));
          }
          resolve(frame.fact.fact);
        }
      });
    });
  }, { laneId: lane, text: draft, generation: epoch });
  await textbox.focus();
  await page.keyboard.press("Control+Enter");
  assert.equal(await textbox.inputValue(), "", "an online submission clears the draft");
  const sent = await page.evaluate((timeout) => Promise.race([
    window.__restartDirectionFact,
    new Promise((_, reject) => setTimeout(
      () => reject(new Error("the restarted resident never acknowledged the direction")),
      timeout,
    )),
  ]), RECONNECT_MS);
  const inbox = await reply(fixture, ["inbox", lane]);
  const durable = inbox.body.lanes[0].pending.find(
    (candidate) => candidate.directive === sent.directive,
  );
  assert.equal(
    durable?.text,
    draft,
    `the restarted resident durably holds the direction: ${JSON.stringify(inbox.body)}`,
  );
  await waitForPage(
    page,
    "the accepted submission leaves the queue",
    TRANSITION_MS,
    () => document.querySelector(".shell")?.dataset.submissionCount === "0",
    undefined,
    { observed: () => submissionState(composer) },
  );
}
