import assert from "node:assert/strict";
import { mkdtemp, mkdir, realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { installActivityReplay } from "./activity-replay.mjs";
import { assertAgesTickWhileTheResidentIsSilent } from "./age-ticker.mjs";
import {
  assertAgentConversation, assertMosaicLattice, entirelyQuotedMessage,
  nestedQuoteMessage, nestedQuotePreview, seatAgentTeam,
} from "./agent-conversation.mjs";
import { assertFirefoxRestoredMosaic } from "./firefox-restored-mosaic.mjs";
import { assertComposerShards } from "./composer-shards.mjs";
import { assertCompleteComposerSubmission } from "./composer-submission.mjs";
import { assertComposerRecovery } from "./composer-recovery.mjs";
import { assertVacantWakeSubmission } from "./vacant-wake-submission.mjs";
import { assertCanvas } from "./canvas-readiness.mjs";
import { assertCanvasProtocolRefresh } from "./canvas-compatibility.mjs";
import { assertAuthoritySurface, driveTeam } from "./canvas-authority.mjs";
import {
  assertPerTeamWatchbands,
  captureReferenceViewport,
} from "./canvas-watchbands.mjs";
import { assertMotionPreference } from "./mosaic-motion.mjs";
import { assertTeamArrangement } from "./team-arrangement.mjs";
import { assertTeamMenu } from "./team-menu.mjs";
import { assertTaskNotes } from "./task-notes.mjs";
import { assertHandshakeTimeout } from "./handshake-timeout.mjs";
import { assertHeadlessLaunchSites, launchBrowser } from "./headless-launch.mjs";
import {
  assertEndpointProbeClosesWithoutResponse,
  assertEndpointProbeStates,
} from "./canvas-endpoint-probes.mjs";
import { assertLineageLifeIsolation } from "./canvas-lineage-life.mjs";
import { assertLineageDrainFailureTeardown } from "./canvas-lineage-teardown.mjs";
import { ownFixture, recordedVendorPids } from "./canvas-lifecycle.mjs";
import { assertBoundaryLoadReading, withLoadReading } from "./canvas-load.mjs";
import { descendantPids, emitShardEvent, startShardWorker } from "./canvas-shards.mjs";
import { assertSignalExitedChildSkipsTeardown } from "./canvas-signal-teardown.mjs";
import { assertFixtureTeardownReaps } from "./canvas-teardown.mjs";
import {
  assertFullyReapedVendorsStaySilent,
  assertSurvivingVendorsAreReported,
} from "./canvas-vendor-reap.mjs";
import {
  assertSpeechVariants,
  assertSystemSpeech,
  installBrowserSpeech,
  installSocketControl,
} from "./canvas-speech.mjs";
import {
  assertHalfOpenSocketRecovers,
  routeCanvasSocket,
} from "./half-open-socket.mjs";
import {
  assertGenerationOnline,
  assertOfflineObserved,
  assertResidentLossObserved,
  assertResidentRestart,
  assertWaitReportDistinguishesLayout,
  assertWaitReporting,
  buildResident,
  chooseCanvasView,
  configureFixture,
  declareVendorModel,
  journalExtent,
  monitorPage,
  reply,
  replyAsLane,
  seedAuthority,
  startResident,
  stopResident,
  CONNECT_MS,
  RECONNECT_MS,
  TRANSITION_MS,
  WAIT_PROBE_MS,
  waitForLocator as waitForElement,
  waitForPageFunction as waitForPage,
} from "./canvas-fixture.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const artifacts = join(root, "artifacts/canvas");
const staleEpoch = "018f9999-9999-7444-8888-999999999999";

async function waitAtFirstSpecCaller(page) {
  return waitForPage(page, "first call-site self-check", WAIT_PROBE_MS, () => false)
    .then(() => null, (error) => error);
}

async function waitAtSecondSpecCaller(page) {
  const absent = page.locator('[data-wait-call-site-self-check="absent"]');
  return waitForElement(page, "second call-site self-check", WAIT_PROBE_MS, absent)
    .then(() => null, (error) => error);
}

function reportedWaitSite(failure) {
  assert.ok(failure, "a call-site self-check that cannot be satisfied must fail");
  const site = failure.message.split("\n").find((line) => line.startsWith("  site: "));
  assert.ok(site, "the call-site self-check reports the frame that armed it");
  return site;
}

function taskStamp(handle) {
  const separator = handle.indexOf("-");
  assert.ok(separator > 0, `task handle carries a stamp: ${handle}`);
  return handle.slice(separator + 1);
}

async function assertDistinctWaitCallSites(page) {
  const first = reportedWaitSite(await waitAtFirstSpecCaller(page));
  const second = reportedWaitSite(await waitAtSecondSpecCaller(page));
  assert.match(
    first,
    /site: at waitAtFirstSpecCaller \(file:.*canvas\.spec\.mjs:\d+:\d+\)/,
    "the first wait reports its named spec caller",
  );
  assert.match(
    second,
    /site: at waitAtSecondSpecCaller \(file:.*canvas\.spec\.mjs:\d+:\d+\)/,
    "the second wait reports its named spec caller",
  );
  assert.notEqual(first, second, "two waits armed at different spec sites remain distinguishable");
}

async function seedProvenEffort(page, fixture, lane, claims) {
  const rest = await driveTeam(page);
  // One task, not one per claim. `task next` ranks peer work ahead of the
  // actor's own review only when the review was reached by freeing a claim
  // (`is_self_review` in board/src/next.rs), so a phase this lane completed
  // competes as ordinary peer work -- but only once nothing in `todo` outranks
  // it. Adding a task per turn would restock that pool every time and hand back
  // `todo` forever, which is what leaves the aggregates in a single group.
  await replyAsLane(fixture, lane, [
    "task",
    "add",
    "Effort proven witness",
    "--project",
    "app.browser",
    "--acceptance",
    "the fold prices this phase from the transcript it was spent in",
  ]);
  const priced = [];
  for (let attempt = 0; attempt < claims; attempt += 1) {
    // The allocator chooses which eligible task to hand over, and by this point
    // the fixture has freed others that are eligible too. So the handle is read
    // back off the claim rather than assumed to be the one just added.
    const won = await replyAsLane(fixture, lane, ["task", "next"]);
    assert.ok(
      won.body.task,
      `the allocator answered with a task handle: ${JSON.stringify(won.body)}`,
    );
    priced.push(won.body);
    // Fixture pools can contain older plan-only rows that outrank the task just
    // added above. Accept the exact claimed row before closing it so this
    // effort seeding measures completed work rather than allocator ordering.
    await replyAsLane(fixture, lane, [
      "task",
      "modify",
      won.body.task,
      "--acceptance",
      "the fixture can close the allocator-selected phase",
    ]);
    // `task done` names no handle: it closes the claim the actor holds, which is
    // the one just won above. It is the terminal for whichever phase that claim
    // opened -- `TaskDone` and `TaskPhased` together -- so a review phase closes
    // through exactly this call and needs no branch on which phase was handed
    // over, which the allocator's answer does not carry.
    await replyAsLane(fixture, lane, [
      "task",
      "done",
      "--validation",
      "the browser regression read this phase back off the Magnitude pane",
    ]);
  }
  await rest();
  return priced;
}

async function cancelPendingDirectives(fixture, lane) {
  const inbox = await reply(fixture, ["inbox", lane]);
  for (const directive of inbox.body.lanes[0].pending) {
    await reply(fixture, ["steer", "cancel", directive.directive]);
  }
  const quiet = await reply(fixture, ["inbox", lane]);
  assert.deepEqual(
    quiet.body.lanes[0].pending,
    [],
    "pending direction cannot choose the restart trigger under test",
  );
}

async function waitForRestartedOccupant(fixture, lane, ancestor) {
  const deadline = Date.now() + 20_000;
  let snapshot;
  while (Date.now() < deadline) {
    snapshot = await reply(fixture, ["lane"]);
    const row = snapshot.body.lanes.find((candidate) => candidate.lane === lane);
    if (row?.occupant && row.occupant !== ancestor) return row;
    await new Promise((resolve) => setTimeout(resolve, 100));
  }
  throw new Error(
    `lane ${lane} did not restart from ${ancestor}: ${JSON.stringify(snapshot?.body.lanes)}`,
  );
}

async function readEffortSurface(effort) {
  return effort.evaluate((pane) => {
    const table = (name) => pane.querySelector(`[data-effort-table="${name}"]`);
    const headings = (name) =>
      [...(table(name)?.querySelectorAll("thead th") ?? [])].map((cell) => cell.textContent);
    const counters = (name) => table(name)?.querySelectorAll("[data-effort-counter]").length ?? 0;
    const rows = (name) => [...(table(name)?.querySelectorAll("tbody tr") ?? [])];
    return {
      provenHeadings: headings("proven"),
      withheldHeadings: headings("withheld"),
      provenCounters: counters("proven"),
      withheldCounters: counters("withheld"),
      proven: rows("proven").map((row) => ({
        identity: row.dataset.effortIdentity,
        phase: row.querySelector(".tag")?.textContent ?? "",
        magnitude: row.dataset.effortMagnitude,
        continuation: row.dataset.effortContinuation,
      })),
      withheld: rows("withheld").map((row) => ({
        task: row.querySelector("td")?.textContent ?? "",
        reasons: [...row.querySelectorAll("[data-effort-reason]")].map(
          (tag) => tag.dataset.effortReason,
        ),
        counters: row.querySelectorAll("[data-effort-counter]").length,
      })),
      aggregates: rows("aggregates").map((row) => ({
        identity: row.dataset.effortIdentity,
        measure: row.dataset.effortMeasure,
        phase: row.querySelector(".tag")?.textContent ?? "",
        count: Number(row.querySelector('[data-effort-counter="count"]')?.textContent),
      })),
    };
  });
}

// Span one claimed phase across two launch identities.
//
// A phase is sized from the occupants that spent it, and `one_identity` refuses
// to price one whose occupants disagree about what was launched. Reaching that
// disagreement needs the identity to move without the phase ending. The current
// lifecycle does that through held-claim automatic wake: after the root config
// rewrite is committed, draining the host lane makes the resident relaunch it
// and record `ClaimCarried` onto the successor occupant.
//
// The session generation is deliberately left where it is. What separates the
// two occupants here is the configured identity alone, which `declareVendorModel`
// moves and each launch re-resolves from the worktree it starts in.
async function seedMixedLaunchEffort(page, fixture, lane) {
  const rest = await driveTeam(page);
  // The proven seeding above drains the eligible pool deliberately, so this one
  // restocks it. Restocking is safe here and not there: it runs after every
  // priced phase is closed, so it cannot restore the `todo` work whose absence
  // is what let a completed phase be handed back and split the aggregate groups.
  await replyAsLane(fixture, lane, [
    "task",
    "add",
    "Effort mixed launch witness",
    "--project",
    "app.browser",
    "--acceptance",
    "the fold refuses to price a phase whose occupants disagree on what was launched",
  ]);
  const won = await replyAsLane(fixture, lane, ["task", "next"]);
  assert.ok(won.body.task, `a phase to span identities: ${JSON.stringify(won.body)}`);
  await replyAsLane(fixture, lane, [
    "task",
    "modify",
    won.body.task,
    "--acceptance",
    "the fixture can close the allocator-selected mixed-launch phase",
  ]);
  const seated = await reply(fixture, ["lane"]);
  const seat = seated.body.lanes.find((row) => row.lane === lane);
  assert.ok(seat, `the lane to span identities is seated: ${JSON.stringify(seated.body.lanes)}`);
  assert.ok(seat.occupant, `the lane starts occupied: ${JSON.stringify(seat)}`);
  await cancelPendingDirectives(fixture, lane);
  await declareVendorModel(fixture, "canvas-successor-model");
  await reply(fixture, ["lane", "drain", lane]);
  await waitForRestartedOccupant(fixture, lane, seat.occupant);
  await replyAsLane(fixture, lane, [
    "task",
    "done",
    "--validation",
    "the browser regression crossed this phase over a rewritten launch identity",
  ]);
  await rest();
  return won.body.task;
}

/// Drive priced and withheld task effort through the real connection.
///
/// The fixture reaches this helper already carrying a phase the fold refuses to
/// size -- a claim it opened and freed, which the surface shows under
/// `missing-end` and `claim-freed`. `seedProvenEffort` adds the other side, so
/// both tables stand at once and each control below has a leg that would notice
/// if the surface simply stopped rendering.
async function assertTaskEffortSurface(page, fixture, seeded) {
  // Enough turns for the `todo` work the fixture already carries to drain far
  // enough that a phase this lane completed outranks what is left, which is the
  // only axis of the grouping key this fixture can move: one `[vendor.claude]`
  // section launches every lane and there is no per-lane model override, so
  // every priced phase here necessarily wears one launch identity.
  const claims = 4;
  const priced = await seedProvenEffort(page, fixture, seeded.lane, claims);
  const mixed = await seedMixedLaunchEffort(page, fixture, seeded.lane);
  await chooseCanvasView(page, "Magnitude");
  const effort = page.locator('[data-pane="effort"]');
  await effort.waitFor();
  // Wait for the closes to arrive as rendered rows rather than for a duration:
  // `task done` returns before the fact has crossed the socket and been folded.
  // Both tables are waited for together because the controls below are a
  // partition, and a pane caught with only one side built would satisfy the
  // withheld half of that partition for the wrong reason.
  // The condition waited on is the one the controls need rather than a row
  // count, so a surface that priced every phase under one key fails as a stated
  // shortfall below instead of as a wait that timed out for no stated reason.
  try {
    await page.waitForFunction(
      (want) => {
        const pane = document.querySelector('[data-pane="effort"]');
        const rows = (name) => [
          ...(pane?.querySelectorAll(`[data-effort-table="${name}"] tbody tr`) ?? []),
        ];
        const keys = new Set(
          rows("proven").map(
            (row) => `${row.dataset.effortIdentity} in ${row.querySelector(".tag")?.textContent}`,
          ),
        );
        return keys.size >= want && rows("withheld").length >= 1;
      },
      2,
      { timeout: 20_000 },
    );
  } catch (error) {
    process.stderr.write(`EFFORT-PROBE-HTML ${await effort.innerHTML()}\n`);
    throw error;
  }
  // One snapshot rather than a read per assertion: any fact invalidates the
  // effort read, so separate round trips can straddle a refresh and compare a
  // heading from one render against a row from the next.
  const mixedTask = taskStamp(mixed);
  let surface = await readEffortSurface(effort);
  for (let attempt = 0; attempt < 8; attempt += 1) {
    const spanned = surface.withheld.find((row) => row.task === mixedTask);
    if (spanned?.reasons.length === 1 && spanned.reasons[0] === "mixed-launch") break;
    await page.getByRole("button", { name: "Refresh", exact: true }).click();
    await new Promise((resolve) => setTimeout(resolve, 500));
    surface = await readEffortSurface(effort);
  }
  // Written before the first assertion, so a run that fails one control still
  // reports the whole surface the rest of them would have been judged against.
  process.stderr.write(
    `EFFORT-PROBE ${JSON.stringify(surface)} won=${JSON.stringify(priced)} `
      + `mixed=${mixed}\n`,
  );
  await effort.screenshot({ path: join(artifacts, "canvas-task-effort.png") });

  // Control: no sizing reaches an unavailable row.
  //
  // Checked on which elements exist, not on which characters appear -- the
  // reason details legitimately carry sequence numbers ("phase opened at 55"),
  // so a digit sweep over the text would either fail on those or be relaxed
  // until it proved nothing. `counter()` marks every cell that carries a
  // magnitude, which makes this a question the DOM answers exactly.
  const { provenHeadings, withheldHeadings } = surface;
  assert.deepEqual(
    withheldHeadings,
    ["Task", "Phase", "Launch", "Withheld because"],
    `the withheld table names its phase and its reasons, and has no sizing column \
at all, but rendered ${JSON.stringify(withheldHeadings)}`,
  );
  for (const sizing of [
    "Turns",
    "Renewals",
    "Wall",
    "Input",
    "Cached",
    "Cache write",
    "Output",
    "Reasoning",
  ]) {
    assert.ok(provenHeadings.includes(sizing), `the proven table sizes ${sizing}`);
    assert.ok(!withheldHeadings.includes(sizing), `no withheld row can carry ${sizing}`);
  }
  assert.equal(
    surface.withheldCounters,
    0,
    "an unavailable phase carries no token count, duration, average or total",
  );
  assert.ok(
    surface.provenCounters > 0,
    "the surface renders sizing somewhere, so the count above is a partition and not a blank pane",
  );

  // Control: aggregates group on the exact launch identity and phase.
  const provenRows = surface.proven;
  const aggregateRows = surface.aggregates;
  for (const row of [...provenRows, ...aggregateRows]) {
    assert.deepEqual(
      row.identity?.split("/").filter((part) => part !== "").length,
      3,
      `every priced row wears a whole vendor/model/effort tuple, not a prefix of one: ${row.identity}`,
    );
  }
  const key = (row) => `${row.identity} in ${row.phase}`;
  const provenGroups = new Map();
  for (const row of provenRows) {
    provenGroups.set(key(row), (provenGroups.get(key(row)) ?? 0) + 1);
  }
  // A key that had dropped either half would put these rows in one group.
  assert.ok(
    provenGroups.size > 1,
    `the seeding priced more than one grouping key: ${JSON.stringify([...provenGroups])}`,
  );
  const aggregateGroups = new Map();
  for (const row of aggregateRows) {
    const measures = aggregateGroups.get(key(row)) ?? [];
    measures.push(row.measure);
    aggregateGroups.set(key(row), measures);
    // Counted against the priced rows alone, which is also what holds the
    // withheld phase out: the surface carries more phases than these counts
    // total, so an aggregate that admitted an unsized phase would overshoot
    // rather than merely report a number nothing else disagrees with.
    assert.equal(
      row.count,
      provenGroups.get(key(row)),
      `the ${row.measure} for ${key(row)} counts that pair's phases and no others`,
    );
  }
  assert.deepEqual(
    [...aggregateGroups.keys()].sort(),
    [...provenGroups.keys()].sort(),
    "aggregates and priced rows agree on which exact pairs exist",
  );
  for (const [pair, measures] of aggregateGroups) {
    assert.deepEqual(
      measures.sort(),
      ["average", "total"],
      `${pair} is summed and averaged separately, so a sum is never read as a mean`,
    );
  }

  // Control: a phase its occupants disagree about is withheld, under that reason.
  //
  // Found by handle rather than by sweeping the table for the reason, because
  // the fixture reaches here already carrying a withheld phase, and a sweep
  // would be answered by that one no matter what the carried claim did. The
  // reasons are compared whole for the same reason `mixed-launch` is reachable
  // at all: `one_identity` is the only refusal the spanned launch earns, so a
  // row that also carried `missing-end` or `broken-carry` would mean the claim
  // did not survive the hand-off it was seeded to survive.
  const spanned = surface.withheld.find((row) => row.task === mixedTask);
  assert.ok(
    spanned,
    `the phase carried across two launch identities is withheld, but the withheld \
table holds ${JSON.stringify(surface.withheld)}`,
  );
  assert.deepEqual(
    spanned.reasons,
    ["mixed-launch"],
    `${mixed} is withheld for disagreeing about what was launched and nothing else`,
  );
  // Asserted on this row rather than only on the table total above, so it stays
  // a claim about the spanned phase even if the fixture later carries a withheld
  // phase whose blankness is doing the work.
  assert.equal(spanned.counters, 0, `no sizing cell reaches ${mixed}`);
}

async function assertMosaicComposer(page, fixture, seeded) {
  await chooseCanvasView(page, "Agents");
  const team = page.locator('[data-pane="agent-console"]');
  await team.getByRole("tab", { name: /^Compose/ }).click();
  const lane = page.locator(`.team-agent[data-agent-lane="${seeded.lane}"]`);
  const activity = team.locator(`.mosaic-message-card[data-agent-lane="${seeded.lane}"]`);
  const quoteLane = page.locator(`.team-agent[data-agent-lane="${seeded.quoteLane}"]`);
  const quoteActivity = team.locator(
    `.mosaic-message-card[data-agent-lane="${seeded.quoteLane}"]`,
  );
  await activity.filter({ hasText: "browser direction survives streaming" }).waitFor();

  await assertComposerShards({
    page,
    team,
    quoteLane,
    quoteActivity,
    seeded,
    nestedQuotePreview,
    nestedQuoteMessage,
    entirelyQuotedMessage,
    latestPrimaryProse: seeded.quoteProse,
  });

  await activity
    .filter({ hasText: "browser direction survives streaming" })
    .first()
    .getByRole("button", { name: "Quote this message" })
    .click();
  const composer = lane.locator(".composer-shard--primary");
  const quotedComposer = lane.locator(".composer-shard--quote");
  await quotedComposer.waitFor();
  // Quoting still carries its context, but on the shard that owns it rather
  // than inside a rendered copy of the draft.
  assert.equal(
    await quotedComposer.getAttribute("title"),
    "browser direction survives streaming",
    "the quoted message stays reachable from the shard it minted",
  );
  await assertCompleteComposerSubmission({
    page,
    fixture,
    lane,
    activity,
    quoteLane,
    quoteActivity,
    seeded,
    composer,
    quotedComposer,
    artifactPath: join(artifacts, "canvas-light-mosaic-composer.png"),
  });

  await lane.evaluate((target) => {
    const transfer = new DataTransfer();
    transfer.setData("text/plain", "dragged card direction");
    target.dispatchEvent(new DragEvent("drop", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
    }));
  });
  await activity.filter({ hasText: "dragged card direction" }).waitFor({
    timeout: 10_000,
  });
  const dropped = await reply(fixture, ["inbox", seeded.lane]);
  assert.ok(
    dropped.body.lanes[0].pending.some(
      (directive) => directive.text === "> dragged card direction",
    ),
    "dropping text on a lane sends quoted steering",
  );
}

