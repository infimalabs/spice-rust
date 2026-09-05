import assert from "node:assert/strict";
import { join } from "node:path";

import {
  chooseCanvasView,
  reply,
  TRANSITION_MS,
  waitForPageFunction,
} from "./canvas-fixture.mjs";

async function openSpiceMenu(page) {
  const menu = page.locator(".canvas-menu");
  if ((await menu.getAttribute("open")) === null) {
    await menu.locator(":scope > summary").click();
  }
  const panel = menu.locator(".canvas-menu-panel");
  await panel.waitFor({ timeout: TRANSITION_MS });
  return panel;
}

async function addOverflowControls(panel) {
  await panel.evaluate((node) => {
    const fixture = document.createElement("section");
    fixture.className = "canvas-menu-section";
    fixture.dataset.menuOverflowFixture = "true";
    for (let index = 1; index <= 14; index += 1) {
      const control = document.createElement("button");
      control.className = "axis-button";
      control.type = "button";
      control.textContent = `Overflow control ${index}`;
      fixture.append(control);
    }
    node.querySelector(".canvas-menu-page").append(fixture);
  });
}

async function assertOverflowReachability(panel, label) {
  const first = panel.locator("button").first();
  const last = panel.locator('[data-menu-overflow-fixture] > button').last();
  const scrollGeometry = await panel.evaluate((node) => ({
    clientHeight: node.clientHeight,
    overflowY: getComputedStyle(node).overflowY,
    scrollHeight: node.scrollHeight,
  }));
  assert.equal(scrollGeometry.overflowY, "auto", `${label} keeps menu overflow internal`);
  assert.ok(
    scrollGeometry.scrollHeight > scrollGeometry.clientHeight,
    `${label} has real overflow content to scroll`,
  );

  await panel.evaluate((node) => { node.scrollTop = 0; });
  const panelTop = await panel.boundingBox();
  const firstBox = await first.boundingBox();
  assert.ok(
    panelTop && firstBox
      && firstBox.y >= panelTop.y - 0.5
      && firstBox.y + firstBox.height <= panelTop.y + panelTop.height + 0.5,
    `${label} keeps the first menu control reachable`,
  );

  await panel.evaluate((node) => { node.scrollTop = node.scrollHeight; });
  const panelBottom = await panel.boundingBox();
  const lastBox = await last.boundingBox();
  assert.ok(
    panelBottom && lastBox
      && lastBox.y + lastBox.height <= panelBottom.y + panelBottom.height + 0.5,
    `${label} keeps the last overflow control reachable`,
  );
  await panel.evaluate((node) => { node.scrollTop = 0; });
}

