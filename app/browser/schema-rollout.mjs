import assert from "node:assert/strict";

import {
  reply,
  TRANSITION_MS,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

function environmentCounts(text) {
  const line = text.split("\n").find((candidate) => candidate.startsWith("environment "));
  assert.ok(line, `task status names its environment: ${text}`);
  const matched = line.match(
    /^environment journal-schema required=(\S+) observed=(\d+) trust-grant required=(\S+) satisfied=(\d+) blocked=(\d+)$/,
  );
  assert.ok(matched, `task status environment is canonical: ${line}`);
  return {
    required: matched[1],
    observed: Number(matched[2]),
    trustRequired: matched[3],
    satisfied: Number(matched[4]),
    blocked: Number(matched[5]),
  };
}

async function pillCounts(pill) {
  return {
    ready: Number(await pill.getAttribute("data-ready")),
    active: Number(await pill.getAttribute("data-active")),
    unavailable: Number(await pill.getAttribute("data-unavailable")),
  };
}

async function waitForPill(page, stem, expected, label) {
  await waitForPage(page, label, TRANSITION_MS, ({ stem, expected }) => {
    const pill = document.querySelector(`.task-pill[data-task-stem="${stem}"]`);
    return pill
      && Number(pill.dataset.ready) === expected.ready
      && Number(pill.dataset.active) === expected.active
      && Number(pill.dataset.unavailable) === expected.unavailable;
  }, { stem, expected });
}

export async function assertEnvironmentTaskRollout(page, fixture) {
  const pill = page.locator('.task-pill[data-task-stem="app"]');
  await pill.waitFor({ timeout: TRANSITION_MS });
  const baselinePill = await pillCounts(pill);
  const baselineStatus = environmentCounts((await reply(fixture, ["task", "status"])).body.text);
  const journal = await reply(fixture, ["journal"]);
  assert.equal(journal.body.schema, baselineStatus.observed);
  const futureSchema = journal.body.schema + 1;

  const added = await reply(fixture, [
    "task",
    "add",
    "Canvas environment-gated rollout witness",
    "--project",
    "app.environment",
    "--requires-journal-schema",
    String(futureSchema),
  ]);
  const blockedPill = {
    ...baselinePill,
    unavailable: baselinePill.unavailable + 1,
  };
  await waitForPill(page, "app", blockedPill, "future-schema task becomes unavailable in Canvas");
  const blockedStatus = environmentCounts((await reply(fixture, ["task", "status"])).body.text);
  assert.deepEqual(
    blockedStatus,
    {
      required: String(futureSchema),
      observed: journal.body.schema,
      trustRequired: baselineStatus.trustRequired,
      satisfied: baselineStatus.satisfied,
      blocked: baselineStatus.blocked + 1,
    },
    "CLI status and Canvas derive the same environment-blocked task",
  );
  const blockedShow = (await reply(fixture, ["task", "show", added.body.task])).body.text;
  assert.match(
    blockedShow,
    new RegExp(
      `journal-schema required=${futureSchema} observed=${journal.body.schema} satisfied=false`,
    ),
  );
  assert.match(blockedShow, /environment blocked/);

  await reply(fixture, ["task", "require", added.body.task, "--clear"]);
  const clearedPill = {
    ...baselinePill,
    ready: baselinePill.ready + 1,
  };
  await waitForPill(page, "app", clearedPill, "cleared requirement becomes ready without wake");
  const clearedShow = (await reply(fixture, ["task", "show", added.body.task])).body.text;
  assert.match(clearedShow, /requirements:\n  -\nenvironment satisfied/);

  await reply(fixture, [
    "task",
    "require",
    added.body.task,
    "--journal-schema",
    String(journal.body.schema),
  ]);
  await waitForPill(page, "app", clearedPill, "satisfied requirement stays ready without wake");
  const satisfiedStatus = environmentCounts(
    (await reply(fixture, ["task", "status"])).body.text,
  );
  assert.deepEqual(
    satisfiedStatus,
    {
      required: String(journal.body.schema),
      observed: journal.body.schema,
      trustRequired: baselineStatus.trustRequired,
      satisfied: baselineStatus.satisfied + 1,
      blocked: baselineStatus.blocked,
    },
    "a satisfied observation is ready while waiting remains unrelated",
  );

  await reply(fixture, ["task", "delete", added.body.task, "browser rollout fixture settled"]);
  await waitForPill(page, "app", baselinePill, "settled rollout fixture leaves task pills unchanged");
}