/// How those same admitted facts were split into pages, under the same bracket.
///
/// Every assertion here reads packed-page metadata rather than a rendered fact
/// row, which is why it cannot be folded into the sequence proofs: a horizon is
/// not a row, so nothing about the rows can bound one.
function assertPackedPaging(provenance, horizonValues, extent, settled) {
  assert.ok(provenance.length > 1, "two-row packing produces multiple fact pages");
  assert.match(provenance[0], /omitted \/ continue/, "first page exposes overflow continuation");
  assert.doesNotMatch(provenance.at(-1), /omitted \/ continue/, "final page closes continuation");
  // A floor for the same reason the fact count is one: the cursor the caller
  // sampled has already been passed if the resident admitted anything since.
  // Reaching it is the claim; standing exactly on it was never the claim. The
  // ceiling is that same bracket, and it is the only one this metadata has.
  assert.ok(
    horizonValues.at(-1) >= extent.cursor && horizonValues.at(-1) <= settled.cursor,
    `latest page reaches the current horizon (${horizonValues.at(-1)} outside ${extent.cursor}..${settled.cursor})`,
  );
  assert.ok(
    horizonValues.every((value, index) => index === 0 || value >= horizonValues[index - 1]),
    "continued and later live pages retain monotone packed horizons",
  );
}

