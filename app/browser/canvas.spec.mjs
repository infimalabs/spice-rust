import assert from "node:assert/strict";
import { once } from "node:events";
import { mkdtemp, mkdir, realpath, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { chromium } from "playwright";
import {
  assertLineageLifeIsolation,
  assertSpeechVariants,
  assertSystemSpeech,
  buildResident,
  chooseCanvasView,
  configureFixture,
  installBrowserSpeech,
  installSocketControl,
  journalExtent,
  monitorPage,
  reply,
  seedAuthority,
  startResident,
  stopResident,
} from "./canvas-fixture.mjs";

const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const artifacts = join(root, "artifacts/canvas");
const coverageReceiptFlag = "--spice-coverage-receipt";
const coverageReceipt = "BROWSER-1kKSWhZk browser coverage complete\n";
const coverageArguments = process.argv.slice(2);
let coverageReceiptPath;
if (coverageArguments.length > 0) {
  if (
    coverageArguments.length !== 2 ||
    coverageArguments[0] !== coverageReceiptFlag ||
    !coverageArguments[1] ||
    coverageArguments[1].startsWith("--")
  ) {
    throw new Error(`expected ${coverageReceiptFlag} FILE or no arguments`);
  }
  coverageReceiptPath = coverageArguments[1];
}
const agentNames = ["Ava", "Bex", "Cy", "Dee"];
const staleEpoch = "018f9999-9999-7444-8888-999999999999";
const nestedQuoteMessage = [
  "> > The final review found no alternate execution path.",
  "> >",
  "> > `cargo xtask verify` is green.",
  ">",
  "> Why it matters: nested context remains visible.",
  "",
  "The author words belong in the composer band.",
].join("\n");
const nestedQuotePreview = "The author words belong in the composer band.";
const entirelyQuotedMessage = [
  "> > Borrowed context has no authored preview.",
  "> > A second borrowed line stays reachable.",
].join("\n");

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


async function assertCanvas(page, scheme) {
  try {
    await page.waitForFunction(() => /cursor \d+/.test(
      document.querySelector(".bar-meta")?.textContent ?? "",
    ), null, { timeout: 10_000 });
  } catch (error) {
    const body = await page.locator("body").innerText().catch(() => "<body unavailable>");
    throw new Error(`canvas did not come online; body=${JSON.stringify(body)}`, { cause: error });
  }
  await page.waitForFunction(() => document.querySelector("#app")?.childElementCount > 0);
  await assert.doesNotReject(() => page.locator(".canvas-menu > summary:visible, .brand-name:visible").waitFor());
  assert.ok(
    await page.locator('[data-pane="agent-console"]').count() <= 1,
    "the active view never mounts duplicate lineage consoles",
  );
  assert.equal(
    await page.locator("button.axis-button", { hasText: /^Mosaic/ }).count(),
    0,
    "the internal packing name is not a user-facing view",
  );
  assert.equal(
    await page.locator("button.axis-button", { hasText: /^Agents/ }).count(),
    1,
    "one Agents entry reaches the one conversation surface",
  );
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
    await chooseCanvasView(page, axis);
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
    await chooseCanvasView(page, "Flow");
    await page.locator(".workspace").evaluate((element) => element.scrollTo(0, element.scrollHeight));
    assert.ok(await page.getByRole("button", { name: "Add note" }).isVisible());
    await page.locator(".workspace").evaluate((element) => element.scrollTo(0, 0));
  }
  await chooseCanvasView(page, "Agents");
  await page.locator('[data-pane="agent-console"]').waitFor();
  await page.screenshot({ path: join(artifacts, `canvas-${scheme}.png`), fullPage: true });
}