async function assertTopChromeSpacing(page, artifacts) {
  const previous = page.viewportSize();
  assert.ok(previous, "top chrome has a viewport to restore");
  const viewports = [
    ["desktop", previous],
    ["332px narrow", { width: 332, height: 720 }],
  ];
  try {
    for (const [label, viewport] of viewports) {
      await page.setViewportSize(viewport);
      const bar = page.locator(".commandbar--mosaic");
      const geometry = await bar.evaluate((node) => {
        const box = (element) => {
          const rect = element.getBoundingClientRect();
          return {
            left: rect.left,
            right: rect.right,
            top: rect.top,
            bottom: rect.bottom,
          };
        };
        const strip = node.querySelector(".task-pill-strip");
        const summary = node.querySelector(".canvas-menu > summary");
        const stamp = summary.querySelector(".canvas-menu-flame");
        const pill = strip.querySelector(".task-pill");
        const style = getComputedStyle(node);
        const summaryStyle = getComputedStyle(summary);
        return {
          viewportWidth: innerWidth,
          bar: box(node),
          strip: box(strip),
          summary: box(summary),
          stamp: box(stamp),
          summaryPaddingLeft: summaryStyle.paddingLeft,
          summaryPaddingRight: summaryStyle.paddingRight,
          accessibleName: summary.getAttribute("aria-label"),
          hasTextLabel: Boolean(summary.querySelector(".canvas-menu-label")),
          paddingTop: style.paddingTop,
          paddingBottom: style.paddingBottom,
          paddingLeft: style.paddingLeft,
          paddingRight: style.paddingRight,
          gap: style.gap,
          stripGap: getComputedStyle(strip).gap,
          pillPaddingTop: getComputedStyle(pill).paddingTop,
          pillPaddingLeft: getComputedStyle(pill).paddingLeft,
          stripClientWidth: strip.clientWidth,
          stripScrollWidth: strip.scrollWidth,
        };
      });
      assert.equal(geometry.paddingTop, "8px", `${label} gives top-level pills 8px above`);
      assert.equal(geometry.paddingBottom, "8px", `${label} gives top-level pills 8px below`);
      assert.equal(geometry.gap, "7px", `${label} preserves the top-level control gap`);
      assert.equal(geometry.stripGap, "6px", `${label} preserves the task-pill gap`);
      assert.equal(geometry.pillPaddingTop, "2px", `${label} preserves task-pill block padding`);
      assert.equal(geometry.pillPaddingLeft, "10px", `${label} preserves task-pill inline padding`);
      assert.ok(
        geometry.stripClientWidth >= 64 && geometry.stripScrollWidth >= geometry.stripClientWidth,
        `${label} gives the horizontally scrollable task strip a visible viewport`,
      );
      assert.ok(
        geometry.strip.right <= geometry.summary.left + 1,
        `${label} keeps task pills clear of the Spice menu`,
      );
      assert.equal(geometry.accessibleName, "Open Spice Canvas menu");
      assert.equal(geometry.hasTextLabel, false, `${label} keeps the control wordless`);
      assert.deepEqual(
        [geometry.summary.right - geometry.summary.left, geometry.summary.bottom - geometry.summary.top],
        [43, 30],
        `${label} preserves the predecessor outer dimensions`,
      );
      assert.deepEqual(
        [geometry.stamp.right - geometry.stamp.left, geometry.stamp.bottom - geometry.stamp.top],
        [31, 25],
        `${label} preserves the predecessor cut-out stamp`,
      );
      assert.deepEqual([geometry.summaryPaddingLeft, geometry.summaryPaddingRight], ["2px", "8px"]);
      const trigger = page.locator(".canvas-menu > summary");
      const stableTriggerBox = async (state) => {
        const box = await trigger.boundingBox();
        assert.ok(box, `${label} ${state} trigger remains visible`);
        assert.deepEqual([box.width, box.height], [43, 30], `${label} ${state} keeps cut-out geometry`);
      };
      await trigger.hover();
      await stableTriggerBox("hover");
      await trigger.focus();
      assert.ok(await trigger.evaluate((node) => node === document.activeElement));
      await stableTriggerBox("focus");
      await page.evaluate(() => { document.documentElement.style.fontSize = "24px"; });
      await stableTriggerBox("zoom");
      await page.evaluate(() => { document.documentElement.style.fontSize = ""; });
      await page.mouse.move(0, 0);
      await trigger.evaluate((node) => node.blur());
      if (label === "desktop") {
        await trigger.screenshot({ path: join(artifacts, "canvas-fast-mode-inactive.png") });
      }
      assert.ok(
        geometry.bar.left >= -0.5 && geometry.bar.right <= geometry.viewportWidth + 0.5,
        `${label} keeps the top chrome inside the viewport`,
      );
      assert.equal(
        geometry.paddingLeft,
        viewport.width <= 720 ? "8px" : "10px",
        `${label} keeps its responsive inline edge`,
      );
      assert.equal(geometry.paddingRight, geometry.paddingLeft, `${label} has symmetric inline edges`);

      const strip = page.locator(".task-pill-strip");
      await strip.evaluate((node) => { node.scrollLeft = 0; });
      const first = await strip.locator(".task-pill").first().boundingBox();
      const stripStart = await strip.boundingBox();
      assert.ok(first && stripStart && first.x >= stripStart.x - 0.5, `${label} shows the first task pill`);
      await strip.evaluate((node) => { node.scrollLeft = node.scrollWidth; });
      const last = await strip.locator(".task-pill").last().boundingBox();
      const stripEnd = await strip.boundingBox();
      assert.ok(
        last && stripEnd && last.x + last.width <= stripEnd.x + stripEnd.width + 0.5,
        `${label} can reveal the complete last task pill`,
      );
      await strip.evaluate((node) => { node.scrollLeft = 0; });

      const panel = await openSpiceMenu(page);
      const navigation = page.locator(".canvas-navigation");
      assert.deepEqual(
        await navigation.locator(".pill-label").allTextContents(),
        ["Tasks", "Metrics", "History", "Teams", "Agents"],
        `${label} exposes one ordered navigation rail`,
      );
      assert.equal(
        await page.locator(".task-pill-strip").count(),
        0,
        `${label} replaces task pills while navigation is open`,
      );
      const expandedGeometry = await page.locator(".commandbar--mosaic").evaluate((node) => {
        const navigation = node.querySelector(".canvas-navigation").getBoundingClientRect();
        const summary = node.querySelector(".canvas-menu > summary").getBoundingClientRect();
        return {
          navigationRight: navigation.right,
          summaryLeft: summary.left,
          summaryWidth: summary.width,
          summaryHeight: summary.height,
        };
      });
      assert.ok(
        expandedGeometry.navigationRight <= expandedGeometry.summaryLeft + 1,
        `${label} keeps Agents adjacent to and clear of the pepper trigger`,
      );
      assert.equal(expandedGeometry.summaryWidth, 43, `${label} keeps the cut-out trigger 43px wide`);
      assert.equal(expandedGeometry.summaryHeight, 30, `${label} keeps the pepper trigger 30px high`);
      if (viewport.width <= 560) {
        await addOverflowControls(panel);
      }
      const panelGeometry = await panel.evaluate((node) => {
        const rect = node.getBoundingClientRect();
        const summary = node.parentElement.querySelector(":scope > summary").getBoundingClientRect();
        return {
          left: rect.left,
          right: rect.right,
          top: rect.top,
          bottom: rect.bottom,
          summaryBottom: summary.bottom,
          viewportWidth: innerWidth,
          viewportHeight: innerHeight,
        };
      });
      assert.ok(
        panelGeometry.left >= -0.5 && panelGeometry.right <= panelGeometry.viewportWidth + 0.5,
        `${label} keeps the Spice menu inside the horizontal viewport`,
      );
      assert.ok(
        panelGeometry.top >= panelGeometry.summaryBottom + 3.5
          && panelGeometry.bottom <= panelGeometry.viewportHeight + 0.5,
        `${label} anchors and bounds the Spice menu below its summary`,
      );
      if (viewport.width <= 560) {
        await assertOverflowReachability(panel, label);
      }
      await page.screenshot({
        path: join(
          artifacts,
          viewport.width <= 560
            ? `canvas-menu-overflow-${viewport.width}x${viewport.height}.png`
            : `canvas-menu-desktop-${viewport.width}x${viewport.height}.png`,
        ),
        fullPage: false,
      });
      await panel.locator('[data-menu-overflow-fixture]').evaluateAll((fixtures) => {
        for (const fixture of fixtures) fixture.remove();
      });
      await page.locator(".canvas-menu > summary").click();
    }
  } finally {
    await page.setViewportSize(previous);
  }
}