async function assertPackedHistory(page, fixture, extent, linked = false) {
  await chooseCanvasView(page, "Chronology");
  const journal = page.locator('[data-pane="commands"]');
  const facts = journal.locator(".event strong");
  await facts.nth(extent.facts - 1).waitFor();
  // One snapshot, and every count below is derived from it. Counting the rows
  // and reading their sequences as two separate calls would let the resident
  // admit a fact between them, and the assertions would then describe a page
  // state that never existed at any instant.
  const sequences = await facts.evaluateAll((nodes) => nodes.map((node) => {
    const value = Number(node.textContent?.split("/")[0].trim());
    if (!Number.isInteger(value)) throw new Error(`invalid rendered sequence: ${node.textContent}`);
    return value;
  }));
  // Closes the row snapshot's bracket with no page read in between, so the fact
  // ceiling below admits only what the resident took while the rows themselves
  // were read. The journal only grows, so a fact the page rendered legitimately
  // was admitted before the snapshot and is counted here too.
  const packed = await journalExtent(fixture);
  const provenance = await journal.locator('[data-subject="facts"]').allTextContents();
  const horizons = await journal
    .locator('[data-subject="facts"] span')
    .evaluateAll((nodes) => nodes.map((node) => node.textContent?.trim()).filter((text) => text?.startsWith("horizon ")));
  // The horizon's own bracket, and the reason there are two samples rather than
  // one. Nothing read after a sample can be bounded by it, so a horizon read
  // below `packed` needs a reply of its own; stretching either sample to cover
  // both would buy one ceiling by loosening the other.
  const settled = await journalExtent(fixture);
  const horizonValues = horizons.map((text) => Number(text.split(" ")[1]));
  // A floor, not an equality. `extent` was sampled before this read, and the
  // resident admits facts of its own while the suite runs -- accounting a lane's
  // transcript is one of them -- so a page that is correctly caught up can hold
  // strictly more than the sample counted. What it can never do is hold fewer.
  assert.ok(
    sequences.length >= extent.facts,
    `all ${extent.facts} admitted facts render (rendered ${sequences.length})`,
  );
  // And a ceiling, which is the half a floor alone gives up. A page inventing a
  // fact past the horizon renders a sequence no one admitted, and that tail
  // stays distinct and stays contiguous, so neither proof below would see it.
  // Only a count the journal itself bounds can.
  assert.ok(
    sequences.length <= packed.facts,
    `no fact renders that the journal never admitted (rendered ${sequences.length} > ${packed.facts})`,
  );

  assertPackedPaging(provenance, horizonValues, extent, settled);

  // This is the duplicate-application proof, and it needs no extent at all:
  // applying one fact twice renders its sequence twice, so distinct-equals-total
  // catches it whether or not the caller's sample is still current. The previous
  // form compared the distinct count against `extent.facts`, which only held
  // while the journal happened to be quiescent and was never the thing that made
  // a repeat detectable.
  assert.equal(
    new Set(sequences).size,
    sequences.length,
    "continuation fabricates no duplicate fact",
  );
  // Contiguous from one, so nothing between the origin and the rendered horizon
  // is missing either. A dropped fact fails here, a repeated one fails above,
  // and an invented one fails against the ceiling -- it is contiguous and
  // distinct, so those two are the wrong instrument for it.
  assert.deepEqual(
    [...sequences].sort((left, right) => left - right),
    Array.from({ length: sequences.length }, (_, index) => index + 1),
    "continuation loses no fact before the packed horizon",
  );

  await assertCausalThread(page, linked);

  await chooseCanvasView(page, "Chronology");
  await page.screenshot({ path: join(artifacts, "canvas-chronology.png"), fullPage: true });
}