async function assertAgentConversation(page, fixture, worktrees) {
  const teamWorktrees = worktrees.slice(0, 4);
  const console = page.locator('[data-pane="agent-console"]');
  await chooseCanvasView(page, "Agents");
  await console.waitFor();
  await console.getByRole("tab", { name: /^Info/ }).click();
  await console.getByLabel("Name").fill(agentNames[0]);
  await console.getByLabel("Worktree").fill(teamWorktrees[0]);
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
  await console.locator(".team-detail-card").nth(0).waitFor({ timeout: 15_000 });
  const firstSeatedAt = Date.now();

  const firstSnapshot = await reply(fixture, ["lane"]);
  const host = firstSnapshot.body.lanes.find((lane) => lane.worktree === teamWorktrees[0]);
  assert.ok(host, "the Canvas seat control creates the lineage host");
  for (const [index, worktree] of teamWorktrees.slice(1).entries()) {
    await reply(fixture, [
      "lane", "add", worktree,
      "--lineage", host.lineage_id,
      "--name", agentNames[index + 1],
      "--life", "Steer",
      "--vendor", "claude",
    ]);
  }
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Agents");
  await console.getByRole("tab", { name: /^Info/ }).click();
  await console.locator(".team-detail-card").nth(3).waitFor({ timeout: 15_000 });

  const lanes = await reply(fixture, ["lane"]);
  const seated = teamWorktrees.map(
    (worktree) => lanes.body.lanes.find((lane) => lane.worktree === worktree),
  );
  assert.ok(seated.every(Boolean), "one lineage admits all four linked worktrees");
  assert.ok(
    seated.every((lane) => lane.lineage_id === host.lineage_id),
    "every independently steerable member retains the shared lineage identity",
  );
  assert.deepEqual(seated.map((lane) => lane.name), agentNames);
  assert.ok(seated.every((lane) => lane.vendor === "claude"));
  assert.ok(seated.every((lane) => lane.life === "steer"), "the team shares one lifetime");

  await console.getByRole("tab", { name: /^Compose/ }).click();
  await console.locator(".team-agent").nth(3).waitFor();
  assert.equal(await console.locator(".team-surface").count(), 1, "one lineage renders one surface");
  assert.equal(
    await console.locator(".team-activity-grid").count(),
    1,
    "all lineage members share exactly one packed activity plane",
  );
  const authored = console.locator('[data-card-kind="agent-message"]');
  await authored.nth(3).waitFor({ timeout: 10_000 });
  const producerIds = new Set(await authored.evaluateAll((cards) =>
    cards.map((card) => card.getAttribute("data-producer-target-id")),
  ));
  const producerNames = new Set(await authored.locator(".message-author").allTextContents());
  assert.ok(producerIds.size > 1, "the packed plane contains more than one producing agent");
  assert.ok(producerNames.size > 1, "each authored card footer names its producing agent");
  await assertRosterPips(page, console, seated);
  const accessibility = await console.locator(".team-activity-grid").ariaSnapshot();
  for (const name of producerNames) {
    assert.match(accessibility, new RegExp(name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")));
  }
  assert.equal(await console.getByRole("button", { name: "Steer", exact: true }).count(), 1);
  const lifetime = console.locator('.team-lifetime input[type="range"]');
  await lifetime.fill("2");
  await console.getByRole("button", { name: "Drain", exact: true }).waitFor();
  await lifetime.fill("0");
  await console.getByRole("button", { name: "Steer", exact: true }).waitFor();
  const messages = [
    "browser direction survives streaming",
    "## Gate contract\n\n- green on the integrated tip\n- exact checks retained",
    "> The reference viewport keeps **all agent status** visible.\n>\n> Density remains readable.\n\n`1000×568` stays bounded.",
    nestedQuoteMessage,
  ];
  const messageLabels = [
    messages[0],
    "Gate contract",
    "reference viewport keeps",
    "final review found",
  ];
  for (const [index, lane] of seated.entries()) {
    const column = console.locator(`.team-agent[data-agent-lane="${lane.lane}"]`);
    await column.getByRole("textbox").fill(messages[index]);
    await console.getByRole("button", { name: "Steer", exact: true }).click();
    await console
      .locator(`.mosaic-message-card[data-agent-lane="${lane.lane}"]`)
      .filter({ hasText: messageLabels[index] })
      .waitFor({ timeout: 10_000 });
  }
  const quoteLane = console.locator(`.team-agent[data-agent-lane="${seated[3].lane}"]`);
  await quoteLane.getByRole("textbox").fill(entirelyQuotedMessage);
  await console.getByRole("button", { name: "Steer", exact: true }).click();
  await console
    .locator(`.mosaic-message-card[data-agent-lane="${seated[3].lane}"]`)
    .filter({ hasText: "Borrowed context has no authored preview." })
    .waitFor({ timeout: 10_000 });
  assert.ok(await console.locator(".mosaic-message-card h2").count(), "cards render headings");
  assert.ok(await console.locator(".mosaic-message-card ul li").count(), "cards render lists");
  assert.ok(
    await console.locator(".mosaic-message-card blockquote blockquote").count(),
    "cards preserve nested quote depth",
  );
  const cardHeights = await console.locator(".mosaic-message-card").evaluateAll((cards) =>
    cards.map((card) => Math.round(card.getBoundingClientRect().height)),
  );
  assert.ok(new Set(cardHeights).size >= 3, "Markdown produces nonuniform Mosaic card heights");
  const planes = console.locator(".lane-mosaic-plane");
  assert.equal(await planes.count(), 1, "one seated lineage owns one Mosaic plane");
  const plane = console.locator(
    `.lane-mosaic-plane[data-agent-plane="${host.lineage_id}"]`,
  );
  await plane.waitFor();
  const placements = await plane.locator(".mosaic-message-card").evaluateAll((cards) =>
    cards.map((card) => ({
      lane: card.dataset.agentLane,
      track: Number(card.dataset.track),
      span: Number(card.dataset.span),
      base: Number(card.dataset.base),
      row: Number(card.dataset.row),
      rows: Number(card.dataset.rows),
      columnStart: Number(getComputedStyle(card).gridColumnStart),
      rowStart: Number(getComputedStyle(card).gridRowStart),
    })),
  );
  assert.ok(placements.length > seated.length, "the lane packs agent and directive cards");
  assert.ok(placements.every((card) => card.span >= 1 && card.track + card.span <= 12));
  assert.ok(placements.every((card) => card.rows >= 2));
  assert.ok(placements.every((card) => card.columnStart === card.track + 1));
  assert.ok(placements.every((card) => card.rowStart === card.row));
  assert.equal(Math.min(...placements.map((card) => card.base)), 0, "the lane owns its row floor");
  const packedMembers = new Set(placements.map((card) => card.lane));
  for (const member of seated) {
    assert.ok(packedMembers.has(member.lane), "every worktree member reaches the lane plane");
  }
  await page.locator('.task-pill[data-task-stem="app"]').waitFor();
  await page.locator('.task-pill[data-task-stem="canvas"]').waitFor();
  assert.match(await page.locator('.task-pill[data-task-stem="app"]').innerText(), /\d+·\d+·\d+/);

  const outgoing = console
    .locator(`.mosaic-message-card[data-agent-lane="${seated[0].lane}"]`)
    .filter({ hasText: messages[0] });
  const inbox = await reply(fixture, ["inbox", seated[0].lane]);
  assert.equal(inbox.body.lanes.length, 1);
  assert.equal(inbox.body.lanes[0].pending.length, 1);
  assert.equal(inbox.body.lanes[0].pending[0].text, messages[0]);
  assert.equal(
    await outgoing.locator(".message-key").innerText(),
    inbox.body.lanes[0].pending[0].key,
  );
  await assertQueuedRosterPip(page, console, seated[0]);
  assert.match(await outgoing.locator(".message-age").innerText(), / left$/);
  assert.equal(await outgoing.locator(".message-actions button").count(), 3);

  for (const mode of ["Filters", "Metrics", "Info"]) {
    await console.getByRole("tab", { name: new RegExp(`^${mode}`) }).click();
    await console.locator(".team-detail-card").nth(3).waitFor();
    assert.equal(await console.locator(".team-detail-card").count(), 4, `${mode} reaches all agents`);
  }
  await console.getByText(teamWorktrees[0], { exact: true }).waitFor();
  await console.getByRole("tab", { name: /^Compose/ }).click();
  await assertFourAgentDensity(console);
  await page.screenshot({ path: join(artifacts, "canvas-light-mosaic-agent.png"), fullPage: true });
  const clearedInbox = await reply(fixture, ["inbox", seated[2].lane]);
  for (const pending of clearedInbox.body.lanes[0].pending) {
    await reply(fixture, ["steer", "cancel", pending.directive]);
  }
  const remainingQuietWindow = Math.max(0, 61_000 - (Date.now() - firstSeatedAt));
  await new Promise((resolveWait) => setTimeout(resolveWait, remainingQuietWindow));
  return { lane: seated[0].lane, quoteLane: seated[3].lane };
}


async function assertFourAgentDensity(console) {
  const geometry = await console.evaluate((surface) => ({
    surfaceWidth: surface.getBoundingClientRect().width,
    contentWidth: surface.parentElement?.getBoundingClientRect().width ?? 0,
    surfaceOverflow: surface.scrollWidth - surface.clientWidth,
    headers: [...surface.querySelectorAll(".composer-shard-heading")].map((node) => {
      const box = node.getBoundingClientRect();
      return { left: box.left, right: box.right, top: box.top, bottom: box.bottom };
    }),
    columns: [...surface.querySelectorAll(".team-agent")].map((node) => {
      const box = node.getBoundingClientRect();
      return { left: box.left, right: box.right, top: box.top, bottom: box.bottom };
    }),
  }));
  assert.equal(geometry.headers.length, 4, "four intelligent draft headers are simultaneous");
  assert.equal(geometry.columns.length, 4, "four live composer columns are simultaneous");
  assert.ok(
    geometry.surfaceWidth >= geometry.contentWidth - 16 &&
      geometry.surfaceWidth <= geometry.contentWidth + 1,
    "one live lineage consumes the available reference viewport",
  );
  assert.ok(geometry.surfaceOverflow <= 1, "the team surface does not hide horizontal content");
  for (const row of [geometry.headers, geometry.columns]) {
    for (let index = 1; index < row.length; index += 1) {
      assert.ok(row[index - 1].right <= row[index].left + 1, "agent columns never overlap");
      assert.ok(Math.abs(row[index - 1].top - row[index].top) <= 1, "agent columns share one row");
    }
  }
}

async function assertRosterPips(page, console, seated) {
  const teamPips = console.locator(".team-light");
  await teamPips.nth(seated.length - 1).waitFor();
  const teamRoster = await teamPips.evaluateAll((pips) => pips.map((pip) => ({
    lane: pip.dataset.laneLightTargetId,
    status: pip.dataset.laneStatus,
  })).sort((left, right) => left.lane.localeCompare(right.lane)));
  assert.deepEqual(
    teamRoster.map((entry) => entry.lane),
    seated.map((member) => member.lane).sort(),
    "the team pips are keyed by every admitted member and no decorative slot",
  );
  assert.ok(
    teamRoster.every((entry) => ["running", "running-stale"].includes(entry.status)),
    "every supervised member has a lit liveness pip",
  );

  await chooseCanvasView(page, "Flow");
  const brandPips = page.locator(".brand-dot");
  await brandPips.nth(seated.length - 1).waitFor();
  const chromeRoster = await brandPips.evaluateAll((pips) => pips.map((pip) => ({
    lane: pip.dataset.agentLane,
    status: pip.dataset.agentStatus,
  })).sort((left, right) => left.lane.localeCompare(right.lane)));
  assert.deepEqual(
    chromeRoster,
    teamRoster,
    "the compact chrome roster preserves the live team identities and liveness states",
  );
  assert.equal(
    await page.locator("button.axis-button", { hasText: /^Mosaic/ }).count(),
    0,
    "analytic chrome keeps the layout implementation name private",
  );
  await chooseCanvasView(page, "Agents");
  await console.waitFor();
}

async function assertQueuedRosterPip(page, console, member) {
  const teamPip = console.locator(
    `.team-light[data-lane-light-target-id="${member.lane}"]`,
  );
  await page.waitForFunction((lane) => (
    document.querySelector(`.team-light[data-lane-light-target-id="${lane}"]`)
      ?.getAttribute("data-lane-status") === "queued"
  ), member.lane);
  const teamStyle = await teamPip.evaluate((pip) => {
    const style = getComputedStyle(pip);
    return {
      backgroundColor: style.backgroundColor,
      boxShadow: style.boxShadow,
      opacity: style.opacity,
    };
  });

  await chooseCanvasView(page, "Flow");
  const brandPip = page.locator(`.brand-dot[data-agent-lane="${member.lane}"]`);
  assert.equal(await brandPip.count(), 1, "the queued member retains exactly one brand pip");
  await page.waitForFunction((lane) => (
    document.querySelector(`.brand-dot[data-agent-lane="${lane}"]`)
      ?.getAttribute("data-agent-status") === "queued"
  ), member.lane);
  assert.equal(await brandPip.getAttribute("data-agent-status"), "queued");
  const brandStyle = await brandPip.evaluate((pip) => {
    const style = getComputedStyle(pip);
    return {
      backgroundColor: style.backgroundColor,
      boxShadow: style.boxShadow,
      opacity: style.opacity,
    };
  });
  assert.deepEqual(
    brandStyle,
    teamStyle,
    "a queued live member keeps the team roster's visible warning state in the brand roster",
  );
  assert.equal(brandStyle.opacity, "1", "queued direction never mutes an occupied agent");
  assert.notEqual(brandStyle.boxShadow, "none", "the queued agent remains visibly lit");
  await chooseCanvasView(page, "Agents");
  await console.waitFor();
}

async function assertPerTeamWatchbands(page, fixture, worktrees) {
  const before = await reply(fixture, ["lane"]);
  const primary = before.body.lanes.find((lane) => lane.worktree === worktrees[0]);
  assert.ok(primary, "the primary team remains seated for watchband proof");
  await reply(fixture, [
    "lane", "add", worktrees[4],
    "--life", "Drive",
    "--vendor", "claude",
  ]);
  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Agents");
  const console = page.locator('[data-pane="agent-console"]');
  await console.getByRole("tab", { name: /^Compose/ }).first().click();
  await console.locator(".team-surface").nth(1).waitFor({ timeout: 15_000 });

  const lanes = await reply(fixture, ["lane"]);
  const secondary = lanes.body.lanes.find((lane) => lane.worktree === worktrees[4]);
  assert.ok(secondary, "the second team is independently seated");
  const primarySurface = console.locator(
    `.team-surface[data-lineage-id="${primary.lineage_id}"]`,
  );
  const secondarySurface = console.locator(
    `.team-surface[data-lineage-id="${secondary.lineage_id}"]`,
  );
  assert.equal(await console.locator(".team-surface").count(), 2);
  assert.equal(await console.locator(".team-speech").count(), 2);
  assert.equal(await console.locator(".team-lifetime").count(), 2);

  const primaryLife = primarySurface.locator('.team-lifetime input[type="range"]');
  const primarySeatLives = new Map(
    lanes.body.lanes
      .filter((lane) => lane.lineage_id === primary.lineage_id)
      .map((lane) => [lane.lane, lane.life]),
  );
  const initialPrimaryPolicy = lanes.body.subscriptions.find(
    (subscription) => subscription.lineage_id === primary.lineage_id,
  )?.policy;
  assert.ok(initialPrimaryPolicy, "the primary team has one authoritative task policy");
  const initialPrimaryLabel = `${initialPrimaryPolicy[0].toUpperCase()}${initialPrimaryPolicy.slice(1)}`;
  const initialPrimaryIndex = { steer: "0", drive: "1", drain: "2" }[initialPrimaryPolicy];
  await page.evaluate((missingLineage) => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    const send = socket.send.bind(socket);
    let intercepted = false;
    const rewrite = (bytes) => {
      const command = JSON.parse(new TextDecoder().decode(bytes));
      if (!intercepted && command.kind === "lineage::configure") {
        intercepted = true;
        command.body.lineage_id = missingLineage;
        return new TextEncoder().encode(JSON.stringify(command));
      }
      return bytes;
    };
    socket.send = (data) => {
      if (typeof data === "string") {
        const rewritten = rewrite(new TextEncoder().encode(data));
        send(new TextDecoder().decode(rewritten));
      } else if (data instanceof Blob) {
        void data.arrayBuffer().then((buffer) => send(rewrite(new Uint8Array(buffer))));
      } else {
        send(rewrite(new Uint8Array(data.buffer ?? data, data.byteOffset ?? 0, data.byteLength)));
      }
    };
  }, staleEpoch);
  await primaryLife.fill("2");
  await page.waitForFunction(
    (lineage) => document
      .querySelector(`.team-surface[data-lineage-id="${lineage}"] .team-composer-controls`)
      ?.dataset.lifetimePending === "false",
    primary.lineage_id,
  );
  await primarySurface
    .getByRole("button", { name: initialPrimaryLabel, exact: true })
    .waitFor();
  assert.equal(await primaryLife.inputValue(), initialPrimaryIndex);
  assert.match(await primarySurface.locator(".team-control-issue").innerText(), /lineage::missing/);

  await primaryLife.fill("2");
  await page.waitForFunction(
    (lineage) => document
      .querySelector(`.team-surface[data-lineage-id="${lineage}"] .team-composer-controls`)
      ?.dataset.lifetimePending === "false",
    primary.lineage_id,
  );
  await primarySurface.getByRole("button", { name: "Drain", exact: true }).waitFor();
  const configured = await reply(fixture, ["lane"]);
  const primaryRows = configured.body.lanes.filter(
    (lane) => lane.lineage_id === primary.lineage_id,
  );
  const primarySubscription = configured.body.subscriptions.find(
    (subscription) => subscription.lineage_id === primary.lineage_id,
  );
  assert.equal(primaryRows.length, 4, "the first team retains all four worktrees");
  assert.equal(primarySubscription?.policy, "drain");
  assert.ok(
    primaryRows.every((lane) => lane.life === primarySeatLives.get(lane.lane)),
    "watchband Drain never changes terminal lane life",
  );
  assert.ok(
    primaryRows.every((lane) => lane.occupant && lane.disposition === "occupied"),
    "watchband Drain changes task policy and never tears down occupants",
  );
  assert.equal(
    await secondarySurface.getByRole("button", { name: "Drive", exact: true }).count(),
    1,
    "the sibling team keeps its own authoritative lifetime",
  );

  const primarySpeech = primarySurface.locator('.team-speech input[type="range"]');
  const secondarySpeech = secondarySurface.locator('.team-speech input[type="range"]');
  await primarySpeech.fill("0");
  await secondarySpeech.fill("2");
  assert.equal(await primarySpeech.inputValue(), "0");
  assert.equal(await secondarySpeech.inputValue(), "2");
  assert.equal(await primarySurface.locator(".team-speech span").innerText(), "quiet");
  assert.equal(await secondarySurface.locator(".team-speech span").innerText(), "narrate");
  await console.getByRole("tab", { name: /^Info/ }).first().click();
  await console.getByRole("tab", { name: /^Compose/ }).first().click();
  assert.equal(await primarySpeech.inputValue(), "0", "Quiet survives a team redraw");
  assert.equal(await secondarySpeech.inputValue(), "2", "Narrate survives a team redraw");
  assert.equal(await primarySurface.locator(".team-speech span").innerText(), "quiet");
  assert.equal(await secondarySurface.locator(".team-speech span").innerText(), "narrate");

  await page.evaluate(() => { window.__speechLog = []; });
  await primarySurface.getByLabel("Speak this message").first().click();
  assert.deepEqual(
    await page.evaluate(() => window.__speechLog),
    [],
    "Quiet suppresses manual speech for only its team",
  );
  await secondarySurface.getByLabel("Speak this message").first().click();
  await page.waitForFunction(() => window.__speechLog.some((row) => row.action === "speak"));

  const policyMessage = "drain lifetime is task policy, not occupant teardown";
  await primarySurface.locator(".team-agent").first().getByRole("textbox").fill(policyMessage);
  await primarySurface.getByRole("button", { name: "Drain", exact: true }).click();
  await primarySurface
    .locator(".mosaic-message-card")
    .filter({ hasText: policyMessage })
    .waitFor({ timeout: 10_000 });
  const inbox = await reply(fixture, ["inbox", primaryRows[0].lane]);
  assert.ok(
    inbox.body.lanes[0].pending.some((directive) => directive.text === policyMessage),
    "the Drain-labelled submit follows the configured team policy into real steering",
  );
  assert.equal(await primarySpeech.inputValue(), "0", "Quiet survives a streamed refresh");
  assert.equal(await secondarySpeech.inputValue(), "2", "Narrate survives a streamed refresh");
  assert.equal(await primarySurface.locator(".team-speech span").innerText(), "quiet");
  assert.equal(await secondarySurface.locator(".team-speech span").innerText(), "narrate");

  const lifecycleLane = primaryRows[0];
  await reply(fixture, ["lane", "drain", lifecycleLane.lane]);
  const vacantSnapshot = await reply(fixture, ["lane"]);
  const vacant = vacantSnapshot.body.lanes.find((lane) => lane.lane === lifecycleLane.lane);
  assert.ok(vacant, "the configured member remains durable after leaving");
  assert.equal(vacant.disposition, "vacant");
  assert.equal(vacant.occupant, undefined);
  assert.equal(vacant.life, primarySeatLives.get(vacant.lane));
  assert.equal(
    vacantSnapshot.body.subscriptions.find(
      (subscription) => subscription.lineage_id === primary.lineage_id,
    )?.policy,
    "drain",
  );
  await reply(fixture, ["lane", "wake", lifecycleLane.lane]);
  const wokenSnapshot = await reply(fixture, ["lane"]);
  const woken = wokenSnapshot.body.lanes.find((lane) => lane.lane === lifecycleLane.lane);
  assert.ok(woken?.occupant, "watchband Drain leaves the member eligible for a normal wake");
  assert.equal(woken.disposition, "occupied");
  assert.equal(woken.life, primarySeatLives.get(woken.lane));
  await page.screenshot({
    path: join(artifacts, "canvas-light-watchband-teams.png"),
    fullPage: true,
  });
  return { lineage: primary.lineage_id, policy: "drain" };
}

async function captureReferenceViewport(page) {
  await chooseCanvasView(page, "Agents");
  const team = page.locator('[data-pane="agent-console"]');
  await team.getByRole("tab", { name: /^Info/ }).click();
  for (const name of agentNames) {
    await team.getByText(name, { exact: true }).waitFor();
  }
  await team.getByText("0 pending", { exact: true }).waitFor();
  await team.getByText("running", { exact: true }).first().waitFor();
  await team.getByText("running-stale", { exact: true }).first().waitFor();
  await page.screenshot({
    path: join(artifacts, "canvas-reference-agent-details-dark.png"),
    fullPage: false,
  });
  await team.getByRole("tab", { name: /^Compose/ }).click();
  await team.locator(".mosaic-message-card").nth(3).waitFor({ timeout: 10_000 });
  assert.equal(
    await page.evaluate(() => matchMedia("(prefers-color-scheme: dark)").matches),
    true,
    "the reference capture explicitly exercises dark mode",
  );
  await assertFourAgentDensity(team);
  const pixels = await page.evaluate(() => ({
    width: innerWidth,
    height: innerHeight,
    background: getComputedStyle(document.body).backgroundColor,
    bodyWidth: document.body.scrollWidth,
  }));
  assert.deepEqual([pixels.width, pixels.height], [1000, 568]);
  assert.equal(
    await page.locator(".mosaic-workspace > .topbar").count(),
    0,
    "the live lineage has no redundant operator control bar",
  );
  assert.equal(pixels.bodyWidth, 1000, "the reference viewport has no horizontal overflow");
  assert.equal(pixels.background, "rgb(11, 15, 12)", "the captured surface uses the dark palette");
  await page.screenshot({
    path: join(artifacts, "canvas-reference-four-agent-dark.png"),
    fullPage: false,
  });
}

async function assertAuthoritySurface(page, seeded) {
  await chooseCanvasView(page, "Flow");
  const currentWork = page.locator(".section").filter({ hasText: "Current work" });
  await currentWork.getByText("Browser fixture task", { exact: true }).waitFor();
  await currentWork.getByText("Continuation witness", { exact: true }).waitFor();

  await chooseCanvasView(page, "Magnitude");
  await page.getByRole("heading", { name: "Consent budgets", exact: true }).waitFor();
  await page.getByText("browser", { exact: true }).waitFor();
  assert.match(await page.locator(".section").filter({ hasText: "Consent budgets" }).innerText(), /2\s+2/);

  await chooseCanvasView(page, "Topology");
  const topology = page.locator("main.content");
  await topology.getByText("Resident topology", { exact: true }).waitFor();
  await topology.getByText("claude", { exact: true }).first().waitFor();
  assert.match(await topology.innerText(), new RegExp(seeded.lane.slice(0, 8)));

  await chooseCanvasView(page, "Chronology");
  await page.getByText("browser direction survives streaming", { exact: true }).waitFor();
  const trust = page.locator('[data-pane="trust"]');
  await trust.waitFor();
  const trustText = await trust.innerText();
  assert.match(trustText, /trust::pended/);
  assert.match(trustText, /trust::granted/);
  assert.match(trustText, /granted/);
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

  const nestedCard = quoteActivity
    .filter({ hasText: nestedQuotePreview })
    .getByRole("button", { name: "Quote this message" });
  await nestedCard.click();
  let previewBand = quoteLane
    .locator(".composer-quote-band")
    .filter({ hasText: nestedQuotePreview });
  await previewBand.waitFor();
  let previewLabel = await previewBand.locator("header span").innerText();
  assert.equal(previewLabel, nestedQuotePreview);
  assert.doesNotMatch(previewLabel, /[<>]/, "the producer preview contains no quote marker");
  assert.equal(
    await previewBand.getAttribute("title"),
    nestedQuoteMessage,
    "the complete nested message remains reachable from the band",
  );
  await nestedCard.click();
  assert.equal(
    await quoteLane.locator(".composer-quote-band").count(),
    1,
    "quoting the same message identity reveals its existing band",
  );
  const repeatedQuoteId = await previewBand.getByRole("textbox").getAttribute("id");
  await page.waitForFunction(
    (id) => document.activeElement?.id === id,
    repeatedQuoteId,
  );
  assert.equal(
    await previewBand
      .getByRole("textbox")
      .evaluate((textarea) => document.activeElement === textarea),
    true,
    "repeated quote identity focuses the existing reply",
  );

  await quoteActivity
    .filter({ hasText: "Borrowed context has no authored preview." })
    .getByRole("button", { name: "Quote this message" })
    .click();
  assert.equal(
    await quoteLane.locator(".composer-quote-band").count(),
    2,
    "two distinct messages retain two simultaneous quote bands",
  );
  previewBand = quoteLane.locator(".composer-quote-band").first();
  await previewBand.waitFor();
  previewLabel = await previewBand.locator("header span").innerText();
  assert.equal(previewLabel, "quoted message", "an absent preview has a visible placeholder");
  assert.ok(previewLabel.trim(), "the absent-preview band is not blank");
  assert.doesNotMatch(previewLabel, /[<>]/);
  assert.equal(await previewBand.getAttribute("title"), entirelyQuotedMessage);

  await activity
    .filter({ hasText: "browser direction survives streaming" })
    .getByRole("button", { name: "Quote this message" })
    .click();
  const composer = lane.locator(".mosaic-composer");
  const quotedComposer = lane.locator(".composer-quote-band");
  await quotedComposer.waitFor();
  await composer.getByLabel("Markdown preview").waitFor();
  assert.equal(
    await composer.locator("blockquote").innerText(),
    "browser direction survives streaming",
  );
  await quotedComposer.getByRole("textbox").fill("canvas quote reply");
  assert.equal(await composer.getByRole("textbox").inputValue(), "");
  const pixel = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII=",
    "base64",
  );
  await composer.locator('input[type="file"]').setInputFiles([
    { name: "pixel.png", mimeType: "image/png", buffer: pixel },
    { name: "pixel-copy.png", mimeType: "image/png", buffer: pixel },
  ]);
  const attachmentControls = composer.locator(".composer-attachment-list");
  await attachmentControls.getByRole("img", { name: "pixel.png" }).waitFor();
  await attachmentControls.getByRole("img", { name: "pixel-copy.png" }).waitFor();
  assert.equal(
    await composer.getByLabel("Markdown preview").locator("img").count(),
    2,
    "draft images are appended to the generated Markdown preview",
  );
  await page.screenshot({
    path: join(artifacts, "canvas-light-mosaic-composer.png"),
    fullPage: true,
  });
  await quotedComposer.getByRole("textbox").press("Control+Enter");
  await activity.filter({ hasText: "canvas quote reply" }).waitFor({
    timeout: 10_000,
  });
  assert.equal(
    await lane
      .locator("textarea[data-primary-composer-target-id]")
      .evaluate((textarea) => document.activeElement === textarea),
    true,
    "quote keyboard submission returns focus to that target's primary composer",
  );
  assert.equal(
    await quoteLane.locator(".composer-quote-band").count(),
    2,
    "target keyboard submission leaves sibling target drafts unchanged",
  );
  assert.equal(
    await activity.filter({ hasText: "canvas quote reply" }).locator(".message-body img").count(),
    2,
    "durable image links render through the card Markdown body",
  );
  const quoted = await reply(fixture, ["inbox", seeded.lane]);
  const quotedDirective = quoted.body.lanes[0].pending.find(
    (directive) => directive.text.includes("canvas quote reply"),
  );
  assert.ok(quotedDirective, "quote reply reaches the lane inbox");
  assert.match(quotedDirective.text, /> browser direction survives streaming/);
  assert.match(quotedDirective.text, /!\[pixel\.png\]\(data:image\/png;base64,/);
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

async function assertPackedHistory(page, extent, linked = false) {
  await chooseCanvasView(page, "Chronology");
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

  await chooseCanvasView(page, "Topology");
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

  await chooseCanvasView(page, "Chronology");
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
  await chooseCanvasView(page, "Chronology");
  await page.waitForSelector(".status.offline", { timeout: 10_000 });
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
  await chooseCanvasView(page, "Chronology");
  const before = await page.locator('[data-pane="commands"] .event strong').allTextContents();
  monitor.expectNetworkInterruption(true);
  await page.evaluate(() => {
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    socket.close(4000, "fixture reconnect");
  });
  await page.waitForSelector(".status.offline", { timeout: 10_000 });
  assert.match(await page.locator(".status-copy").innerText(), /Reconnecting/);
  await chooseCanvasView(page, "Chronology");
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


await buildResident();
const fixture = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-browser-")));
const linkedRoot = await realpath(await mkdtemp(join(tmpdir(), "spice-canvas-lanes-")));
await mkdir(artifacts, { recursive: true });
const worktrees = await configureFixture(fixture, linkedRoot);

let resident;
let browser;
try {
  resident = await startResident(fixture);
  const seeded = await seedAuthority(fixture);
  browser = await chromium.launch({ headless: true });
  await assertLineageLifeIsolation(browser);

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
  Object.assign(seeded, await assertAgentConversation(lightPage, fixture, worktrees));
  const reference = await browser.newContext({
    colorScheme: "dark",
    viewport: { width: 1000, height: 568 },
  });
  await installBrowserSpeech(reference);
  const referencePage = await reference.newPage();
  const referenceMonitor = monitorPage(referencePage);
  await referencePage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await captureReferenceViewport(referencePage);
  referenceMonitor.assertClean();
  await reference.close();
  seeded.extent = await journalExtent(fixture);
  await assertAuthoritySurface(lightPage, seeded);
  await assertMosaicComposer(lightPage, fixture, seeded);
  await assertPackedHistory(lightPage, await journalExtent(fixture), true);
  await assertNoCredentialOrLandingGap(lightPage, [resident.discovery.capability]);
  await assertSystemSpeech(lightPage);
  await chooseCanvasView(lightPage, "Flow");
  await lightPage.getByLabel("Task").fill(seeded.task);
  await lightPage.getByLabel("Observation").fill("browser mutation reached the resident");
  await lightPage.getByRole("button", { name: "Add note" }).click();
  await chooseCanvasView(lightPage, "Chronology");
  await lightPage.getByText(/Note appended to/).waitFor({ timeout: 10_000 });
  await chooseCanvasView(lightPage, "Flow");
  await lightPage.getByLabel("Task").fill("not a handle");
  await lightPage.getByLabel("Observation").fill("the callback must explain this refusal");
  const addNote = lightPage.getByRole("button", { name: "Add note" });
  assert.equal(await addNote.isEnabled(), true, "nonempty malformed input reaches the callback");
  await addNote.click();
  const noteRefusal = lightPage.getByRole("alert");
  await noteRefusal.waitFor();
  assert.match(await noteRefusal.innerText(), /Task must be .*both parts alphanumeric/);
  assert.match(await noteRefusal.innerText(), /"not a handle"/);
  const existingTasks = await lightPage.locator("#note-tasks option").evaluateAll(
    (options) => options.map((option) => option.value),
  );
  const absentTask = ["SPICE-aBcD", "SPICE-zYxW"].find(
    (candidate) => !existingTasks.includes(candidate),
  );
  assert.ok(absentTask, "an absent canonical task handle is available");
  await lightPage.getByLabel("Task").fill(absentTask);
  await lightPage.getByLabel("Observation").fill("this task does not exist");
  await addNote.click();
  await noteRefusal.waitFor({ state: "detached" });
  await chooseCanvasView(lightPage, "Chronology");
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
  const configuredPolicy = await assertPerTeamWatchbands(lightPage, fixture, worktrees);
  lightMonitor.assertClean();
  await dark.close();

  const liveTeam = await reply(fixture, ["lane"]);
  for (const agent of liveTeam.body.lanes) {
    await reply(fixture, ["lane", "drain", agent.lane]);
  }
  const firstEpoch = resident.discovery.epoch;
  const firstCapability = resident.discovery.capability;
  lightMonitor.expectNetworkInterruption(true);
  await chooseCanvasView(lightPage, "Chronology");
  await stopResident(fixture, resident);
  await lightPage.waitForSelector(".status.offline", { timeout: 10_000 });
  await lightPage.locator(".event.fault").waitFor({ timeout: 10_000 });
  resident = await startResident(fixture);
  assert.notEqual(resident.discovery.epoch, firstEpoch, "restart publishes a new generation");
  assert.notEqual(
    resident.discovery.capability,
    firstCapability,
    "restart publishes a new operator capability",
  );
  const restartedLanes = await reply(fixture, ["lane"]);
  assert.equal(
    restartedLanes.body.subscriptions.find(
      (subscription) => subscription.lineage_id === configuredPolicy.lineage,
    )?.policy,
    configuredPolicy.policy,
    "restart preserves the configured task policy instead of deriving it from lane life",
  );
  await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await chooseCanvasView(lightPage, "Chronology");
  await lightPage.waitForSelector(".status.online", { timeout: 10_000 });
  await lightPage.waitForFunction(
    (epoch) => document.querySelector(".status-copy")?.textContent?.includes(epoch),
    resident.discovery.epoch,
  );
  lightMonitor.expectNetworkInterruption(false);
  await assertCanvas(lightPage, "light-restarted");
  await chooseCanvasView(lightPage, "Chronology");
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
  await rm(linkedRoot, { recursive: true, force: true });
}
if (coverageReceiptPath) await writeFile(coverageReceiptPath, coverageReceipt);
