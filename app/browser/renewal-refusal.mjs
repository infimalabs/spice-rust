import assert from "node:assert/strict";

import { TRANSITION_MS, waitForPageFunction } from "./canvas-fixture.mjs";

const RENEWAL_ISSUE = "Could not renew this agent";
const STALE_TOPOLOGY_REVISION = 0;

/// Prove a refused renewal reports its reason beside the control that sent it.
///
/// The browser is made stale exactly the way an operator meets it: the typed
/// command leaves with a topology revision the resident has already moved
/// past, so the resident refuses it and the durable lane state gives the
/// control nothing new to render. The refusal must still land as the lane
/// composer issue, the same line a refused wake reports on, and a later
/// accepted request must clear only that line.
export async function assertRenewalRefusalReason(page, surface) {
  const menu = surface
    .locator(
      ".composer-shard--primary .agent-action-menu:has(.agent-renew-request:not(:disabled))",
    )
    .first();
  await menu.waitFor();
  const renew = menu.locator(".agent-renew-request");
  const lane = await renew.getAttribute("data-renewal-lane");
  assert.ok(lane, "the renewal control names its lane");
  const composer = surface.locator(
    `.team-agent[data-agent-lane="${lane}"] .composer-shard--primary`,
  );
  const issue = composer.locator(".composer-issue");
  assert.equal(await issue.count(), 0, "the composer starts without an issue line");
  assert.equal(await renew.getAttribute("data-renewal-state"), "none");
  assert.equal(await renew.getAttribute("data-renewal-blocked"), null);

  await installStaleRenewalRevision(page);
  try {
    await menu.locator(":scope > summary").click();
    await renew.click();
    await waitForPageFunction(
      page,
      "a refused renewal reports its reason in the lane composer",
      TRANSITION_MS * 4,
      ({ lane, prefix }) => document
        .querySelector(
          `.team-agent[data-agent-lane="${lane}"] .composer-shard--primary .composer-issue`,
        )
        ?.textContent?.startsWith(prefix) === true,
      { lane, prefix: RENEWAL_ISSUE },
    );
  } finally {
    await restoreRenewalRevision(page);
  }
  assert.match(
    await issue.innerText(),
    /^Could not renew this agent: lane::renewal-topology-moved\. read lane state/,
    "the composer issue carries the exact refusal reason and its explanation",
  );
  assert.equal(await issue.getAttribute("role"), "status");
  assert.equal(
    await renew.getAttribute("data-renewal-state"),
    "none",
    "a refused request leaves the durable control untouched",
  );
  assert.equal(await renew.isDisabled(), false, "the refused request remains retryable");
  assert.equal(await menu.getAttribute("open"), null, "the refused click still closes the drawer");

  await menu.locator(":scope > summary").click();
  await renew.click();
  await waitForPageFunction(
    page,
    "an accepted renewal clears the refusal it retried",
    TRANSITION_MS * 4,
    (lane) => {
      const agent = document.querySelector(`.team-agent[data-agent-lane="${lane}"]`);
      return agent?.querySelector(".agent-renew-request")?.dataset.renewalState === "requested"
        && agent.querySelector(".composer-shard--primary .composer-issue") === null;
    },
    lane,
  );
  await menu.locator(":scope > summary").click();
  await renew.click();
  await waitForPageFunction(
    page,
    "cancelling the retried renewal restores the idle control",
    TRANSITION_MS * 4,
    (lane) => document
      .querySelector(`.agent-renew-request[data-renewal-lane="${lane}"]`)
      ?.dataset.renewalState === "none",
    lane,
  );
  assert.equal(await issue.count(), 0, "an accepted cancel raises no issue line");
}

/// Prove a disabled renewal control names the condition withholding it.
///
/// The lane has no occupant, so the control must say so inline rather than
/// leaving a greyed button whose hover a disabled control cannot show.
export async function assertBlockedRenewalControl(lane, agent) {
  const renewal = await lane
    .locator(".composer-shard--primary .agent-renew-request")
    .evaluate((button) => ({
      disabled: button.disabled,
      state: button.dataset.renewalState,
      blocked: button.dataset.renewalBlocked,
      label: button.getAttribute("aria-label"),
      title: button.getAttribute("title"),
      inline: button.querySelector(".agent-renew-blocked")?.textContent,
    }));
  assert.deepEqual(
    renewal,
    {
      disabled: true,
      state: "none",
      blocked: "no occupant is seated",
      label: `Request agent renewal for ${agent} (unavailable: no occupant is seated)`,
      title: `Start a fresh successor for ${agent}. Unavailable: no occupant is seated`,
      inline: "no occupant is seated",
    },
    "the vacant renewal control names its block inline, in its name, and in its hover",
  );
}

/// Rewrite the next typed renewal so it names a topology the resident moved past.
async function installStaleRenewalRevision(page) {
  await page.evaluate((revision) => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    const send = socket.send.bind(socket);
    window.__renewalRevisionRestore = () => {
      socket.send = send;
    };
    const rewrite = (bytes) => {
      const command = JSON.parse(new TextDecoder().decode(bytes));
      if (command.kind !== "lane::renewal") return bytes;
      command.body.topology_revision = revision;
      return new TextEncoder().encode(JSON.stringify(command));
    };
    socket.send = (data) => {
      if (ArrayBuffer.isView(data)) {
        send(rewrite(new Uint8Array(data.buffer, data.byteOffset, data.byteLength)));
      } else if (data instanceof ArrayBuffer) {
        send(rewrite(new Uint8Array(data)));
      } else {
        send(data);
      }
    };
  }, STALE_TOPOLOGY_REVISION);
}

async function restoreRenewalRevision(page) {
  await page.evaluate(() => {
    window.__renewalRevisionRestore?.();
    delete window.__renewalRevisionRestore;
  });
}