/// The Topology walk behind the same admitted history, asserted on its own.
///
/// It reads a different pane against a different terminal condition than the
/// packed fact pages above, and shares only the page they are both views of.
async function assertCausalThread(page, linked) {
  await chooseCanvasView(page, "Topology");
  const thread = page.locator('[data-pane="lineage"]');
  const minimumRows = linked ? 2 : 1;
  const threadRowsLocator = thread.locator("tbody tr");
  await threadRowsLocator.nth(minimumRows - 1).waitFor();
  assert.ok(
    await threadRowsLocator.count() >= minimumRows,
    linked ? "multi-link causal thread renders" : "terminal causal thread renders",
  );
  // The terminal label is the tail of a walk built one admitted frame at a time,
  // not a value the row count settles. `observe_stream` inserts one CausalLink
  // per admitted frame and the walk stops at `ChainEnd::Dangling(position)` the
  // moment a position is missing (`walk` in app/src/state.rs), which renders as
  // `dangling at N` (`render_causal_thread` in app/src/browser/view.rs). Two
  // rows can be on screen
  // while headers below them are still in flight, so the row wait above leaves
  // this field mid-flight: it was observed once reading `dangling at 14` on a
  // tree that re-ran green, and CAUSAL_HEADERS is 1024, so position 14 was never
  // eviction.
  //
  // Keep the wait rejection: a walk that genuinely remains dangling or cyclic
  // is a defect, and neither terminal state is an acceptable substitute for a
  // root. The rejection carries the label it found instead, because the position
  // is the diagnosis -- reading 14 against CAUSAL_HEADERS of 1024 is what ruled
  // eviction out above, where a bare timeout carries no position at all. That
  // reading is the whole observation: a second read taken after the wait could
  // only report a later frame than the one that failed, so there is none.
  //
  // The pane carries exactly one count, and the predicate requires that rather
  // than assuming it: a second one would have `querySelector` silently answer
  // for the wrong node, and the strict locator handed to `observed` reports that
  // by name instead.
  const terminal = thread.locator(".section-count");
  await waitForPage(page, "the causal walk reaches root", TRANSITION_MS, () => {
    const counts = document.querySelectorAll('[data-pane="lineage"] .section-count');
    return counts.length === 1 && counts[0].textContent === "root";
  }, undefined, { observed: () => terminal.innerText() });
  const threadRows = await thread.locator("tbody tr").allTextContents();
  if (linked) {
    assert.ok(threadRows.some((row) => !row.endsWith("root")), "causal walk includes a linked cause");
  }
}