async function waitForDisclosure(page, expanded, reason) {
  await waitForPageFunction(
    page,
    reason,
    TRANSITION_MS,
    (expected) => {
      const menu = document.querySelector(".canvas-menu");
      const trigger = menu?.querySelector(":scope > summary");
      return trigger?.getAttribute("aria-expanded") === String(expected)
        && menu.hasAttribute("open") === expected
        && Boolean(document.querySelector(".canvas-navigation")) === expected
        && Boolean(document.querySelector(".task-pill-strip")) === !expected;
    },
    expanded,
  );
}

async function assertPepperDisclosure(page) {
  const menu = page.locator(".canvas-menu");
  const trigger = menu.locator(":scope > summary");
  const navigation = page.getByRole("navigation", { name: "Canvas views" });
  const taskStrip = page.getByRole("navigation", { name: "Open task projects" });

  assert.equal(await trigger.getAttribute("aria-haspopup"), "menu");
  await waitForDisclosure(page, false, "the pepper disclosure begins closed");

  const previous = page.viewportSize();
  assert.ok(previous, "the disclosure check has a viewport to restore");
  try {
    await page.setViewportSize({ width: 332, height: 720 });
    const maximumScroll = await taskStrip.evaluate((node) => node.scrollWidth - node.clientWidth);
    assert.ok(maximumScroll > 0, "the narrow task strip has a real horizontal offset to reset");
    await taskStrip.evaluate((node) => { node.scrollLeft = node.scrollWidth; });
    assert.ok(await taskStrip.evaluate((node) => node.scrollLeft > 0), "the task strip scrolls away from its start");

    await trigger.click();
    await waitForDisclosure(page, true, "the pepper disclosure exposes Canvas destinations");
    await page.locator(".mosaic-workspace").click({ position: { x: 2, y: 2 } });
    await waitForDisclosure(page, false, "an outside pointer dismisses the pepper disclosure");
    assert.equal(await taskStrip.evaluate((node) => node.scrollLeft), 0, "the remounted task strip starts at its first pill");
    const firstPill = await taskStrip.locator(".task-pill").first().boundingBox();
    const stripStart = await taskStrip.boundingBox();
    assert.ok(
      firstPill && stripStart && firstPill.x >= stripStart.x - 0.5,
      "the remounted task strip exposes its first pill",
    );
  } finally {
    await page.setViewportSize(previous);
  }

  await trigger.click();
  await waitForDisclosure(page, true, "the pepper disclosure reopens for focus dismissal");
  const outsideFocus = page.locator('.mosaic-workspace[data-active="true"] textarea').first();
  await outsideFocus.focus();
  await waitForDisclosure(page, false, "focus outside the pepper disclosure dismisses it");
  assert.ok(
    await outsideFocus.evaluate((node) => node === document.activeElement),
    "outside focus remains on its requested control",
  );

  await trigger.click();
  await waitForDisclosure(page, true, "the pepper disclosure reopens for Escape dismissal");
  await navigation.getByRole("button", { name: /^Tasks/ }).focus();
  await page.keyboard.press("Escape");
  await waitForDisclosure(page, false, "Escape dismisses the pepper disclosure");
  assert.ok(
    await trigger.evaluate((node) => node === document.activeElement),
    "Escape restores focus to the pepper trigger",
  );

  await trigger.click();
  await waitForDisclosure(page, true, "the pepper disclosure reopens for destination selection");
  await navigation.getByRole("button", { name: /^Tasks/ }).click();
  await page.getByRole("heading", { name: "Tasks", exact: true }).waitFor({ timeout: TRANSITION_MS });
  await waitForDisclosure(page, false, "a Canvas destination closes the pepper disclosure");
  const panel = await openSpiceMenu(page);
  assert.equal(
    await panel.locator(".canvas-menu-page").getAttribute("data-menu-page"),
    "flow",
    "the reopened pepper panel belongs to the selected destination",
  );
  await panel.getByRole("heading", { name: "Tasks", exact: true }).waitFor();
  await chooseCanvasView(page, "Agents");
  await waitForDisclosure(page, false, "the disclosure check restores the Agents workspace");
}

