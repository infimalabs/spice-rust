import assert from "node:assert/strict";
import { execFile, spawn } from "node:child_process";
import { once } from "node:events";
import { unwatchFile, watchFile } from "node:fs";
import { chmod, cp, mkdtemp, mkdir, readFile, readdir, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import { chromium } from "playwright";

const execute = promisify(execFile);
const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const binary = join(root, "target/debug/spice");
const artifacts = join(root, "artifacts/canvas");
const sessionRecord = JSON.stringify({
  type: "system",
  subtype: "init",
  session_id: "018f7777-7777-7444-8888-999999999999",
});
const staleEpoch = "018f9999-9999-7444-8888-999999999999";

const axisPanes = new Map([
  ["Magnitude", ["Effort", "Pressure", "Residue", "Consent Volume"]],
  ["Flow", ["Tasks", "Claims", "Deferrals", "Reviews"]],
  ["Topology", ["Lanes", "Worktrees", "Lineage", "Occupancy", "Residents"]],
  ["Chronology", [
    "Compaction",
    "Queues",
    "Directives",
    "Consent Disposition",
    "Trust",
    "Commands",
    "Recovery",
  ]],
]);

async function buildResident() {
  try {
    await execute("cargo", ["build", "-p", "spice", "--bin", "spice"], {
      cwd: root,
      maxBuffer: 16 * 1024 * 1024,
    });
  } catch (error) {
    const stdout = error?.stdout ?? "<none>";
    const stderr = error?.stderr ?? "<none>";
    throw new Error(
      `canvas spec could not build its resident: stdout=${stdout} stderr=${stderr}`,
      { cause: error },
    );
  }
}

function waitForDiscovery(child, canvas, timeout = 30_000) {
  return new Promise((resolveDiscovery, rejectDiscovery) => {
    let settled = false;
    let lastError;
    const published = (current, previous) => {
      if (current.ino !== previous.ino || current.size !== previous.size) readPublishedDiscovery();
    };
    watchFile(canvas, { interval: 50 }, published);
    const deadline = setTimeout(() => {
      finish(new Error("timed out waiting for resident canvas discovery", { cause: lastError }));
    }, timeout);
    const residentExit = (code) => {
      finish(new Error(`resident exited ${code} before publishing canvas discovery`));
    };

    function finish(error, discovery) {
      if (settled) return;
      settled = true;
      clearTimeout(deadline);
      unwatchFile(canvas, published);
      child.off("exit", residentExit);
      if (error) rejectDiscovery(error);
      else resolveDiscovery(discovery);
    }

    function readPublishedDiscovery() {
      readFile(canvas, "utf8").then(
        (bytes) => {
          try {
            finish(null, JSON.parse(bytes));
          } catch (error) {
            finish(new Error("resident canvas discovery is malformed", { cause: error }));
          }
        },
        (error) => {
          lastError = error;
          if (error.code !== "ENOENT") finish(error);
        },
      );
    }

    child.once("exit", residentExit);
    readPublishedDiscovery();
  });
}

async function startResident(fixture) {
  const canvas = join(fixture, ".git/.spice/canvas");
  await mkdir(dirname(canvas), { recursive: true });
  // Port 0 asks the kernel for a free one, as every Rust integration test that
  // spawns a resident already does. The default is a fixed port so an operator
  // has a stable URL, but this fixture is one of several residents alive at
  // once -- the lane's own appliance, the other lanes', and a second copy of
  // this spec -- and it reads the port back out of the published discovery
  // record, so it never needed the fixed one.
  const child = spawn(binary, ["--json", "up", "--port", "0"], {
    cwd: fixture,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stdout = "";
  let stderr = "";
  child.stdout.on("data", (chunk) => { stdout += chunk.toString(); });
  child.stderr.on("data", (chunk) => { stderr += chunk.toString(); });
  child.on("exit", (code) => {
    if (code && code !== 0) process.stderr.write(`resident exited ${code}: ${stderr}\n`);
  });
  try {
    const discovery = await waitForDiscovery(child, canvas);
    return { child, discovery };
  } catch (error) {
    let residue;
    try {
      residue = await readdir(fixture, { recursive: true });
    } catch (readError) {
      residue = [`<unreadable: ${readError}>`];
    }
    if (child.exitCode === null) {
      const exited = once(child, "exit");
      child.kill("SIGKILL");
      await exited;
    }
    throw new Error(`resident did not publish discovery: stdout=${stdout || "<none>"} stderr=${stderr || "<none>"} files=${JSON.stringify(residue)}`, {
      cause: error,
    });
  }
}

function decodeOutcome(stdout, stderr = "") {
  const text = stdout.trim() || stderr.trim();
  assert.ok(text, "spice command returns one JSON outcome");
  return JSON.parse(text);
}

async function invoke(fixture, args) {
  try {
    const result = await execute(binary, ["--json", ...args], {
      cwd: fixture,
      timeout: 20_000,
    });
    return decodeOutcome(result.stdout, result.stderr);
  } catch (error) {
    if (typeof error?.stdout === "string" || typeof error?.stderr === "string") {
      return decodeOutcome(error.stdout ?? "", error.stderr ?? "");
    }
    throw error;
  }
}

async function reply(fixture, args) {
  const outcome = await invoke(fixture, args);
  assert.equal(outcome.kind, "reply", `${args.join(" ")} replies: ${JSON.stringify(outcome)}`);
  return outcome;
}

/// Refuse a run whose stage carries no app to serve.
///
/// There is one stage now and the resident has no embedded bundle to fall back
/// on, so a lane that has not run `cargo xtask app` -- or whose stage was
/// half written -- gets a resident that starts, answers, and serves nothing the
/// browser can execute. That failure does not announce itself: it surfaces as a
/// timeout in whichever feature the lane just changed, ten seconds of waiting
/// for an element no served code renders, and it reads exactly like a
/// regression in that change. One directory read before the copy turns a
/// debugging session into a refusal naming the command that fixes it.
async function configureFixture(fixture) {
  const appStage = "dist/stage-app";
  // The stage is a generation store now: one atomic pointer names a complete
  // immutable generation, so the wasm lives under that generation rather than
  // beside the pointer. The precondition reads the same pointer the resident
  // reads, so it refuses on exactly what the resident would refuse on.
  const stage = join(root, appStage);
  const pointer = await readFile(join(stage, "current"), "utf8").catch(() => "");
  const generation = pointer.trim();
  const staged = generation
    ? await readdir(join(stage, "generations", generation)).catch(() => [])
    : [];
  assert.ok(
    staged.includes("app_bg.wasm"),
    `${stage} publishes no complete app generation: run cargo xtask app to build the stage this spec serves`,
  );
  await cp(stage, join(fixture, appStage), { recursive: true });
  const vendor = join(fixture, "fake-claude");
  await writeFile(vendor, [
    "#!/bin/sh",
    "trap 'exit 0' TERM INT",
    "name=${PWD##*/}",
    `printf '%s\\n' '${sessionRecord}'`,
    concatAgentRecord(),
    "while :; do sleep 1; done",
    "",
  ].join("\n"));
  await chmod(vendor, 0o700);
  await writeFile(join(fixture, "spice.toml"), [
    "[consent]",
    'posture = "queue"',
    "",
    "[[consent.rule]]",
    'name = "browser"',
    'tool = "shell:*"',
    'answer = "allow"',
    "budget = 2",
    "",
    "[lane]",
    'lease = "30s"',
    'vendor = "claude"',
    "",
    "[tasks]",
    'stems = ["app", "canvas"]',
    "",
    "[vendor.claude]",
    `binary = ${JSON.stringify(vendor)}`,
    "",
    "[canvas.pack]",
    "rows = 2",
    "",
  ].join("\n"));
}

function concatAgentRecord() {
  const prefix = "{\"type\":\"assistant\",\"message\":{\"type\":\"message\","
    + "\"role\":\"assistant\",\"content\":[{\"type\":\"text\","
    + "\"text\":\"Canvas message from ";
  const suffix = "\"}],\"usage\":{\"input_tokens\":1,\"cache_read_input_tokens\":0,"
    + "\"cache_creation_input_tokens\":0,\"output_tokens\":1}}}";
  return `printf '%s%s%s\\n' '${prefix}' "$name" '${suffix}'`;
}

async function journalExtent(fixture) {
  const report = await reply(fixture, ["journal"]);
  assert.ok(report.body.cursor, "running authority has a journal cursor");
  return {
    cursor: Number(report.body.cursor),
    facts: Number(report.body.facts),
  };
}

async function seedAuthority(fixture) {
  const pending = await reply(fixture, ["trust"]);
  const digest = pending.body.digest;
  assert.equal(pending.body.standing, "pended");
  await reply(fixture, ["trust", "grant", digest]);

  const first = await reply(fixture, [
    "task",
    "add",
    "Browser fixture task",
    "--project",
    "app.browser",
  ]);
  await reply(fixture, [
    "task",
    "add",
    "Continuation witness",
    "--project",
    "canvas.browser",
  ]);
  await reply(fixture, ["task", "note", first.body.task, "first causal observation"]);
  await reply(fixture, ["task", "note", first.body.task, "second causal observation"]);

  return {
    task: first.body.task,
    extent: await journalExtent(fixture),
  };
}

function monitorPage(page) {
  const pageErrors = [];
  const requestFailures = [];
  let expectedNetworkInterruption = false;
  page.on("pageerror", (error) => pageErrors.push(error.stack ?? error.toString()));
  page.on("console", (message) => {
    if (message.type() === "error") pageErrors.push(`console: ${message.text()}`);
  });
  page.on("requestfailed", (request) => {
    if (!expectedNetworkInterruption) {
      requestFailures.push(`${request.url()} ${request.failure()?.errorText ?? "unknown failure"}`);
    }
  });
  return {
    expectNetworkInterruption(value) {
      expectedNetworkInterruption = value;
    },
    assertClean() {
      assert.deepEqual(pageErrors, [], "page emits no uncaught errors");
      assert.deepEqual(requestFailures, [], "page emits no unexpected request failures");
    },
  };
}

async function stopResident(fixture, resident) {
  const exited = resident.child.exitCode === null
    ? once(resident.child, "exit")
    : Promise.resolve();
  await execute(binary, ["--json", "down"], { cwd: fixture, timeout: 10_000 });
  await exited;
}

async function assertCanvas(page, scheme) {
  try {
    await page.waitForSelector(".status.online", { timeout: 10_000 });
  } catch (error) {
    const body = await page.locator("body").innerText().catch(() => "<body unavailable>");
    throw new Error(`canvas did not come online; body=${JSON.stringify(body)}`, { cause: error });
  }
  await page.waitForFunction(() => document.querySelector("#app")?.childElementCount > 0);
  await assert.doesNotReject(() => page.locator(".brand-name:visible, .mobile-brand:visible").waitFor());
  await assert.doesNotReject(() => page.getByRole("heading", { name: "Agents", exact: true }).waitFor());
  await page.waitForFunction(() => {
    const cursor = document.querySelector(".bar-meta")?.textContent ?? "";
    return /cursor \d+/.test(cursor);
  });

  const pixels = await page.evaluate(() => {
    const root = document.querySelector("#app");
    const canvas = document.querySelector(".content");
    const rootStyle = root ? getComputedStyle(root) : null;
    const canvasBox = canvas?.getBoundingClientRect();
    const workspaceBox = document.querySelector(".workspace")?.getBoundingClientRect();
    const barBox = document.querySelector(".commandbar")?.getBoundingClientRect();
    return {
      appChildren: root?.childElementCount ?? 0,
      bodyWidth: document.body.scrollWidth,
      viewportWidth: window.innerWidth,
      canvasHeight: Math.round(canvasBox?.height ?? 0),
      ink: rootStyle?.color ?? "",
      background: getComputedStyle(document.body).backgroundColor,
      barOverlap: (barBox?.bottom ?? 0) > (workspaceBox?.top ?? 0) + 1,
      overflowingControls: [...document.querySelectorAll("button, .pane-chip")]
        .filter((element) => {
          const style = getComputedStyle(element);
          return style.display !== "none" && style.visibility !== "hidden";
        })
        .filter((element) => element.scrollWidth > element.clientWidth + 1)
        .map((element) => element.textContent?.trim() ?? element.className),
    };
  });
  assert.ok(pixels.appChildren > 0, "Wasm mounted a nonblank app");
  assert.ok(pixels.canvasHeight > 300, "primary canvas has visible content");
  assert.ok(pixels.bodyWidth <= pixels.viewportWidth, "page has no viewport overflow");
  assert.notEqual(pixels.ink, pixels.background, "foreground and background pixels differ");
  assert.equal(pixels.barOverlap, false, "the workspace starts below the command bar");
  assert.deepEqual(pixels.overflowingControls, [], "controls and pane labels fit their bounds");

  for (const [axis, panes] of axisPanes) {
    await page.getByRole("button", { name: axis }).click();
    await page.getByRole("heading", { name: axis, exact: true }).waitFor();
    assert.deepEqual(
      await page.locator(".pane-chip").allTextContents(),
      panes,
      `${axis} renders its complete declared pane registry`,
    );
  }
  await page.keyboard.press("Tab");
  assert.ok(await page.locator(":focus-visible").count(), "keyboard focus remains visible");
  if ((page.viewportSize()?.width ?? 1000) <= 560) {
    await page.getByRole("button", { name: "Flow" }).click();
    await page.locator(".workspace").evaluate((element) => element.scrollTo(0, element.scrollHeight));
    assert.ok(await page.getByRole("button", { name: "Add note" }).isVisible());
    await page.locator(".workspace").evaluate((element) => element.scrollTo(0, 0));
  }
  await page.getByRole("button", { name: "Mosaic" }).click();
  await page.getByRole("heading", { name: "Agents", exact: true }).waitFor();
  await page.screenshot({ path: join(artifacts, `canvas-${scheme}.png`), fullPage: true });
}

/// The console's one compose box, addressed so a feed full of cards cannot
/// shadow it. Every card action is labelled "... this message", so the box has
/// to be picked by role and by an exact name rather than by a label substring.
function composeField(lane) {
  return lane.locator(".mosaic-composer").getByRole("textbox");
}

async function assertAgentConversation(page, fixture, memberWorktree) {
  const console = page.locator('[data-pane="agent-console"]');
  await page.getByRole("button", { name: "Mosaic" }).click();
  await console.getByRole("heading", { name: "Agents", exact: true }).waitFor();
  await console.getByLabel("Worktree").fill(fixture);
  await console.getByLabel("Life").fill("steer");

  // A refused submission has to say why. The vendor field is free text with no
  // datalist, so a proper name typed the way it is written everywhere else is
  // the trip an operator actually takes, and it used to send no request and
  // print nothing: the button read as dead. This is the only refusal reachable
  // by clicking, because the compose button disables itself while the message
  // is empty or no agent is seated.
  const refusal = console.locator(".form-refusal");
  await console.getByLabel("Vendor").fill("My Agent");
  await console.getByRole("button", { name: "Seat agent" }).click();
  await refusal.waitFor({ timeout: 10_000 });
  assert.match(await refusal.innerText(), /Vendor must be one lower-case word/);

  await console.getByLabel("Vendor").fill("Claude");
  await console.getByRole("button", { name: "Seat agent" }).click();
  await refusal.waitFor({ state: "detached", timeout: 10_000 });

  const tabs = console.locator(".agent-tab");
  try {
    await tabs.first().waitFor({ timeout: 15_000 });
  } catch (error) {
    await page.getByRole("button", { name: "Chronology" }).click();
    const rendered = await page.locator("body").innerText();
    const lanes = await invoke(fixture, ["lane"]);
    throw new Error(`Canvas seat did not create a lane: lanes=${JSON.stringify(lanes)} body=${JSON.stringify(rendered)}`, {
      cause: error,
    });
  }
  const lanes = await reply(fixture, ["lane"]);
  const seated = lanes.body.lanes.find((lane) => lane.worktree === fixture);
  assert.ok(seated, "the Canvas seat control creates a lane in the requested worktree");
  assert.equal(seated.vendor, "claude");
  assert.equal(seated.life, "steer");
  assert.equal(
    seated.lineage_id,
    seated.lane,
    "a new seat starts a one-member lineage",
  );
  const added = await reply(fixture, [
    "lane", "add", memberWorktree, "--lineage", seated.lineage_id,
  ]);
  const memberLane = added.body.lane;
  const plural = await reply(fixture, ["lane"]);
  const member = plural.body.lanes.find((lane) => lane.lane === memberLane);
  assert.ok(member, "the second linked worktree is seated");
  assert.equal(
    member.lineage_id,
    seated.lineage_id,
    "the second seat joins the first Canvas lane",
  );
  try {
    await page.waitForFunction(
      () => document.querySelectorAll('[data-pane="agent-console"] .agent-tab').length === 2,
      null,
      { timeout: 15_000 },
    );
  } catch (error) {
    throw new Error(
      `Canvas did not render the plural lineage: ${JSON.stringify({
        connection: await page.locator(".status-copy").innerText().catch(() => "unavailable"),
        tabs: await tabs.count(),
        chronology: await page.locator("body").innerText().catch(() => "unavailable"),
        lanes: plural.body.lanes,
      })}`,
      { cause: error },
    );
  }
  const tab = console.locator(`.agent-tab[data-lane="${seated.lane}"]`);
  assert.equal(await tab.getAttribute("data-lane"), seated.lane);

  const lineage = page.locator(`[data-lane-id="${seated.lineage_id}"]`);
  await lineage.waitFor({ timeout: 10_000 });
  const message = "browser direction survives streaming";
  await composeField(lineage).fill(message);
  await lineage.getByRole("button", { name: "Send", exact: true }).click();
  const outgoing = lineage.locator(".operator-direction").filter({ hasText: message });
  await outgoing.waitFor({ timeout: 10_000 });
  // A card is only drawn for a direction still in the pending region, and it
  // marks that with the time left before the direction lapses; once a lane takes
  // the direction the card is gone rather than relabelled. So a countdown here is
  // the browser-side proof it is still waiting. This replaces a literal "pending"
  // label the mosaic card no longer prints -- matching that word again would pass
  // on any card that merely mentioned it. The server-side pending state is
  // asserted directly below, so the two together pin both ends.
  assert.match(
    await outgoing.locator(".message-age").innerText(),
    /^(\d+[smhd]|now) left$/,
  );

  const inbox = await reply(fixture, ["inbox", seated.lane]);
  assert.equal(inbox.body.lanes.length, 1);
  assert.equal(inbox.body.lanes[0].pending.length, 1);
  assert.equal(inbox.body.lanes[0].pending[0].text, message);
  // The card's key is the server's key for that queued direction, which is what
  // makes the card a view of the inbox rather than an echo of what was typed.
  assert.equal(
    await outgoing.locator(".message-key").innerText(),
    inbox.body.lanes[0].pending[0].key,
  );
  // The strip is read while exactly one direction is queued, before the packing
  // check floods the feed: its whole claim is that the card carries the pending
  // count and the latest text, and both are only checkable against a known one.
  await assertLaneStrip(page, fixture, seated, message);
  await assertLineageLane(page, lineage, fixture, memberWorktree, [seated.lane, memberLane]);
  await assertMosaicPacking(page, lineage);
  await page.screenshot({ path: join(artifacts, "canvas-light-mosaic-agent.png"), fullPage: true });
  return {
    lane: seated.lane,
    lineage: seated.lineage_id,
    memberLane,
  };
}

async function assertLineageLane(page, lineage, fixture, memberWorktree, memberLanes) {
  assert.equal(
    await page.locator(".mosaic-lane").count(),
    1,
    "two member seats share exactly one Canvas lane",
  );
  assert.equal(
    await lineage.locator(".mosaic-feed").count(),
    1,
    "the lineage owns exactly one packed message plane",
  );

  const leaves = [fixture, memberWorktree]
    .map((worktree) => worktree.split("/").filter(Boolean).pop());
  const lights = lineage.locator(".lane-light");
  await lights.nth(1).waitFor({ timeout: 15_000 });
  assert.deepEqual(
    (await lights.evaluateAll((nodes) => nodes.map(
      (node) => node.dataset.laneLightTargetId,
    ))).sort(),
    [...leaves].sort(),
    "one status pip names each assigned worktree",
  );
  assert.ok(
    (await lights.evaluateAll((nodes) => nodes.map((node) => node.dataset.agentStatus)))
      .every((status) => ["queued", "running", "bound", "unbound"].includes(status)),
    "each member pip exposes one stable status",
  );

  const authored = lineage.locator(".mosaic-card.agent-authored");
  await authored.nth(1).waitFor({ timeout: 15_000 });
  const producers = await authored.evaluateAll((nodes) => nodes.map(
    (node) => node.dataset.producerTargetId,
  ));
  assert.deepEqual(
    [...new Set(producers)].sort(),
    [...leaves].sort(),
    "one packed plane retains both producing worktrees",
  );
  assert.deepEqual(
    [...new Set(await authored.locator(".message-author").allTextContents())].sort(),
    [...leaves].sort(),
    "card footers name both producing agents",
  );
  const expectedAccents = new Map(leaves.map((leaf, index) => [
    leaf,
    Number(BigInt(`0x${memberLanes[index].replaceAll("-", "")}`) % 6n).toString(),
  ]));
  for (const card of await authored.all()) {
    const producer = await card.getAttribute("data-producer-target-id");
    assert.equal(
      await card.getAttribute("data-accent-slot"),
      expectedAccents.get(producer),
      "each authored card keeps its producer's accent slot",
    );
  }
  const accessibility = await lineage.ariaSnapshot();
  for (const leaf of leaves) {
    assert.match(accessibility, new RegExp(leaf), `the accessibility tree names ${leaf}`);
  }
}

/// The strip above the console: one card per seated lane, carrying its state.
///
/// The card is what the operator reads before opening a conversation, so it is
/// checked for what it joins rather than for its existence: the lane's name,
/// the direction that is actually queued, the count of it, and the vendor. The
/// name is the worktree's last segment because the full path is the same for
/// every card up to it; the whole path is on the label a screen reader reads,
/// and on the conversation heading the card selects.
async function assertLaneStrip(page, fixture, seated, message) {
  const tiles = page.locator(".agent-tabs.watchband .watchband-card");
  const tile = page.locator(
    `.agent-tabs.watchband .watchband-card[data-lane="${seated.lane}"]`,
  );
  await tile.waitFor({ timeout: 10_000 });
  assert.equal(await tiles.count(), 2, "the strip carries one card per lineage member");

  // The card has room for one field of the path, so it carries the segment that
  // tells this lane from its neighbours, cut to that field. The cut is asserted
  // in both directions: a card that stopped truncating would still start with
  // the leaf, and one that truncated a name that already fit would still fit.
  const leaf = fixture.split("/").filter(Boolean).pop();
  const name = await tile.locator(".watchband-name").innerText();
  assert.ok(name.length <= 22, `the name fits the card's field: ${name}`);
  assert.ok(leaf.startsWith(name.replace(/…$/, "")), `the name is the worktree leaf: ${name}`);
  assert.equal(name.endsWith("…"), leaf.length > 22, "the ellipsis marks a cut and only a cut");

  // The queued direction itself is the hover card, flattened to one line; the
  // count and the disposition are the line under the name. Between them the
  // card answers "how many, and what is newest" without being opened. The claim
  // line is checked too because it is the join against a third source -- the
  // board -- and an empty board has to read as "no claim" rather than as blank.
  // `occupied` is the resident's own word for the seat -- the vocabulary is
  // vacant/occupied/degraded -- so the card is showing the lane table rather
  // than a label the view made up, and a lane that lost its occupant would read
  // differently here without the count changing at all.
  assert.equal(await tile.getAttribute("title"), message);
  assert.match(
    await tile.locator(".watchband-line.muted").first().innerText(),
    /^1 pending, occupied/,
  );
  assert.equal(await tile.locator(".watchband-line.mono").innerText(), "no claim");
  assert.equal(await tile.locator(".tag.vendor").innerText(), "claude");
  assert.equal(
    await tile.getAttribute("aria-label"),
    `Open claude lane ${seated.lane.slice(0, 8)} in ${fixture}`,
    "the label a screen reader reads carries the whole path the card cut",
  );

  // Selecting the card aims the one compose form at that lane, which is the
  // whole point of a strip that shares a single input. The heading is what says
  // which lane the form now writes to, and the form is live because that lane
  // is occupied -- a vacant one disables it.
  await tile.click();
  assert.equal(await tile.getAttribute("aria-selected"), "true");
  const compose = page.locator(`[data-lane-id="${seated.lineage_id}"] .mosaic-composer textarea`);
  assert.equal(await compose.isDisabled(), false, "the occupied lineage host accepts direction");
  await page.screenshot({ path: join(artifacts, "canvas-light-mosaic-strip.png"), fullPage: true });
}

async function assertMosaicPacking(page, lane) {
  const feed = lane.locator(".mosaic-feed");
  const laneCount = Number(await feed.getAttribute("data-lanes"));
  assert.ok(laneCount >= 1 && laneCount <= 6, "geometry exposes a legal base-lane count");
  const cards = lane.locator(".mosaic-card");
  const existingCards = await cards.count();
  const addedCards = laneCount * 2;
  for (let index = 0; index < addedCards; index += 1) {
    await composeField(lane).fill(`mosaic placement ${index}`);
    await lane.getByRole("button", { name: "Send", exact: true }).click();
  }
  const targetCards = existingCards + addedCards;
  await cards.nth(targetCards - 1).waitFor({ timeout: 15_000 });
  assert.equal(await cards.count(), targetCards);
  assert.equal(await cards.first().getAttribute("data-state"), "frozen");
  const directions = lane.locator(".mosaic-card.operator-direction");
  assert.ok(
    (await directions.evaluateAll((elements) => elements.map(
      (element) => element.dataset.producerTargetId,
    ))).every((producer) => producer === undefined),
    "operator directions never masquerade as agent-produced cards",
  );
  const keys = await directions.locator(".message-key").allTextContents();
  assert.deepEqual(keys, [...keys].sort(), "Canvas journal order agrees with steering key order");
  assert.deepEqual(
    await directions.evaluateAll((elements) => elements.map((element) => element.dataset.key)),
    keys,
    "each packed card binds its visible key to its machine-readable identity",
  );
  const ages = directions.locator(".message-age");
  assert.ok(
    (await ages.allTextContents()).every((age) => age.endsWith(" left")),
    "spatially packed keys carry the age that orders them",
  );
  assert.equal(await ages.count(), addedCards + 1);

  const placements = await cards.evaluateAll((elements) => elements.map((element) => {
    const box = element.getBoundingClientRect();
    return {
      track: Number(element.dataset.track),
      span: Number(element.dataset.span),
      base: Number(element.dataset.base),
      row: Number(element.dataset.row),
      rows: Number(element.dataset.rows),
      gridColumnStart: Number(getComputedStyle(element).gridColumnStart),
      gridRowStart: Number(getComputedStyle(element).gridRowStart),
      left: box.left,
      right: box.right,
      top: box.top,
      bottom: box.bottom,
    };
  }));
  assert.ok(placements.every((card) => card.span >= 2 && card.track + card.span <= 12));
  assert.ok(placements.every((card) => card.rows >= 2));
  assert.ok(placements.every((card) => card.gridColumnStart === card.track + 1));
  assert.ok(placements.every((card) => card.gridRowStart === card.row));
  for (let left = 0; left < placements.length; left += 1) {
    for (let right = left + 1; right < placements.length; right += 1) {
      const a = placements[left];
      const b = placements[right];
      const separated = a.right <= b.left + 1 || b.right <= a.left + 1
        || a.bottom <= b.top + 1 || b.bottom <= a.top + 1;
      assert.ok(separated, `mosaic cards ${left} and ${right} do not overlap`);
    }
  }

  await feed.evaluate((element) => element.scrollTo(0, element.scrollHeight));
  const before = await feed.evaluate((element) => element.scrollTop);
  await composeField(lane).fill("mosaic scroll compensation");
  await lane.getByRole("button", { name: "Send", exact: true }).click();
  await cards.nth(targetCards).waitFor({ timeout: 10_000 });
  const after = await feed.evaluate((element) => element.scrollTop);
  assert.ok(after >= before, "incremental placement preserves a scrolled viewport");
  await page.setViewportSize({ width: 1200, height: 900 });
  await page.setViewportSize({ width: 1440, height: 900 });
  await cards.nth(targetCards).waitFor();
}

async function assertAuthoritySurface(page, seeded) {
  await page.getByRole("button", { name: "Flow" }).click();
  const currentWork = page.locator(".section").filter({ hasText: "Current work" });
  await currentWork.getByText("Browser fixture task", { exact: true }).waitFor();
  await currentWork.getByText("Continuation witness", { exact: true }).waitFor();

  await page.getByRole("button", { name: "Magnitude" }).click();
  await page.getByRole("heading", { name: "Consent budgets", exact: true }).waitFor();
  await page.getByText("browser", { exact: true }).waitFor();
  assert.match(await page.locator(".section").filter({ hasText: "Consent budgets" }).innerText(), /2\s+2/);

  await page.getByRole("button", { name: "Topology" }).click();
  const topology = page.locator("main.content");
  await topology.getByText("Resident topology", { exact: true }).waitFor();
  await topology.getByText("claude", { exact: true }).first().waitFor();
  assert.match(await topology.innerText(), new RegExp(seeded.lane.slice(0, 8)));

  await page.getByRole("button", { name: "Chronology" }).click();
  await page.getByText("browser direction survives streaming", { exact: true }).waitFor();
  const trust = page.locator('[data-pane="trust"]');
  await trust.waitFor();
  const trustText = await trust.innerText();
  assert.match(trustText, /trust::pended/);
  assert.match(trustText, /trust::granted/);
  assert.match(trustText, /granted/);
}

async function assertMosaicComposer(page, fixture, seeded) {
  // Anchored rather than exact: the pill carries a badge with the count of
  // directions waiting across every lane, so its accessible name is "Mosaic 5"
  // by the time anything has been sent. Anchoring still keeps it off any later
  // control that merely ends in the word. The heading below is the exact one --
  // it is the pane rather than the control, so it is what proves the click
  // landed.
  await page.getByRole("button", { name: /^Mosaic/ }).click();
  await page.getByRole("heading", { name: "Mosaic", exact: true }).waitFor();
  const lane = page.locator(`[data-lane-id="${seeded.lineage}"]`);
  await lane.getByText("browser direction survives streaming", { exact: true }).waitFor();

  await lane.locator(".mosaic-card")
    .filter({ hasText: "browser direction survives streaming" })
    .getByRole("button", { name: "Quote this message" })
    .click();
  const composer = lane.locator(".mosaic-composer");
  await composer.getByLabel("Markdown preview").waitFor();
  assert.equal(
    await composer.locator("blockquote").innerText(),
    "browser direction survives streaming",
  );
  await composer.getByRole("textbox").fill("canvas quote reply");
  const pixel = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
    "base64",
  );
  await composer.locator('input[type="file"]').setInputFiles([
    { name: "pixel.png", mimeType: "image/png", buffer: pixel },
    { name: "pixel-copy.png", mimeType: "image/png", buffer: pixel },
  ]);
  await composer.getByRole("img", { name: "pixel.png" }).waitFor();
  await composer.getByRole("img", { name: "pixel-copy.png" }).waitFor();
  await page.screenshot({
    path: join(artifacts, "canvas-light-mosaic-composer.png"),
    fullPage: true,
  });
  await composer.getByRole("button", { name: "Send" }).click();
  await lane.locator(".mosaic-card").filter({ hasText: "canvas quote reply" }).waitFor({
    timeout: 10_000,
  });
  const quoted = await reply(fixture, ["inbox", seeded.lane]);
  const quotedDirective = quoted.body.lanes[0].pending.find(
    (directive) => directive.text.includes("canvas quote reply"),
  );
  assert.ok(quotedDirective, "quote reply reaches the lane inbox");
  assert.match(quotedDirective.text, /> browser direction survives streaming/);
  assert.equal(quotedDirective.attachments.length, 2);
  assert.deepEqual(
    quotedDirective.attachments.map((attachment) => attachment.name).sort(),
    ["pixel-copy.png", "pixel.png"],
  );
  for (const attachment of quotedDirective.attachments) {
    assert.equal(attachment.content_type, "image/png");
    assert.match(attachment.data_url, /^data:image\/png;base64,/);
  }

  await lane.evaluate((target) => {
    const transfer = new DataTransfer();
    transfer.setData("text/plain", "dragged card direction");
    target.dispatchEvent(new DragEvent("drop", {
      bubbles: true,
      cancelable: true,
      dataTransfer: transfer,
    }));
  });
  await lane.locator(".mosaic-card").filter({ hasText: "dragged card direction" }).waitFor({
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

async function assertPackedHistory(page, extent, linked = false) {
  await page.getByRole("button", { name: "Chronology" }).click();
  const journal = page.locator('[data-pane="commands"]');
  const facts = journal.locator(".event strong");
  await facts.nth(extent.facts - 1).waitFor();
  assert.equal(await facts.count(), extent.facts, `all ${extent.facts} admitted facts render`);

  const provenance = await journal.locator('[data-subject="facts"]').allTextContents();
  assert.ok(provenance.length > 1, "two-row packing produces multiple fact pages");
  assert.match(provenance[0], /omitted \/ continue/, "first page exposes overflow continuation");
  assert.doesNotMatch(provenance.at(-1), /omitted \/ continue/, "final page closes continuation");
  const horizons = await journal
    .locator('[data-subject="facts"] span')
    .evaluateAll((nodes) => nodes.map((node) => node.textContent?.trim()).filter((text) => text?.startsWith("horizon ")));
  const horizonValues = horizons.map((text) => Number(text.split(" ")[1]));
  assert.equal(horizonValues.at(-1), extent.cursor, "latest page reaches the current horizon");
  assert.ok(
    horizonValues.every((value, index) => index === 0 || value >= horizonValues[index - 1]),
    "continued and later live pages retain monotone packed horizons",
  );

  const sequences = await journal.locator(".event strong").evaluateAll((nodes) => nodes.map((node) => {
    const value = Number(node.textContent?.split("/")[0].trim());
    if (!Number.isInteger(value)) throw new Error(`invalid rendered sequence: ${node.textContent}`);
    return value;
  }));
  assert.equal(new Set(sequences).size, extent.facts, "continuation fabricates no duplicate fact");
  assert.deepEqual(
    [...sequences].sort((left, right) => left - right),
    Array.from({ length: extent.facts }, (_, index) => index + 1),
    "continuation loses no fact before the packed horizon",
  );

  await page.getByRole("button", { name: "Topology" }).click();
  const thread = page.locator('[data-pane="lineage"]');
  const minimumRows = linked ? 2 : 1;
  const threadRowsLocator = thread.locator("tbody tr");
  await threadRowsLocator.nth(minimumRows - 1).waitFor();
  assert.ok(
    await threadRowsLocator.count() >= minimumRows,
    linked ? "multi-link causal thread renders" : "terminal causal thread renders",
  );
  assert.match(await thread.locator(".section-count").innerText(), /root/);
  const threadRows = await thread.locator("tbody tr").allTextContents();
  if (linked) {
    assert.ok(threadRows.some((row) => !row.endsWith("root")), "causal walk includes a linked cause");
  }

  await page.getByRole("button", { name: "Chronology" }).click();
  await page.screenshot({ path: join(artifacts, "canvas-chronology.png"), fullPage: true });
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
  await page.waitForSelector(".status.offline", { timeout: 10_000 });
  await page.getByRole("button", { name: "Chronology" }).click();
  const fault = page.locator(".event.fault");
  await fault.waitFor();
  assert.match(await fault.innerText(), /Canvas connection reset/);
  assert.match(await fault.innerText(), /accepted|frame|epoch|socket/i);
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
  await page.getByRole("button", { name: "Chronology" }).click();
  const before = await page.locator('[data-pane="commands"] .event strong').allTextContents();
  monitor.expectNetworkInterruption(true);
  await page.evaluate(() => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    socket.close(4000, "fixture reconnect");
  });
  await page.waitForSelector(".status.offline", { timeout: 10_000 });
  assert.match(await page.locator(".status-copy").innerText(), /Reconnecting/);
  await page.getByRole("button", { name: "Chronology" }).click();
  await page.locator(".event.fault").waitFor();

  await reply(fixture, ["task", "note", seeded.task, "written during browser disconnect"]);
  const advanced = await journalExtent(fixture);
  assert.ok(advanced.facts > beforeExtent.facts, "offline command advances resident authority");

  await page.waitForSelector(".status.online", { timeout: 15_000 });
  await page.waitForFunction((expected) => document.querySelector(".status-copy")?.textContent?.includes(expected), epoch);
  monitor.expectNetworkInterruption(false);
  await assertPackedHistory(page, advanced);
  const after = await page.locator('[data-pane="commands"] .event strong').allTextContents();
  assert.ok(after.length > before.length, "same-generation resume appends missed facts");
}

async function routeSpeechBootstrap(page, speech) {
  await page.route("**/bootstrap", async (route) => {
    const response = await route.fetch();
    const bootstrap = await response.json();
    bootstrap.speech = speech;
    await route.fulfill({
      response,
      contentType: "application/json; charset=utf-8",
      body: JSON.stringify(bootstrap),
    });
  });
}

async function installBrowserSpeech(context, voices = ["Zoe (Premium)"]) {
  await context.addInitScript((voiceNames) => {
    window.__speechLog = [];
    class FakeVoice {
      constructor(name) { this.name = name; }
    }
    class FakeUtterance {
      constructor(text) {
        this.text = text;
        this.voice = null;
        this.rate = 1;
        this.onend = null;
        this.onerror = null;
      }
    }
    class FakeAudio {
      constructor(src) {
        this.src = src;
        this.currentTime = 0;
        this.onended = null;
        this.onerror = null;
        window.__speechLog.push({ action: "audio", src });
      }
      play() {
        window.__speechLog.push({ action: "play" });
        return Promise.resolve();
      }
      pause() { window.__speechLog.push({ action: "pause" }); }
    }
    window.SpeechSynthesisVoice = FakeVoice;
    window.SpeechSynthesisUtterance = FakeUtterance;
    window.Audio = FakeAudio;
    Object.defineProperty(window, "speechSynthesis", {
      configurable: true,
      value: {
        cancel() { window.__speechLog.push({ action: "cancel" }); },
        getVoices() { return voiceNames.map((name) => new FakeVoice(name)); },
        speak(utterance) {
          window.__speechLog.push({
            action: "speak",
            text: utterance.text,
            voice: utterance.voice?.name,
            rate: utterance.rate,
          });
        },
      },
    });
  }, voices);
}

async function installSocketControl(context) {
  await context.addInitScript(() => {
    const NativeWebSocket = window.WebSocket;
    window.__canvasSockets = [];
    window.WebSocket = class FixtureWebSocket extends NativeWebSocket {
      constructor(...args) {
        super(...args);
        window.__canvasSockets.push(this);
      }
    };
  });
}

async function assertSystemSpeech(page) {
  await page.getByRole("button", { name: "Flow" }).click();
  const button = page.locator(".speech-button");
  await button.waitFor();
  assert.equal(await button.getAttribute("aria-label"), "Read current canvas");
  const before = await button.boundingBox();
  assert.ok(before, "speech control has stable geometry");
  await button.click();
  await page.locator('.speech-button[data-state="playing"]').waitFor();
  assert.equal(await button.getAttribute("aria-label"), "Stop reading");
  const after = await button.boundingBox();
  assert.deepEqual(after, before, "play state does not shift the speech control");
  const spoken = await page.evaluate(() => window.__speechLog.find((row) => row.action === "speak"));
  assert.equal(spoken.voice, "Zoe (Premium)");
  assert.equal(spoken.rate, 1);
  assert.match(spoken.text, /Flow/);
  assert.match(spoken.text, /Current work/);
  await button.click();
  await page.locator('.speech-button[data-state="idle"]').waitFor();
  const actions = await page.evaluate(() => window.__speechLog.map((row) => row.action));
  assert.ok(actions.includes("cancel"), "stop cancels system speech");
}

async function assertSpeechVariants(browser, endpoint) {
  const unavailableContext = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  await installBrowserSpeech(unavailableContext, []);
  const unavailablePage = await unavailableContext.newPage();
  const unavailableMonitor = monitorPage(unavailablePage);
  await unavailablePage.goto(endpoint, { waitUntil: "networkidle" });
  await unavailablePage.waitForSelector(".status.online");
  await unavailablePage.locator(".speech-button").click();
  await unavailablePage.locator('.speech-button[data-state="error"]').waitFor();
  assert.match(await unavailablePage.locator(".speech-button").getAttribute("title"), /Zoe.*unavailable/);
  unavailableMonitor.assertClean();
  await unavailableContext.close();

  const variants = await browser.newContext({ viewport: { width: 1024, height: 768 } });
  await installBrowserSpeech(variants);

  const disabled = await variants.newPage();
  const disabledMonitor = monitorPage(disabled);
  await routeSpeechBootstrap(disabled, { backend: "disabled" });
  await disabled.goto(endpoint, { waitUntil: "networkidle" });
  await disabled.waitForSelector(".status.online");
  assert.equal(await disabled.locator(".speech-button").count(), 0, "disabled hides playback");
  disabledMonitor.assertClean();
  await disabled.close();

  const externalUnavailable = await variants.newPage();
  const externalUnavailableMonitor = monitorPage(externalUnavailable);
  await routeSpeechBootstrap(externalUnavailable, {
    backend: "external",
    content_type: "audio/wav",
    words_per_minute: 175,
    available: false,
  });
  await externalUnavailable.goto(endpoint, { waitUntil: "networkidle" });
  await externalUnavailable.waitForSelector(".status.online");
  const unavailableButton = externalUnavailable.locator(".speech-button");
  assert.equal(await unavailableButton.getAttribute("aria-label"), "Speech unavailable");
  assert.equal(await unavailableButton.isDisabled(), true);
  externalUnavailableMonitor.assertClean();
  await externalUnavailable.close();

  const external = await variants.newPage();
  const externalMonitor = monitorPage(external);
  await routeSpeechBootstrap(external, {
    backend: "external",
    content_type: "audio/wav",
    words_per_minute: 175,
    available: true,
  });
  let submitted;
  await external.route("**/speech", async (route) => {
    submitted = route.request().postDataJSON();
    await route.fulfill({
      status: 200,
      contentType: "audio/wav",
      body: Buffer.from("fixture-audio"),
    });
  });
  await external.goto(endpoint, { waitUntil: "networkidle" });
  await external.waitForSelector(".status.online");
  await external.getByRole("button", { name: "Flow" }).click();
  const externalButton = external.locator(".speech-button");
  await externalButton.click();
  await external.locator('.speech-button[data-state="playing"]').waitFor();
  assert.deepEqual(Object.keys(submitted), ["text"]);
  assert.match(submitted.text, /Flow/);
  await externalButton.click();
  await external.locator('.speech-button[data-state="idle"]').waitFor();
  const externalActions = await external.evaluate(() => window.__speechLog.map((row) => row.action));
  assert.ok(externalActions.includes("play"));
  assert.ok(externalActions.includes("pause"));
  externalMonitor.assertClean();
  await external.close();

  const malformed = await variants.newPage();
  const malformedMonitor = monitorPage(malformed);
  await routeSpeechBootstrap(malformed, { backend: "system", voice: "Zoe (Premium)" });
  await malformed.goto(endpoint, { waitUntil: "networkidle" });
  await malformed.waitForSelector(".status.offline");
  assert.match(await malformed.locator(".status-copy").innerText(), /bootstrap response was invalid/);
  assert.equal(await malformed.locator(".speech-button").count(), 0);
  await malformed.getByRole("button", { name: "Chronology" }).click();
  assert.match(await malformed.locator(".event.fault").innerText(), /bootstrap response was invalid/);
  malformedMonitor.assertClean();
  await malformed.close();

  await variants.close();
}

await buildResident();
const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-browser-")));
const memberWorktree = `${fixture}-member`;
await mkdir(artifacts, { recursive: true });
await configureFixture(fixture);
await execute("git", ["init", "-q"], { cwd: fixture });
await execute("git", ["add", "spice.toml"], { cwd: fixture });
await execute("git", [
  "-c", "user.name=Canvas Fixture",
  "-c", "user.email=canvas@spice.invalid",
  "commit", "-q", "-m", "canvas fixture",
], { cwd: fixture });
await execute("git", ["worktree", "add", "-q", "-b", "browser-member", memberWorktree], {
  cwd: fixture,
});
process.stderr.write("canvas spec: serving dist/stage-app, the app this worktree built\n");

let resident;
let browser;
try {
  resident = await startResident(fixture);
  const seeded = await seedAuthority(fixture);
  browser = await chromium.launch({ headless: true });

  const light = await browser.newContext({
    colorScheme: "light",
    viewport: { width: 1440, height: 900 },
  });
  await installBrowserSpeech(light);
  await installSocketControl(light);
  const lightPage = await light.newPage();
  const lightMonitor = monitorPage(lightPage);
  lightPage.on("console", (message) => process.stderr.write(`browser console: ${message.text()}\n`));
  await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await assertCanvas(lightPage, "light");
  Object.assign(seeded, await assertAgentConversation(lightPage, fixture, memberWorktree));
  seeded.extent = await journalExtent(fixture);
  await assertAuthoritySurface(lightPage, seeded);
  await assertMosaicComposer(lightPage, fixture, seeded);
  await assertPackedHistory(lightPage, await journalExtent(fixture), true);
  await assertNoCredentialOrLandingGap(lightPage, [resident.discovery.capability]);
  await assertSystemSpeech(lightPage);
  await lightPage.getByRole("button", { name: "Flow" }).click();
  await lightPage.getByLabel("Task").fill(seeded.task);
  await lightPage.getByLabel("Observation").fill("browser mutation reached the resident");
  await lightPage.getByRole("button", { name: "Add note" }).click();
  await lightPage.getByRole("button", { name: "Chronology" }).click();
  await lightPage.getByText(/Note appended to/).waitFor({ timeout: 10_000 });
  await lightPage.getByRole("button", { name: "Flow" }).click();
  const existingTasks = await lightPage.locator("#note-tasks option").evaluateAll(
    (options) => options.map((option) => option.value),
  );
  const absentTask = ["SPICE-aBcD", "SPICE-zYxW"].find(
    (candidate) => !existingTasks.includes(candidate),
  );
  assert.ok(absentTask, "an absent canonical task handle is available");
  await lightPage.getByLabel("Task").fill(absentTask);
  await lightPage.getByLabel("Observation").fill("this task does not exist");
  await lightPage.getByRole("button", { name: "Add note" }).click();
  await lightPage.getByRole("button", { name: "Chronology" }).click();
  await lightPage.locator(".event.refusal").waitFor({ timeout: 10_000 });
  const beforeReconnect = await journalExtent(fixture);
  await assertPackedHistory(lightPage, beforeReconnect);

  const dark = await browser.newContext({
    colorScheme: "dark",
    viewport: { width: 390, height: 844 },
  });
  await installBrowserSpeech(dark);
  const darkPage = await dark.newPage();
  const darkMonitor = monitorPage(darkPage);
  await darkPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await assertCanvas(darkPage, "dark-narrow");
  await assertAuthoritySurface(darkPage, seeded);
  await assertNoCredentialOrLandingGap(darkPage, [resident.discovery.capability]);
  darkMonitor.assertClean();
  await assertSpeechVariants(browser, resident.discovery.endpoint);
  await assertStaleGeneration(browser, resident.discovery.endpoint, resident.discovery.epoch);

  await assertSameGenerationResume(
    lightPage,
    lightMonitor,
    fixture,
    seeded,
    resident.discovery.epoch,
    beforeReconnect,
  );
  lightMonitor.assertClean();
  await dark.close();

  await reply(fixture, ["lane", "drain", seeded.lane]);
  await reply(fixture, ["lane", "drain", seeded.memberLane]);
  const drainedLineage = await reply(fixture, ["lane"]);
  assert.ok(
    drainedLineage.body.lanes
      .filter((lane) => lane.lineage_id === seeded.lineage)
      .every((lane) => lane.occupant === undefined),
    "every member is vacant before resident shutdown",
  );
  const firstEpoch = resident.discovery.epoch;
  const firstCapability = resident.discovery.capability;
  lightMonitor.expectNetworkInterruption(true);
  await stopResident(fixture, resident);
  await lightPage.waitForSelector(".status.offline", { timeout: 10_000 });
  await lightPage.getByRole("button", { name: "Chronology" }).click();
  await lightPage.locator(".event.fault").waitFor({ timeout: 10_000 });
  resident = await startResident(fixture);
  assert.notEqual(resident.discovery.epoch, firstEpoch, "restart publishes a new generation");
  assert.notEqual(
    resident.discovery.capability,
    firstCapability,
    "restart publishes a new operator capability",
  );
  await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await lightPage.waitForSelector(".status.online", { timeout: 10_000 });
  await lightPage.waitForFunction(
    (epoch) => document.querySelector(".status-copy")?.textContent?.includes(epoch),
    resident.discovery.epoch,
  );
  lightMonitor.expectNetworkInterruption(false);
  await assertCanvas(lightPage, "light-restarted");
  await lightPage.getByRole("button", { name: "Chronology" }).click();
  assert.equal(
    await lightPage.getByText(/Note appended to/).count(),
    0,
    "new generation clears prior command reply state",
  );
  const restartedExtent = await journalExtent(fixture);
  await assertPackedHistory(lightPage, restartedExtent);
  await assertAuthoritySurface(lightPage, seeded);
  await assertNoCredentialOrLandingGap(lightPage, [firstCapability, resident.discovery.capability]);
  lightMonitor.assertClean();

  await light.close();
} finally {
  if (browser) await browser.close();
  if (resident?.child.exitCode === null) {
    const exited = once(resident.child, "exit");
    resident.child.kill("SIGKILL");
    await exited;
  }
  await rm(fixture, { recursive: true, force: true });
  await rm(memberWorktree, { recursive: true, force: true });
}