async function assertNoCredentialOrLandingGap(page, capabilities) {
  const content = await page.content();
  assert.doesNotMatch(content, /release::not-landed/);
  for (const capability of capabilities) {
    assert.ok(!content.includes(capability), "rendered DOM does not expose a capability");
  }
  const urls = await page.evaluate(() => performance.getEntriesByType("resource").map((entry) => entry.name));
  for (const capability of capabilities) {
    assert.ok(urls.every((url) => !url.includes(capability)), "resource URLs do not expose a capability");
  }
}

async function assertStaleGeneration(browser, endpoint, currentEpoch) {
  const context = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  const page = await context.newPage();
  const monitor = monitorPage(page);
  await page.route("**/bootstrap", async (route) => {
    const response = await route.fetch();
    const bootstrap = await response.json();
    assert.equal(bootstrap.epoch, currentEpoch);
    bootstrap.epoch = staleEpoch;
    await route.fulfill({
      response,
      contentType: "application/json; charset=utf-8",
      body: JSON.stringify(bootstrap),
    });
  });
  await page.goto(endpoint, { waitUntil: "domcontentloaded" });
  await chooseCanvasView(page, "Chronology");
  await assertOfflineObserved(page, "refusing a stale generation");
  const fault = page.locator(".event.fault");
  await fault.waitFor();
  assert.match(await fault.innerText(), /Canvas connection reset/);
  assert.match(await fault.innerText(), /accepted|frame|epoch|socket/i);
  monitor.assertClean();
  await context.close();
}