async function assertAxisMenu(page, view, copy) {
  const visibleView = {
    Flow: "Tasks",
    Magnitude: "Metrics",
    Chronology: "History",
    Topology: "Teams",
  }[view] ?? view;
  await chooseCanvasView(page, view);
  const panel = await openSpiceMenu(page);
  const menuPage = panel.locator(".canvas-menu-page");
  assert.equal(
    await menuPage.getAttribute("data-menu-page"),
    view.toLowerCase(),
    `${view} opens its own Spice panel rather than the Agents panel`,
  );
  assert.equal(await panel.locator(".canvas-team").count(), 0, `${view} renders no team manager`);
  await panel.getByRole("heading", { name: visibleView, exact: true }).waitFor();
  assert.match(await menuPage.innerText(), copy, `${view} describes its own page context`);
}

async function waitForMemberIn(scope, lane) {
  const member = scope.locator(`.canvas-team-member[data-team-member="${lane}"]`);
  await member.waitFor({ timeout: TRANSITION_MS });
  return member;
}

async function assertMemberAccents(scope, expected, message) {
  assert.deepEqual(
    await scope.locator(":scope > .canvas-team-members > .canvas-team-member")
      .evaluateAll((members) => members.map((member) => member.dataset.accent)),
    expected,
    message,
  );
}

