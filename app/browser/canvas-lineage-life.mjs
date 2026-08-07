// Lineage-life isolation scenario for the Canvas browser suite.
import assert from "node:assert/strict";
import { mkdtemp, realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

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

export async function assertLineageLifeIsolation(browser) {
  const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-lineage-life-")));
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
    assert.ok(host, "the four-member Drain lineage has a host");
    for (const worktree of worktrees.slice(1, 4)) {
      await reply(fixture, [
        "lane", "add", worktree,
        "--lineage", host.lineage_id,
        "--life", "Drain",
        "--vendor", "claude",
      ]);
    }
    await reply(fixture, [
      "lane", "add", worktrees[4], "--life", "Drive", "--vendor", "claude",
    ]);
    const secondSnapshot = await reply(fixture, ["lane"]);
    const drive = secondSnapshot.body.lanes.find((lane) => lane.worktree === worktrees[4]);
    assert.ok(drive, "the four-member Drive lineage has a host");
    for (const worktree of worktrees.slice(5)) {
      await reply(fixture, [
        "lane", "add", worktree,
        "--lineage", drive.lineage_id,
        "--life", "Drive",
        "--vendor", "claude",
      ]);
    }
    const snapshot = await reply(fixture, ["lane"]);
    const drainMembers = snapshot.body.lanes.filter(
      (lane) => lane.lineage_id === host.lineage_id,
    );
    const driveMembers = snapshot.body.lanes.filter(
      (lane) => lane.lineage_id === drive.lineage_id,
    );
    assert.equal(drainMembers.length, 4, "one admitted lineage contains four members");
    assert.ok(drainMembers.every((lane) => lane.life === "drain"));
    assert.equal(driveMembers.length, 4, "the sibling lineage contains four members");
    assert.ok(driveMembers.every((lane) => lane.life === "drive"));
    assert.notEqual(drive.lineage_id, host.lineage_id);

    context = await browser.newContext({
      colorScheme: "dark",
      viewport: { width: 1000, height: 568 },
    });
    const page = await context.newPage();
    const monitor = monitorPage(page);
    await page.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
    await chooseCanvasView(page, "Agents");
    const console = page.locator('[data-pane="agent-console"]');
    await console.waitFor();
    const drainSurface = console.locator(
      `.team-surface[data-lineage-id="${host.lineage_id}"]`,
    );
    const driveSurface = console.locator(
      `.team-surface[data-lineage-id="${drive.lineage_id}"]`,
    );
    await drainSurface.getByRole("button", { name: "Drain", exact: true }).waitFor();
    await driveSurface.getByRole("button", { name: "Drive", exact: true }).waitFor();
    assert.equal(await console.locator(".team-surface").count(), 2);

    await drainSurface.getByRole("textbox").first().fill("drain lineage accessibility");
    await drainSurface.getByRole("button", { name: "Drain", exact: true }).click();
    const drainPlane = drainSurface.locator(".lane-mosaic-plane");
    await drainPlane
      .locator(".mosaic-message-card")
      .filter({ hasText: "drain lineage accessibility" })
      .waitFor({ timeout: 10_000 });
    await driveSurface.getByRole("textbox").first().fill("drive lineage accessibility");
    await driveSurface.getByRole("button", { name: "Drive", exact: true }).click();
    const drivePlane = driveSurface.locator(".lane-mosaic-plane");
    await drivePlane
      .locator(".mosaic-message-card")
      .filter({ hasText: "drive lineage accessibility" })
      .waitFor({ timeout: 10_000 });

    const planes = console.locator(".lane-mosaic-plane");
    assert.equal(await planes.count(), 2, "two admitted lineages own exactly two packed planes");
    assert.match(await drainPlane.ariaSnapshot(), /Drain policy, 4 members/);
    assert.match(await drivePlane.ariaSnapshot(), /Drive policy, 4 members/);
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
        (subscription) => subscription.lineage_id === host.lineage_id,
      )?.policy,
      "steer",
    );
    assert.ok(
      changed.body.lanes
        .filter((lane) => lane.lineage_id === host.lineage_id)
        .every((lane) => lane.life === "drain"),
      "changing task subscription preserves the lineage's terminal seat life",
    );
    assert.equal(
      await driveSurface.getByRole("button", { name: "Drive", exact: true }).count(),
      1,
      "changing the Drain lineage leaves its Drive sibling unchanged",
    );
    await drainSurface.getByRole("textbox").first().fill("isolated lineage submission");
    await drainSurface.getByRole("button", { name: "Steer", exact: true }).click();
    await drainPlane
      .locator(".mosaic-message-card")
      .filter({ hasText: "isolated lineage submission" })
      .waitFor({ timeout: 10_000 });
    assert.equal(
      await drainSurface.getByRole("button", { name: "Steer", exact: true }).count(),
      1,
      "submitting the changed lineage retains its own policy",
    );
    assert.equal(
      await drivePlane.getAttribute("data-lineage-policy"),
      "Drive",
      "submitting one lineage does not mutate its sibling policy",
    );
    monitor.assertClean();
  } finally {
    if (context) await context.close();
    // Draining the seats first is the graceful path and stays first, but it
    // talks to the resident over eight lanes and can itself fail -- and when it
    // did, every step after it was skipped, so a scenario that failed early
    // left the resident, its stand-ins, and both directories behind. The owner
    // below runs whatever this could not.
    try {
      if (resident?.child.exitCode === null) {
        const liveTeam = await reply(fixture, ["lane"]);
        for (const agent of liveTeam.body.lanes) {
          await reply(fixture, ["lane", "drain", agent.lane]);
        }
      }
    } catch {
      // The resident is unreachable; the owner escalates rather than reporting
      // a teardown error over whatever the scenario itself was failing with.
    }
    await owner.cleanup();
  }
}