/// Drive the half-open case on a context of its own.
///
/// The route has to be in place before the first navigation, and every other
/// page in this suite has already connected by the time it gets here. Reusing
/// one would mean routing a socket that was opened before the route existed.
async function assertHalfOpenSocket(browser, endpoint, epoch, fixture, seeded) {
  const context = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  const page = await context.newPage();
  const monitor = monitorPage(page);
  const route = await routeCanvasSocket(page);
  await page.goto(endpoint, { waitUntil: "domcontentloaded" });
  // Online names the admitted handshake, not the end of the descending packed
  // backlog. Close that continuation before withholding frames, or reconnect
  // can resume a partial surface after its oldest page and retain facts 3..N.
  await assertPackedHistory(page, fixture, await journalExtent(fixture));
  let advanced;
  await assertHalfOpenSocketRecovers(page, monitor, route, epoch, async () => {
    await reply(fixture, ["task", "note", seeded.task, "written while the socket was half open"]);
    advanced = await journalExtent(fixture);
  });
  // Taken during the outage, so it counts a fact the page could not have had
  // and the resume cannot pass vacuously. It is a floor and not a total: the
  // resident keeps admitting facts of its own while this runs, so an extent
  // sampled here is already behind by the time the page is read.
  // `assertPackedHistory` proves duplicate application from the rendered
  // sequences themselves rather than from that count.
  await assertPackedHistory(page, fixture, advanced);
  monitor.assertClean();
  await context.close();
}

