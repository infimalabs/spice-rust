// Team-life isolation scenario for the Canvas browser suite.
import assert from "node:assert/strict";
import { mkdtemp, realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { childExited } from "./canvas-child-state.mjs";
import {
  chooseCanvasView,
  configureFixture,
  monitorPage,
  reply,
  startResident,
} from "./canvas-fixture.mjs";
import { ownFixture } from "./canvas-lifecycle.mjs";
import { assertTiledWorkspace } from "./tiled-workspace.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const artifacts = join(root, "artifacts/canvas");

/// Drain every live team seat, report a failed graceful attempt, and always
/// continue through the fixture owner's non-throwing cleanup boundary.
export async function cleanupTeamFixture({ fixture, resident, owner, request = reply }) {
  try {
    if (resident && !childExited(resident.child)) {
      const liveTeam = await request(fixture, ["lane"]);
      for (const agent of liveTeam.body.lanes) {
        if (agent.occupant) await request(fixture, ["lane", "drain", agent.lane]);
      }
    }
  } catch (error) {
    process.stderr.write(`canvas team teardown could not drain lanes: ${error}\n`);
  }
  await owner.cleanup();
}

async function admitSevenMemberTeam(fixture, worktrees) {
  await reply(fixture, [
    "lane", "add", worktrees[0], "--life", "Steer", "--vendor", "claude",
  ]);
  const first = await reply(fixture, ["lane"]);
  const host = first.body.lanes.find((lane) => lane.worktree === worktrees[0]);
  assert.ok(host, "the seven-member team has an admitted host");
  for (const worktree of worktrees.slice(1)) {
    await reply(fixture, [
      "lane", "add", worktree,
      "--team", host.team,
      "--life", "Steer",
      "--vendor", "claude",
    ]);
  }
  const snapshot = await reply(fixture, ["lane"]);
  const members = snapshot.body.lanes.filter(
    (lane) => lane.team === host.team,
  );
  assert.equal(members.length, 7, "ordinary lane admission retains all seven members");
  assert.deepEqual(
    members.map((member) => member.worktree).sort(),
    [...worktrees].sort(),
    "the seven-member team contains exactly the admitted worktrees",
  );
  assert.ok(
    members.every((member) => !member.occupant),
    "layout proof begins before any team member is occupied",
  );
  return { host, members };
}

async function findSevenMemberComposer(context, endpoint, teamId) {
  const page = await context.newPage();
  const monitor = monitorPage(page);
  await page.goto(endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(page, "Chat");
  const surface = page.locator(`.team-surface[data-team="${teamId}"]`);
  await surface.waitFor();
  await surface.getByRole("tab", { name: /^Chat/ }).click();
  const strip = surface.locator(":scope > .team-pane .team-composer > .team-agents");
  await strip.locator(":scope > .team-agent").nth(6).waitFor();
  return { monitor, strip };
}

async function measureComposerStrip(strip) {
  return strip.evaluate((node) => {
    return {
      weight: node.closest(".team-surface")?.dataset.teamWeight,
      overflowX: getComputedStyle(node).overflowX,
      clientWidth: node.clientWidth,
      scrollWidth: node.scrollWidth,
      bodyWidth: document.body.scrollWidth,
      viewportWidth: innerWidth,
      members: [...node.querySelectorAll(":scope > .team-agent")].map((member) => {
        const box = member.getBoundingClientRect();
        return {
          accent: member.dataset.accent,
          lane: member.dataset.agentLane,
          left: box.left,
          top: box.top,
        };
      }),
    };
  });
}

function assertComposerStripGeometry(geometry, members) {
  const detail = JSON.stringify(geometry);
  assert.equal(
    geometry.weight,
    "6",
    `seven members retain the six-share host ceiling: ${detail}`,
  );
  assert.deepEqual(
    geometry.members.map(({ lane }) => lane),
    members.map(({ lane }) => lane),
    "all seven composers retain resident member order",
  );
  assert.deepEqual(
    geometry.members.map(({ accent }) => accent),
    ["0", "5", "4", "3", "2", "1", "0"],
    "the complete predecessor palette starts green at the right edge and wraps at the far left",
  );
  assert.ok(
    geometry.members.every(({ top }) => Math.abs(top - geometry.members[0].top) <= 1),
    `the implicit seventh track stays in the one composer row: ${detail}`,
  );
  assert.ok(
    geometry.members.every((member, index) =>
      index === 0 || geometry.members[index - 1].left < member.left),
    `the composer row remains ordered west to east: ${detail}`,
  );
  assert.ok(
    ["auto", "scroll"].includes(geometry.overflowX)
      && geometry.scrollWidth > geometry.clientWidth,
    `the composer strip owns horizontal overflow: ${detail}`,
  );
  assert.ok(
    geometry.bodyWidth <= geometry.viewportWidth,
    `strip overflow never widens the page: ${detail}`,
  );
}

async function assertEasternComposerVisible(strip) {
  const eastern = await strip.evaluate((node) => {
    node.style.scrollBehavior = "auto";
    node.scrollLeft = node.scrollWidth;
    const stripBox = node.getBoundingClientRect();
    const last = node.querySelector(":scope > .team-agent:last-child")?.getBoundingClientRect();
    return {
      scrollLeft: node.scrollLeft,
      stripRight: stripBox.right,
      lastRight: last?.right ?? Number.POSITIVE_INFINITY,
    };
  });
  assert.ok(
    eastern.scrollLeft > 0 && eastern.lastRight <= eastern.stripRight + 1,
    `eastward strip scroll reveals member seven: ${JSON.stringify(eastern)}`,
  );
}

/// Prove a team larger than the six-share visual ceiling remains one
/// ordered horizontal composer strip. The seven seats are admitted through the
/// ordinary vacant `lane add` path before any occupant can make topology and
/// task allocation interact, so this fixture isolates layout from lifecycle.
export async function assertSevenMemberComposerStrip(browser) {
  const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-seven-member-")));
  const linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-seven-lanes-")));
  const owner = ownFixture({ fixture, linkedRoot });
  let resident;
  let context;
  try {
    const worktrees = await configureFixture(fixture, linkedRoot, 7);
    resident = owner.ownResident(await startResident(fixture));
    const pending = await reply(fixture, ["trust"]);
    await reply(fixture, ["trust", "grant", pending.body.digest]);
    const { host, members } = await admitSevenMemberTeam(fixture, worktrees);

    for (const colorScheme of ["light", "dark"]) {
      context = await browser.newContext({
        colorScheme,
        viewport: { width: 520, height: 720 },
      });
      const { monitor, strip } = await findSevenMemberComposer(
        context,
        resident.discovery.endpoint,
        host.team,
      );
      assertComposerStripGeometry(await measureComposerStrip(strip), members);
      await assertEasternComposerVisible(strip);
      monitor.assertClean();
      await context.close();
      context = undefined;
    }
  } finally {
    if (context) await context.close();
    await cleanupTeamFixture({ fixture, resident, owner });
  }
}

export async function assertTeamLifeIsolation(browser) {
  const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-team-life-")));
  const linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-life-lanes-")));
  // This scenario is nested inside a run that already owns a fixture of its
  // own. Both owners are registered together and share one set of signal
  // handlers, so an interruption here tears down the outer fixture too rather
  // than only whichever happens to be innermost.
  const owner = ownFixture({ fixture, linkedRoot });
  let resident;
  let context;
  try {
    const worktrees = await configureFixture(fixture, linkedRoot, 8);
    resident = owner.ownResident(await startResident(fixture));
    const pending = await reply(fixture, ["trust"]);
    await reply(fixture, ["trust", "grant", pending.body.digest]);

    await reply(fixture, [
      "lane", "add", worktrees[0], "--life", "Drain", "--vendor", "claude",
    ]);
    const firstSnapshot = await reply(fixture, ["lane"]);
    const host = firstSnapshot.body.lanes.find((lane) => lane.worktree === worktrees[0]);
    assert.ok(host, "the four-member Drain team has a host");
    for (const worktree of worktrees.slice(1, 4)) {
      await reply(fixture, [
        "lane", "add", worktree,
        "--team", host.team,
        "--life", "Drain",
        "--vendor", "claude",
      ]);
    }
    await reply(fixture, [
      "lane", "add", worktrees[4], "--life", "Drive", "--vendor", "claude",
    ]);
    const secondSnapshot = await reply(fixture, ["lane"]);
    const drive = secondSnapshot.body.lanes.find((lane) => lane.worktree === worktrees[4]);
    assert.ok(drive, "the four-member Drive team has a host");
    for (const worktree of worktrees.slice(5)) {
      await reply(fixture, [
        "lane", "add", worktree,
        "--team", drive.team,
        "--life", "Drive",
        "--vendor", "claude",
      ]);
    }
    const snapshot = await reply(fixture, ["lane"]);
    const drainMembers = snapshot.body.lanes.filter(
      (lane) => lane.team === host.team,
    );
    const driveMembers = snapshot.body.lanes.filter(
      (lane) => lane.team === drive.team,
    );
    assert.equal(drainMembers.length, 4, "one admitted team contains four members");
    assert.ok(drainMembers.every((lane) => lane.life === "drain"));
    assert.equal(driveMembers.length, 4, "the sibling team contains four members");
    assert.ok(driveMembers.every((lane) => lane.life === "drive"));
    assert.notEqual(drive.team, host.team);
    await reply(fixture, ["lane", "wake", drive.lane]);

    context = await browser.newContext({
      colorScheme: "dark",
      viewport: { width: 1000, height: 568 },
    });
    const page = await context.newPage();
    const monitor = monitorPage(page);
    await page.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
    await chooseCanvasView(page, "Chat");
    const console = page.locator('[data-pane="agent-console"]');
    await console.waitFor();
    const drainSurface = console.locator(
      `.team-surface[data-team="${host.team}"]`,
    );
    const driveSurface = console.locator(
      `.team-surface[data-team="${drive.team}"]`,
    );
    await drainSurface.getByRole("button", { name: "Drain", exact: true }).waitFor();
    await driveSurface.getByRole("button", { name: "Drive", exact: true }).waitFor();
    assert.equal(await console.locator(".team-surface").count(), 2);

    const drainTextbox = drainSurface.getByRole("textbox").first();
    await drainTextbox.fill("drain team accessibility");
    assert.equal(
      await drainSurface.getByRole("button", { name: "Drain", exact: true }).isDisabled(),
      true,
      "a vacant Drain team exposes its current non-submittable boundary",
    );
    const drainPlane = drainSurface.locator(".lane-mosaic-plane");
    const driveTextbox = driveSurface
      .locator(`.team-agent[data-agent-lane="${drive.lane}"]`)
      .getByRole("textbox");
    await driveTextbox.fill("drive team accessibility");
    await driveSurface.getByRole("button", { name: "Drive", exact: true }).click();
    const drivePlane = driveSurface.locator(".lane-mosaic-plane");
    const driveInbox = await reply(fixture, ["inbox", drive.lane]);
    assert.ok(
      driveInbox.body.lanes[0].pending.some(
        (directive) => directive.text === "drive team accessibility",
      ),
      "the occupied Drive sibling receives its submitted direction",
    );

    const planes = console.locator(".lane-mosaic-plane");
    assert.equal(await planes.count(), 2, "two admitted teams own exactly two packed planes");
    assert.deepEqual(
      [
        await drainPlane.getAttribute("data-team-policy"),
        await drainPlane.getAttribute("data-team-members"),
      ],
      ["Drain", "4"],
      "the vacant plane retains the Drain team's policy and member count",
    );
    assert.deepEqual(
      [
        await drivePlane.getAttribute("data-team-policy"),
        await drivePlane.getAttribute("data-team-members"),
      ],
      ["Drive", "4"],
      "the occupied plane retains the Drive team's policy and member count",
    );
    await assertTiledWorkspace(
      page,
      console,
      join(artifacts, "canvas-tiled-workspace-dark.png"),
    );

    await drainSurface.locator('.team-lifetime input[type="range"]').fill("0");
    await drainSurface.getByRole("button", { name: "Steer", exact: true }).waitFor();
    const changed = await reply(fixture, ["lane"]);
    assert.equal(
      changed.body.subscriptions.find(
        (subscription) => subscription.team === host.team,
      )?.policy,
      "steer",
    );
    assert.ok(
      changed.body.lanes
        .filter((lane) => lane.team === host.team)
        .every((lane) => lane.life === "drain"),
      "changing task allocation lifetime preserves the team's declared seat life",
    );
    assert.equal(
      await driveSurface.getByRole("button", { name: "Drive", exact: true }).count(),
      1,
      "changing the Drain team leaves its Drive sibling unchanged",
    );
    await drainTextbox.fill("isolated team submission");
    assert.equal(
      await drainSurface.getByRole("button", { name: "Steer", exact: true }).isDisabled(),
      true,
      "changing task policy does not manufacture residency for a vacant Drain seat",
    );
    assert.equal(
      await drivePlane.getAttribute("data-team-policy"),
      "Drive",
      "changing one team does not mutate its sibling policy",
    );
    monitor.assertClean();
  } finally {
    if (context) await context.close();
    // Draining the seats first is the graceful path and stays first, but it
    // talks to the resident over eight lanes and can itself fail -- and when it
    // did, every step after it was skipped, so a scenario that failed early
    // left the resident, its stand-ins, and both directories behind. The owner
    // below runs whatever this could not.
    await cleanupTeamFixture({ fixture, resident, owner });
  }
}
