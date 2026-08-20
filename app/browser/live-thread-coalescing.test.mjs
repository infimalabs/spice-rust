import { mkdtemp, realpath } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import test from "node:test";

import {
  buildResident,
  configureFixture,
  seedAuthority,
  startResident,
} from "./canvas-fixture.mjs";
import { ownFixture } from "./canvas-lifecycle.mjs";
import { launchBrowser } from "./headless-launch.mjs";
import { assertLiveThreadCoalescing } from "./live-thread-coalescing.mjs";

test("an established socket coalesces a live fact burst into one newest thread", {
  timeout: 90_000,
}, async () => {
  if (process.env.SPICE_CANVAS_PREBUILT !== "1") await buildResident();
  const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-live-thread-")));
  const linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-live-thread-lanes-")));
  const owner = ownFixture({ fixture, linkedRoot });
  let browser;
  try {
    await configureFixture(fixture, linkedRoot, 1);
    const resident = owner.ownResident(await startResident(fixture));
    const seeded = await seedAuthority(fixture);
    browser = await launchBrowser();
    await assertLiveThreadCoalescing(
      browser,
      resident.discovery.endpoint,
      fixture,
      seeded.task,
    );
  } finally {
    if (browser) await browser.close();
    await owner.cleanup();
  }
});