async function assertSameGenerationResume(
  page,
  monitor,
  fixture,
  seeded,
  epoch,
  beforeExtent,
) {
  await chooseCanvasView(page, "Chronology");
  const before = await page.locator('[data-pane="commands"] .event strong').allTextContents();
  monitor.expectNetworkInterruption(true);
  await page.evaluate(() => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    socket.close(4000, "fixture reconnect");
  });
  await assertOfflineObserved(page, "the fixture closes its socket");
  assert.match(await page.locator(".status-copy").innerText(), /Reconnecting/);
  await chooseCanvasView(page, "Chronology");
  await page.locator(".event.fault").waitFor();

  await reply(fixture, ["task", "note", seeded.task, "written during browser disconnect"]);
  const advanced = await journalExtent(fixture);
  assert.ok(advanced.facts > beforeExtent.facts, "offline command advances resident authority");

  await assertGenerationOnline(page, epoch, "the socket is reopened", RECONNECT_MS);
  monitor.expectNetworkInterruption(false);
  await assertPackedHistory(page, fixture, advanced);
  const after = await page.locator('[data-pane="commands"] .event strong').allTextContents();
  assert.ok(after.length > before.length, "same-generation resume appends missed facts");
}

assertBoundaryLoadReading();
const run = startShardWorker(process.argv.slice(2));
if (process.env.SPICE_CANVAS_PREBUILT !== "1") await buildResident();
let fixture;
let linkedRoot;
let owner;
let resident;
let browser;
try {
  if (run.owns("ownership")) {
    await run.scenario("launch-boundaries", async () => {
      await assertHeadlessLaunchSites();
      await assertEndpointProbeClosesWithoutResponse();
      await assertEndpointProbeStates();
      await assertSignalExitedChildSkipsTeardown();
      await assertLineageDrainFailureTeardown();
    });
    await run.scenario("fixture-reaping", async () => {
      await assertSurvivingVendorsAreReported();
      await assertFullyReapedVendorsStaySilent();
      await assertFixtureTeardownReaps();
    });
  } else {
    fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-browser-")));
    linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-lanes-")));
    owner = ownFixture({ fixture, linkedRoot });
    await mkdir(artifacts, { recursive: true });
    const worktrees = await configureFixture(fixture, linkedRoot, 6, { noCardMember: 5 });
    resident = owner.ownResident(await startResident(fixture));
    const seeded = await seedAuthority(fixture);
    browser = await launchBrowser();
    let light;
    let lightPage;
    let lightMonitor;
    let hiddenStatusReport;
    let configuredPolicy;
    let dark;
    let darkMonitor;
    if (run.owns("readiness")) {
      await run.scenario("light-readiness", async () => {
        await assertHandshakeTimeout(browser, resident.discovery.endpoint);
        await assertCanvasProtocolRefresh(browser, resident.discovery.endpoint);
        await assertLineageLifeIsolation(browser);
        light = await browser.newContext({ colorScheme: "light", viewport: { width: 1440, height: 900 } });
        await installBrowserSpeech(light);
        await installSocketControl(light);
        lightPage = await light.newPage();
        lightMonitor = monitorPage(lightPage);
        lightPage.on("console", (message) => process.stderr.write(`browser console: ${message.text()}\n`));
        await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
        await assertCanvas(lightPage, "light");
        await assertDistinctWaitCallSites(lightPage);
        await assertWaitReporting(lightPage, resident);
        lightMonitor.assertClean();
        await light.close();
      });
    } else if (run.owns("interaction")) {
      light = await browser.newContext({ colorScheme: "light", viewport: { width: 1440, height: 900 } });
      const activityReplay = await installActivityReplay(light);
      await installBrowserSpeech(light);
      await installSocketControl(light);
      lightPage = await light.newPage();
      lightMonitor = monitorPage(lightPage);
      lightPage.on("console", (message) => process.stderr.write(`browser console: ${message.text()}\n`));
      await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
      await assertCanvas(lightPage, "light");
      await run.scenario("agent-conversation", async () => {
        Object.assign(seeded, await assertAgentConversation(
          lightPage, fixture, worktrees, activityReplay, artifacts,
        ));
        await assertTeamMenu(lightPage, fixture);
      });
      await run.scenario("firefox-restored-mosaic", async () => {
        await assertFirefoxRestoredMosaic(resident.discovery.endpoint, artifacts);
      });
      await run.scenario("reference-composer", async () => {
        const reference = await browser.newContext({ colorScheme: "dark", viewport: { width: 1000, height: 568 } });
        await installBrowserSpeech(reference);
        const page = await reference.newPage();
        const monitor = monitorPage(page);
        await page.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
        await captureReferenceViewport(page, artifacts);
        monitor.assertClean();
        await reference.close();
        seeded.extent = await journalExtent(fixture);
        await assertAuthoritySurface(lightPage, seeded);
        await assertMosaicComposer(lightPage, fixture, seeded);
        await assertComposerRecovery(lightPage, fixture, seeded, lightMonitor);
        await assertVacantWakeSubmission(lightPage, fixture, seeded);
        await assertPackedHistory(lightPage, fixture, await journalExtent(fixture), true);
        await assertNoCredentialOrLandingGap(lightPage, [resident.discovery.capability]);
        await assertSystemSpeech(lightPage);
      });
      await run.scenario("authority-task-flow", async () => {
        await assertTaskNotes(lightPage, seeded.task);
        await assertPackedHistory(lightPage, fixture, await journalExtent(fixture));
        await assertTaskEffortSurface(lightPage, fixture, seeded);
        lightMonitor.assertClean();
        await light.close();
      });
    } else {
      await run.scenario("variant-surfaces", async () => {
        light = await browser.newContext({ colorScheme: "light", viewport: { width: 1440, height: 900 } });
        await installBrowserSpeech(light);
        await installSocketControl(light);
        lightPage = await light.newPage();
        lightMonitor = monitorPage(lightPage);
        await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
        await assertCanvas(lightPage, "light-resilience");
        hiddenStatusReport = await assertWaitReporting(lightPage, resident);
        const team = await seatAgentTeam(lightPage, fixture, worktrees);
        Object.assign(seeded, {
          lane: team.lane,
          quoteLane: team.quoteLane,
          agentName: team.agentName,
          quoteProse: team.quoteProse,
          primaryHeadings: team.primaryHeadings,
        });
        await reply(fixture, ["steer", seeded.lane, "browser direction survives streaming"]);
        for (const [index, member] of team.seated.entries()) {
          const rows = Array.from({ length: 18 + index * 6 }, (_, row) => `- row ${row + 1}`);
          await reply(fixture, ["steer", member.lane,
            [`resilience lattice witness ${index + 1}`, ...rows].join("\n")]);
        }
        seeded.extent = await journalExtent(fixture);
        dark = await browser.newContext({ colorScheme: "dark", viewport: { width: 390, height: 844 } });
        await installBrowserSpeech(dark);
        const page = await dark.newPage();
        darkMonitor = monitorPage(page);
        await page.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
        await assertCanvas(page, "dark-narrow");
        await assertMosaicLattice(page, false);
        await assertAuthoritySurface(page, seeded);
        await assertNoCredentialOrLandingGap(page, [resident.discovery.capability]);
        await assertMotionPreference(browser, resident.discovery.endpoint, artifacts);
        await assertSpeechVariants(browser, resident.discovery.endpoint);
        await assertStaleGeneration(browser, resident.discovery.endpoint, resident.discovery.epoch);
      });
      await run.scenario("socket-resilience", async () => {
        const extent = await journalExtent(fixture);
        await assertSameGenerationResume(
          lightPage, lightMonitor, fixture, seeded, resident.discovery.epoch, extent,
        );
        await assertHalfOpenSocket(
          browser, resident.discovery.endpoint, resident.discovery.epoch, fixture, seeded,
        );
        await assertAgesTickWhileTheResidentIsSilent(
          browser, resident.discovery.endpoint, fixture, seeded.lane,
        );
      });
      await run.scenario("team-lifecycle", async () => {
        configuredPolicy = await assertPerTeamWatchbands(
          lightPage,
          fixture,
          worktrees,
          { artifacts, staleEpoch },
        );
        await assertTeamArrangement(lightPage, fixture, worktrees, artifacts);
        lightMonitor.assertClean();
        darkMonitor.assertClean();
        await dark.close();
      });
      await run.scenario("resident-restart", async () => {
        const firstEpoch = resident.discovery.epoch;
        const firstCapability = resident.discovery.capability;
        lightMonitor.expectNetworkInterruption(true);
        await chooseCanvasView(lightPage, "Chronology");
        await assertWaitReportDistinguishesLayout(lightPage, resident, hiddenStatusReport);
        await stopResident(fixture, resident);
        await assertResidentLossObserved(lightPage, resident);
        const restart = { epoch: firstEpoch, capability: firstCapability, policy: configuredPolicy };
        resident = owner.ownResident(await assertResidentRestart(fixture, lightPage, restart));
        lightMonitor.expectNetworkInterruption(false);
        await assertCanvas(lightPage, "light-restarted");
        await chooseCanvasView(lightPage, "Chronology");
        assert.equal(await lightPage.getByText(/Note appended to/).count(), 0,
          "new generation clears prior command reply state");
        await assertPackedHistory(lightPage, fixture, await journalExtent(fixture));
        await assertAuthoritySurface(lightPage, seeded);
        await assertNoCredentialOrLandingGap(
          lightPage, [firstCapability, resident.discovery.capability],
        );
        lightMonitor.assertClean();
        await light.close();
      });
    }
  }
} catch (error) {
  throw withLoadReading(error);
} finally {
  if (owner) {
    const pids = [...new Set([process.pid, resident?.child.pid,
      ...await recordedVendorPids(fixture), ...descendantPids()])].filter(Number.isInteger);
    try {
      if (browser) await browser.close();
    } finally {
      await owner.cleanup();
      emitShardEvent({ kind: "ownership", shard: run.shard.name, pids, fixture, linkedRoot });
    }
  }
}
run.finish();