async function assertResidentLineage(fixture, lane, lineage, policyLineage, reason) {
  const answer = await reply(fixture, ["lane"]);
  const member = answer.body.lanes.find((row) => row.lane === lane);
  assert.ok(member, `${reason}: moved member remains in the authoritative lane answer`);
  assert.equal(member.lineage_id, lineage, `${reason}: resident committed the target lineage`);
  const subscription = answer.body.subscriptions.find(
    (candidate) => candidate.lineage_id === policyLineage,
  );
  assert.equal(
    subscription?.policy,
    "drain",
    `${reason}: membership change preserves the independently selected lifetime`,
  );
}

/** Exercise the Agents-page Spice panel, shared navigation, and durable team moves. */
export async function assertTeamMenu(page, fixture, artifacts) {
  await chooseCanvasView(page, "Agents");
  await assertTopChromeSpacing(page, artifacts);
  await assertPepperDisclosure(page);
  await chooseCanvasView(page, "Topology");
  await page.getByRole("heading", { name: "Teams", exact: true }).waitFor();
  await openSpiceMenu(page);
  assert.equal(
    await page.getByRole("navigation", { name: "Canvas views" })
      .getByRole("button", { name: /^Agents/ }).count(),
    1,
    "the analytics pages keep the same route back to live Agents",
  );
  await assertAxisMenu(page, "Topology", /resident teams, seats, capabilities/i);
  await assertAxisMenu(page, "Chronology", /facts, command outcomes, and their causal thread/i);
  await chooseCanvasView(page, "Agents");

  let panel = await openSpiceMenu(page);
  assert.equal(await panel.locator(".canvas-menu-page").getAttribute("data-menu-page"), "agents");
  await panel.getByRole("heading", { name: "Global", exact: true }).waitFor();
  const fastMode = panel.getByRole("menuitemcheckbox", { name: /Fast mode/i });
  const trigger = page.locator(".canvas-menu > summary");
  const offAppearance = await trigger.evaluate((node) => {
    const style = getComputedStyle(node);
    return [style.backgroundColor, style.borderColor, node.getBoundingClientRect().width];
  });
  assert.equal(await fastMode.getAttribute("aria-checked"), "false");
  assert.match(await fastMode.innerText(), /Fast mode\s+off/);

  const browser = page.context().browser();
  assert.ok(browser, "the fast-mode peer has a browser to own its context");
  const peerContext = await browser.newContext();
  const peer = await peerContext.newPage();
  try {
    await peer.goto(page.url(), { waitUntil: "networkidle" });
    const peerPanel = await openSpiceMenu(peer);
    let peerFastMode = peerPanel.getByRole("menuitemcheckbox", { name: /Fast mode/i });
    await peerFastMode.waitFor({ timeout: TRANSITION_MS });
    assert.equal(await peerFastMode.getAttribute("aria-checked"), "false");

    await fastMode.click();
    await waitForPageFunction(
      page,
      "initiating Canvas applies acknowledged fast mode",
      TRANSITION_MS,
      () => document
        .querySelector('[role="menuitemcheckbox"]')
        ?.getAttribute("aria-checked") === "true",
    );
    await waitForPageFunction(
      peer,
      "peer Canvas receives the committed settings fact",
      TRANSITION_MS,
      () => document
        .querySelector('[role="menuitemcheckbox"]')
        ?.getAttribute("aria-checked") === "true",
    );
    assert.match(await trigger.getAttribute("title"), /fast mode on/i);
    assert.ok(await trigger.evaluate((node) => node.classList.contains("canvas-menu-trigger--fast")));
    const onAppearance = await trigger.evaluate((node) => {
      const style = getComputedStyle(node);
      return [style.backgroundColor, style.borderColor, node.getBoundingClientRect().width];
    });
    assert.notDeepEqual(onAppearance.slice(0, 2), offAppearance.slice(0, 2));
    assert.equal(onAppearance[2], offAppearance[2], "mode color changes without shifting the cut-out");
    await trigger.screenshot({ path: join(artifacts, "canvas-fast-mode-active-control.png") });
    await page.emulateMedia({ reducedMotion: "reduce" });
    assert.equal(
      await page.locator(".canvas-menu-flame").evaluate(
        (node) => getComputedStyle(node, "::before").animationName,
      ),
      "none",
      "reduced motion leaves the flame visible without animation",
    );
    assert.notEqual(
      await page.locator(".canvas-menu-flame").evaluate(
        (node) => getComputedStyle(node, "::before").opacity,
      ),
      "0",
      "reduced motion preserves the fast-mode status treatment",
    );
    await page.screenshot({
      path: join(artifacts, "canvas-fast-mode-active.png"),
      fullPage: false,
    });
    await page.emulateMedia({ reducedMotion: "no-preference" });

    await peer.reload({ waitUntil: "networkidle" });
    peerFastMode = (await openSpiceMenu(peer))
      .getByRole("menuitemcheckbox", { name: /Fast mode/i });
    await peerFastMode.waitFor({ timeout: TRANSITION_MS });
    assert.equal(
      await peerFastMode.getAttribute("aria-checked"),
      "true",
      "reload reads the durable setting rather than browser storage",
    );
    await peerFastMode.focus();
    await peerFastMode.press("Space");
    await waitForPageFunction(
      page,
      "keyboard-authored setting converges back to the first Canvas",
      TRANSITION_MS,
      () => document
        .querySelector('[role="menuitemcheckbox"]')
        ?.getAttribute("aria-checked") === "false",
    );
  } finally {
    await peerContext.close();
  }
  assert.equal(await panel.locator(".canvas-team").count(), 1, "the four agents begin together");
  assert.equal(await panel.locator(".canvas-team-member").count(), 4);
  assert.equal(
    await panel.locator('[data-team-drop="new"]').innerText(),
    "DROP AGENT HERE",
  );
  assert.match(
    await panel.locator(".canvas-unteamed").innerText(),
    /Agents Without Team[\s\S]*No unteamed agents/i,
  );

  const initial = await reply(fixture, ["lane"]);
  const host = initial.body.lanes[0].lineage_id;
  const moved = initial.body.lanes.at(-1).lane;
  const surface = page.locator(`.team-surface[data-lineage-id="${host}"]`);
  await surface.locator('.team-lifetime input[type="range"]').fill("2");
  await waitForPageFunction(
    page,
    "team lifetime command settles before membership edits",
    TRANSITION_MS,
    (lineage) => document
      .querySelector(`.team-surface[data-lineage-id="${lineage}"] .team-composer-controls`)
      ?.dataset.lifetimePending === "false",
    host,
  );
  await surface.getByRole("button", { name: "Drain", exact: true }).waitFor();
  const drained = await reply(fixture, ["lane"]);
  assert.equal(
    drained.body.subscriptions.find((subscription) => subscription.lineage_id === host)?.policy,
    "drain",
    "the team begins membership edits with a committed Drain policy",
  );
  await page.evaluate(() => {
    window.__teamMenuLineageCommands = [];
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    const send = socket.send.bind(socket);
    const record = (bytes) => {
      const command = JSON.parse(new TextDecoder().decode(bytes));
      if (command.kind === "lineage::configure") {
        window.__teamMenuLineageCommands.push(command.body);
      }
    };
    socket.send = (data) => {
      if (typeof data === "string") {
        record(new TextEncoder().encode(data));
      } else if (data instanceof Blob) {
        void data.arrayBuffer().then((buffer) => record(new Uint8Array(buffer)));
      } else if (ArrayBuffer.isView(data)) {
        record(new Uint8Array(data.buffer, data.byteOffset, data.byteLength));
      } else {
        record(new Uint8Array(data));
      }
      send(data);
    };
  });
  panel = await openSpiceMenu(page);
  let originalTeam = panel.locator(`.canvas-team[data-team-lineage="${host}"]`);
  await assertMemberAccents(
    originalTeam,
    ["3", "2", "1", "0"],
    "the team menu deals accents from the right control edge",
  );
  let member = await waitForMemberIn(originalTeam, moved);
  await member.dragTo(panel.locator('[data-team-drop="unteamed"]'));

  panel = await openSpiceMenu(page);
  member = await waitForMemberIn(panel.locator(".canvas-unteamed"), moved);
  assert.equal(await member.getAttribute("data-accent"), "0", "a solo member starts at green");
  assert.equal(await panel.locator(".canvas-team").count(), 1);
  assert.equal(await panel.locator(".canvas-team").locator(".canvas-team-member").count(), 3);
  await assertMemberAccents(
    panel.locator(`.canvas-team[data-team-lineage="${host}"]`),
    ["2", "1", "0"],
    "removing a member keeps green anchored to the surviving right edge",
  );
  await assertResidentLineage(fixture, moved, moved, host, "removing an agent from its team");

  originalTeam = panel.locator(`.canvas-team[data-team-lineage="${host}"]`);
  await member.dragTo(originalTeam);
  panel = await openSpiceMenu(page);
  originalTeam = panel.locator(`.canvas-team[data-team-lineage="${host}"]`);
  member = await waitForMemberIn(originalTeam, moved);
  assert.equal(await originalTeam.locator(".canvas-team-member").count(), 4);
  await assertMemberAccents(
    originalTeam,
    ["3", "2", "1", "0"],
    "returning the member redeals the sequence from the right edge",
  );
  await assertResidentLineage(fixture, moved, host, host, "returning an agent to its team");

  await member.dragTo(panel.locator('[data-team-drop="new"]'));
  panel = await openSpiceMenu(page);
  const createdTeam = panel.locator(
    `.canvas-team:not([data-team-lineage="${host}"])`
      + `:has(.canvas-team-member[data-team-member="${moved}"])`,
  );
  await createdTeam.waitFor({ timeout: TRANSITION_MS });
  const created = await createdTeam.getAttribute("data-team-lineage");
  assert.notEqual(created, host, "the New Team target mints a distinct lineage identity");
  assert.notEqual(created, moved, "a new one-agent team stays distinct from an unteamed agent");
  assert.equal(await panel.locator(".canvas-team").count(), 2);
  await assertResidentLineage(fixture, moved, created, host, "creating a team by drop");

  member = await waitForMemberIn(createdTeam, moved);
  originalTeam = panel.locator(`.canvas-team[data-team-lineage="${host}"]`);
  await member.dragTo(originalTeam);
  panel = await openSpiceMenu(page);
  originalTeam = panel.locator(`.canvas-team[data-team-lineage="${host}"]`);
  await waitForMemberIn(originalTeam, moved);
  assert.equal(await panel.locator(".canvas-team").count(), 1, "the empty created team disappears");
  assert.equal(await originalTeam.locator(".canvas-team-member").count(), 4);
  await assertResidentLineage(fixture, moved, host, host, "restoring the original four-agent team");
  await waitForPageFunction(
    page,
    "membership commands cross the browser socket",
    TRANSITION_MS,
    () => window.__teamMenuLineageCommands?.length >= 4,
    undefined,
  );
  const membershipCommands = await page.evaluate(() => window.__teamMenuLineageCommands);
  assert.equal(membershipCommands.length, 4, "each drag emits one lineage membership command");
  for (const body of membershipCommands) {
    assert.ok(Object.hasOwn(body, "member"), "membership command identifies the moved agent");
    assert.equal(
      Object.hasOwn(body, "life"),
      false,
      "membership command omits unrelated lifetime state",
    );
  }
  await surface.locator('.team-lifetime input[type="range"]').fill("0");
  await waitForPageFunction(
    page,
    "team lifetime returns to the scenario baseline",
    TRANSITION_MS,
    (lineage) => document
      .querySelector(`.team-surface[data-lineage-id="${lineage}"] .team-composer-controls`)
      ?.dataset.lifetimePending === "false",
    host,
  );
  await surface.getByRole("button", { name: "Steer", exact: true }).waitFor();
  const restored = await reply(fixture, ["lane"]);
  assert.equal(
    restored.body.subscriptions.find((subscription) => subscription.lineage_id === host)?.policy,
    "steer",
    "the team-menu scenario restores its incoming lifetime",
  );
}
