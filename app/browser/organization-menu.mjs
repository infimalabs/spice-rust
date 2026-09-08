import assert from "node:assert/strict";
import { join } from "node:path";

import {
  chooseCanvasView,
  reply,
  TRANSITION_MS,
  waitForPageFunction,
} from "./canvas-fixture.mjs";
import { assertGlobalNavigation } from "./organization-navigation.mjs";

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

export async function captureCursorFailureArtifact(page, path, error) {
  await page.screenshot({ path, fullPage: false });
  throw error;
}

export async function assertCursorRelocation(page, artifacts, engine) {
  const previous = page.viewportSize();
  assert.ok(previous, "top chrome has a viewport to restore");
  const viewports = [
    ["desktop", previous],
    ["332px narrow", { width: 332, height: 720 }],
  ];
  try {
    for (const [viewportLabel, viewport] of viewports) {
      const label = `${engine} ${viewportLabel}`;
      await page.setViewportSize(viewport);
      await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(
        () => requestAnimationFrame(resolve),
      )));
      const bar = page.locator(".commandbar--mosaic");
      const disclosure = page.locator(".canvas-disclosure");
      const strip = page.locator(".task-pill-strip");
      const cursor = page.locator(".canvas-menu-cursor");
      const disclosureIdentity = await disclosure.evaluate((node) => {
        node.dataset.menuIdentity = crypto.randomUUID();
        return node.dataset.menuIdentity;
      });
      const stripIdentity = await strip.evaluate((node) => {
        node.dataset.menuIdentity = crypto.randomUUID();
        return node.dataset.menuIdentity;
      });
      const laneGeometry = () => page.locator(".organization-surface").evaluateAll((lanes) => (
        lanes.map((lane) => {
          const box = lane.getBoundingClientRect();
          return {
            height: box.height,
            left: box.left,
            right: box.right,
            scrollLeft: lane.closest(".lineage-fleet")?.scrollLeft ?? 0,
            scrollTop: lane.querySelector(".lane-mosaic-plane")?.scrollTop ?? 0,
            top: box.top,
          };
        })
      ));
      const collapsedLanes = await laneGeometry();
      assert.equal(await cursor.count(), 1, `${label} renders one global cursor while closed`);
      const cursorIdentity = await cursor.evaluate((node) => {
        node.dataset.relocationIdentity = crypto.randomUUID();
        return node.dataset.relocationIdentity;
      });
      assert.equal(await cursor.isVisible(), false, `${label} hides the global cursor with the closed menu`);
      assert.equal(await page.locator(".brand-dots").count(), 0, `${label} omits the agent-light roster`);
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
        const workspaceContent = node.nextElementSibling.querySelector(":scope > .content");
        const style = getComputedStyle(node);
        const summaryStyle = getComputedStyle(summary);
        const workspaceStyle = getComputedStyle(workspaceContent);
        return {
          viewportWidth: innerWidth,
          bar: box(node),
          workspaceTop: node.nextElementSibling.getBoundingClientRect().top,
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
          workspacePaddingLeft: workspaceStyle.paddingLeft,
          workspacePaddingRight: workspaceStyle.paddingRight,
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
        [35, 30],
        `${label} uses the balanced trigger dimensions`,
      );
      assert.deepEqual(
        [geometry.stamp.right - geometry.stamp.left, geometry.stamp.bottom - geometry.stamp.top],
        [31, 25],
        `${label} preserves the predecessor cut-out stamp`,
      );
      assert.deepEqual([geometry.summaryPaddingLeft, geometry.summaryPaddingRight], ["0px", "0px"]);
      const trigger = page.locator(".canvas-menu > summary");
      const stableTriggerBox = async (state) => {
        const box = await trigger.boundingBox();
        assert.ok(box, `${label} ${state} trigger remains visible`);
        assert.deepEqual([box.width, box.height], [35, 30], `${label} ${state} keeps centered geometry`);
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
      if (viewportLabel === "desktop" && engine === "chromium") {
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
      await assertUniversalShelfOuterEdge(page, label);
      await assertUniversalShelfOuterEdge(page, `${label} empty task shelf`, true);
      assert.deepEqual(
        [geometry.workspacePaddingLeft, geometry.workspacePaddingRight],
        [geometry.paddingLeft, geometry.paddingRight],
        `${label} gives the header and workspace one shell-owned inline gutter`,
      );

      await strip.evaluate((node) => { node.scrollLeft = 0; });
      const first = await strip.locator(".task-pill").first().boundingBox();
      const stripStart = await strip.boundingBox();
      assert.ok(
        first && stripStart
          && first.x >= Number.parseFloat(geometry.paddingLeft) - 0.5
          && first.x >= stripStart.x - 0.5,
        `${label} shows the first task pill inside the outer gutter`,
      );
      await strip.evaluate((node) => { node.scrollLeft = node.scrollWidth; });
      const last = await strip.locator(".task-pill").last().boundingBox();
      const stripEnd = await strip.boundingBox();
      assert.ok(
        last && stripEnd
          && last.x + last.width <= geometry.viewportWidth
            - Number.parseFloat(geometry.paddingRight) + 0.5
          && last.x + last.width <= stripEnd.x + stripEnd.width + 0.5,
        `${label} can reveal the complete last task pill inside the outer gutter`,
      );
      await strip.evaluate((node) => { node.scrollLeft = 0; });

      await page.screenshot({
        path: join(artifacts, `canvas-menu-collapsed-${engine}-${viewport.width}x${viewport.height}.png`),
        fullPage: false,
      });

      const panel = await openSpiceMenu(page);
      await page.screenshot({
        path: join(artifacts, `canvas-menu-cursor-${engine}-${viewport.width}x${viewport.height}.png`),
        fullPage: false,
      });
      assert.equal(await cursor.count(), 1, `${label} keeps one global cursor while open`);
      assert.equal(await cursor.isVisible(), true, `${label} reveals the global cursor with the open menu`);
      const version = panel.locator(".canvas-menu-version");
      const authoritativeVersion = await page.evaluate(async () => {
        const bootstrap = new URL("/bootstrap", window.location.href);
        bootstrap.search = window.location.search;
        const response = await fetch(bootstrap);
        return response.headers.get("x-spice-canvas-version");
      });
      assert.ok(authoritativeVersion, `${label} bootstrap supplies the Canvas version authority`);
      assert.equal(
        await version.getAttribute("data-canvas-version"),
        authoritativeVersion,
        `${label} footer version matches the live Rust bootstrap authority`,
      );
      assert.equal(
        (await version.innerText()).trim(),
        `Canvas v${authoritativeVersion}`,
        `${label} footer renders the authoritative version`,
      );
      assert.equal(
        await cursor.getAttribute("data-relocation-identity"),
        cursorIdentity,
        `${label} preserves the global cursor node while opening`,
      );
      assert.equal(await page.locator(".brand-dots").count(), 0, `${label} keeps the agent-light roster removed`);
      const navigation = page.locator(".canvas-navigation");
      await assertGlobalNavigation(page, `${label} open Spice menu`);
      assert.deepEqual(
        await navigation.locator(".pill-label").allTextContents(),
        ["Team", "Board", "Agents"],
        `${label} exposes one ordered navigation rail`,
      );
      assert.deepEqual(
        await navigation.locator(".axis-button").evaluateAll(
          (buttons) => buttons.map((button) => button.dataset.navigationTier),
        ),
        ["primary", "primary", "primary"],
        `${label} uses one shared navigation tier`,
      );
      assert.equal(
        await page.locator(".task-pill-strip").count(),
        1,
        `${label} keeps task pills mounted while navigation is open`,
      );
      assert.equal(await disclosure.getAttribute("data-menu-identity"), disclosureIdentity);
      assert.equal(await strip.getAttribute("data-menu-identity"), stripIdentity);
      const expandedGeometry = await page.locator(".commandbar--mosaic").evaluate((node) => {
        const box = (element) => {
          const rect = element.getBoundingClientRect();
          return { bottom: rect.bottom, left: rect.left, right: rect.right, top: rect.top };
        };
        const disclosure = node.querySelector(".canvas-disclosure");
        const strip = disclosure.querySelector(".task-pill-strip");
        const menu = disclosure.querySelector(".canvas-menu");
        const frame = menu.querySelector(".canvas-menu-frame");
        const navigation = disclosure.querySelector(":scope > .canvas-navigation");
        const panel = frame.querySelector(".canvas-menu-panel");
        const summary = menu.querySelector(":scope > summary");
        const navigationStyle = getComputedStyle(navigation);
        return {
          bar: box(node),
          disclosure: box(disclosure),
          frame: box(frame),
          navigation: box(navigation),
          panel: box(panel),
          strip: box(strip),
          summary: box(summary),
          workspaceTop: node.nextElementSibling.getBoundingClientRect().top,
          frameContainsNavigation: frame.contains(navigation),
          frameContainsPanel: frame.contains(panel),
          disclosureOwnsNavigation: navigation.parentElement === disclosure,
          menuContainsFrame: menu.contains(frame),
          disclosureContainsAll: [strip, menu, summary, frame, navigation, panel]
            .every((element) => disclosure.contains(element)),
          navigationClientWidth: navigation.clientWidth,
          navigationOverflowX: navigationStyle.overflowX,
          navigationScrollWidth: navigation.scrollWidth,
          navigationButtons: Array.from(navigation.querySelectorAll(":scope > .axis-button"))
            .map((button) => ({
              ...box(button),
              clientWidth: button.clientWidth,
              scrollWidth: button.scrollWidth,
            })),
        };
      });
      assert.deepEqual(
        expandedGeometry.bar,
        geometry.bar,
        `${label} overlays the open frame without resizing the command bar`,
      );
      assert.deepEqual(
        await laneGeometry(),
        collapsedLanes,
        `${label} overlays without moving or resizing lane surfaces`,
      );
      assert.equal(
        expandedGeometry.workspaceTop,
        geometry.workspaceTop,
        `${label} overlays Canvas without pushing its workspace`,
      );
      assert.equal(expandedGeometry.disclosureContainsAll, true, `${label} has one disclosure owner`);
      assert.equal(expandedGeometry.menuContainsFrame, true, `${label} nests the frame under the trigger details`);
      assert.equal(expandedGeometry.frameContainsNavigation, false, `${label} removes navigation from the panel frame`);
      assert.equal(
        expandedGeometry.disclosureOwnsNavigation,
        true,
        `${label} mounts navigation directly in the shared header shell`,
      );
      assert.equal(expandedGeometry.frameContainsPanel, true, `${label} keeps context controls in the frame`);
      assert.ok(
        Math.abs(expandedGeometry.frame.right - expandedGeometry.disclosure.right) <= 0.5
          && Math.abs(expandedGeometry.frame.top - expandedGeometry.workspaceTop) <= 0.5,
        `${label} anchors the menu at the organization top below the trigger row`,
      );
      assert.ok(
        Math.abs(
          expandedGeometry.frame.right - expandedGeometry.frame.left
            - Math.min(360, expandedGeometry.disclosure.right - expandedGeometry.disclosure.left),
        ) <= 0.5,
        `${label} uses one responsive mobile-width frame`,
      );
      if (viewport.width > 560) {
        assert.ok(
          expandedGeometry.frame.left > expandedGeometry.disclosure.left + 1,
          `${label} does not expand the menu across the desktop canvas`,
        );
      }
      assert.ok(
        expandedGeometry.strip.left >= expandedGeometry.disclosure.left - 0.5
          && expandedGeometry.strip.right <= expandedGeometry.summary.left + 0.5
          && expandedGeometry.summary.right <= expandedGeometry.disclosure.right + 0.5,
        `${label} contains project pills and trigger in the framed top row`,
      );
      assert.ok(
        expandedGeometry.navigation.left >= expandedGeometry.strip.left - 0.5
          && expandedGeometry.navigation.right <= expandedGeometry.summary.left + 0.5
          && expandedGeometry.navigation.top >= expandedGeometry.disclosure.top - 0.5
          && expandedGeometry.navigation.bottom <= expandedGeometry.disclosure.bottom + 0.5
          && expandedGeometry.panel.left >= expandedGeometry.frame.left - 0.5
          && expandedGeometry.panel.right <= expandedGeometry.frame.right + 0.5
          && expandedGeometry.panel.bottom <= expandedGeometry.frame.bottom + 0.5,
        `${label} contains global controls in the header and context controls in the lower frame`,
      );
      assert.ok(
        expandedGeometry.navigationScrollWidth <= expandedGeometry.navigationClientWidth + 1
          && expandedGeometry.navigationOverflowX === "auto",
        `${label} keeps the intrinsic destination strip scroll-ready without forcing overflow`,
      );
      assert.equal(
        expandedGeometry.navigationButtons.length,
        3,
        `${label} lays out every current navigation button`,
      );
      assert.ok(
        Math.abs(
          expandedGeometry.navigationButtons[0].left - expandedGeometry.navigation.left,
        ) <= 8
          && Math.abs(
            expandedGeometry.navigationButtons.at(-1).right - expandedGeometry.navigation.right,
          ) <= 0.5,
        `${label} sizes the current navigation to its intrinsic controls`,
      );
      assert.ok(
        expandedGeometry.navigationButtons.every((button) => (
          button.top >= expandedGeometry.navigation.top - 0.5
            && button.bottom <= expandedGeometry.navigation.bottom + 0.5
            && button.scrollWidth <= button.clientWidth
        )),
        `${label} keeps every intrinsic navigation button usable without label overflow`,
      );
      assert.deepEqual(
        [expandedGeometry.summary.right - expandedGeometry.summary.left,
          expandedGeometry.summary.bottom - expandedGeometry.summary.top],
        [35, 30],
        `${label} keeps the centered trigger geometry while expanded`,
      );
      assert.deepEqual(
        expandedGeometry.summary,
        geometry.summary,
        `${label} leaves the Spice trigger stationary while the menu opens`,
      );
      assert.ok(
        collapsedLanes.some((lane) => (
          expandedGeometry.frame.left < lane.right
            && expandedGeometry.frame.right > lane.left
            && expandedGeometry.frame.top <= lane.top + 0.5
            && expandedGeometry.frame.bottom > lane.top + 1
        )),
        `${label} overlays the lane surface from its top edge`,
      );
      assert.ok(
        expandedGeometry.strip.right - expandedGeometry.strip.left >= 96,
        `${label} preserves the 6rem task viewport minimum`,
      );
      const densityGeometry = await disclosure.evaluate((node) => {
        const box = (element) => {
          const rect = element.getBoundingClientRect();
          return { bottom: rect.bottom, left: rect.left, right: rect.right, top: rect.top };
        };
        const strip = node.querySelector(":scope > .task-pill-strip");
        const navigation = node.querySelector(":scope > .canvas-navigation");
        const frame = node.querySelector(".canvas-menu-frame");
        const originalPills = Array.from(strip.querySelectorAll(":scope > .task-pill"));
        const originalDisplays = originalPills.map((pill) => pill.style.display);
        for (const pill of originalPills) pill.style.display = "none";

        const sparsePill = document.createElement("button");
        sparsePill.className = "task-pill";
        sparsePill.dataset.headerDensityFixture = "sparse";
        sparsePill.textContent = "one";
        strip.append(sparsePill);
        strip.scrollLeft = 0;
        const sparse = box(sparsePill);
        const sparseNavigation = box(navigation);
        const sparseClear = sparse.right <= sparseNavigation.left + 0.5;

        const densePill = document.createElement("button");
        densePill.className = "task-pill";
        densePill.dataset.headerDensityFixture = "dense";
        densePill.textContent = "dense task pill ".repeat(18);
        strip.append(densePill);
        strip.scrollLeft = strip.scrollWidth;

        for (let index = 1; index <= 7; index += 1) {
          const destination = document.createElement("button");
          destination.className = "axis-button pill";
          destination.dataset.headerNavigationFixture = "true";
          const label = document.createElement("span");
          label.className = "pill-label";
          label.textContent = `Future global destination ${index}`;
          destination.append(label);
          navigation.append(destination);
        }
        navigation.scrollLeft = navigation.scrollWidth;

        const dense = box(densePill);
        const cappedNavigation = box(navigation);
        const frameBounds = box(frame);
        const lastDestination = box(navigation.lastElementChild);
        const hitX = Math.min(dense.right, cappedNavigation.right) - 2;
        const hitY = (Math.max(dense.top, cappedNavigation.top)
          + Math.min(dense.bottom, cappedNavigation.bottom)) / 2;
        const hit = document.elementFromPoint(hitX, hitY);
        const navigationStyle = getComputedStyle(navigation);
        const result = {
          background: navigationStyle.backgroundColor,
          borderLeftStyle: navigationStyle.borderLeftStyle,
          borderLeftWidth: navigationStyle.borderLeftWidth,
          denseOverlap: dense.left < cappedNavigation.right && dense.right > cappedNavigation.left,
          destinationOnTop: navigation.contains(hit),
          lastDestinationVisible: lastDestination.left >= cappedNavigation.left - 0.5
            && lastDestination.right <= cappedNavigation.right + 0.5,
          navigationCappedToPanel: cappedNavigation.right - cappedNavigation.left
            <= frameBounds.right - frameBounds.left + 0.5,
          navigationClientWidth: navigation.clientWidth,
          navigationScrollWidth: navigation.scrollWidth,
          sparseClear,
          stripClientWidth: strip.clientWidth,
          stripScrollWidth: strip.scrollWidth,
        };

        sparsePill.remove();
        densePill.remove();
        for (const destination of navigation.querySelectorAll("[data-header-navigation-fixture]")) {
          destination.remove();
        }
        originalPills.forEach((pill, index) => { pill.style.display = originalDisplays[index]; });
        strip.scrollLeft = 0;
        navigation.scrollLeft = 0;
        return result;
      });
      assert.equal(densityGeometry.sparseClear, true, `${label} leaves sparse task pills unobscured`);
      assert.ok(
        densityGeometry.stripScrollWidth > densityGeometry.stripClientWidth
          && densityGeometry.denseOverlap,
        `${label} lets a dense task strip continue beneath the destination overlay`,
      );
      assert.ok(
        densityGeometry.navigationScrollWidth > densityGeometry.navigationClientWidth,
        `${label} caps an expanded future destination set for horizontal scrolling; `
          + JSON.stringify(densityGeometry),
      );
      assert.equal(
        densityGeometry.lastDestinationVisible,
        true,
        `${label} can horizontally reveal the final future destination; `
          + JSON.stringify(densityGeometry),
      );
      assert.equal(
        densityGeometry.navigationCappedToPanel,
        true,
        `${label} never lets the destination strip exceed the panel width`,
      );
      assert.deepEqual(
        [densityGeometry.borderLeftStyle, densityGeometry.borderLeftWidth],
        ["solid", "1px"],
        `${label} draws a hard boundary before global destinations`,
      );
      assert.notEqual(densityGeometry.background, "rgba(0, 0, 0, 0)");
      assert.equal(
        densityGeometry.destinationOnTop,
        true,
        `${label} paints destination controls above overlapped task pills`,
      );
      if (viewport.width <= 560) {
        await addOverflowControls(panel);
      }
      const panelGeometry = await panel.evaluate((node) => {
        const rect = node.getBoundingClientRect();
        const frame = node.closest(".canvas-menu-frame").getBoundingClientRect();
        const summary = node.closest(".canvas-menu").querySelector(":scope > summary")
          .getBoundingClientRect();
        const navigation = node.closest(".canvas-disclosure").querySelector(":scope > .canvas-navigation");
        const navigationStyle = getComputedStyle(navigation);
        const commandbarStyle = getComputedStyle(node.closest(".commandbar--mosaic"));
        const globalNode = node.querySelector(".canvas-menu-global");
        const global = globalNode.getBoundingClientRect();
        const footerNode = node.querySelector(".canvas-menu-footer");
        const footer = footerNode.getBoundingClientRect();
        const version = node.querySelector(".canvas-menu-version").getBoundingClientRect();
        const themeNode = node.querySelector(".canvas-theme-selector");
        const theme = themeNode.getBoundingClientRect();
        const cursor = node.querySelector(".canvas-menu-cursor").getBoundingClientRect();
        const fastMode = node.querySelector('[role="menuitemcheckbox"]').getBoundingClientRect();
        return {
          left: rect.left,
          right: rect.right,
          top: rect.top,
          bottom: rect.bottom,
          summaryBottom: summary.bottom,
          frameBottom: frame.bottom,
          frameLeft: frame.left,
          frameRight: frame.right,
          frameTop: frame.top,
          viewportWidth: innerWidth,
          viewportHeight: innerHeight,
          documentWidth: document.documentElement.scrollWidth,
          commandbarOverflowX: commandbarStyle.overflowX,
          commandbarOverflowY: commandbarStyle.overflowY,
          navigationClientWidth: navigation.clientWidth,
          navigationOverflowX: navigationStyle.overflowX,
          navigationScrollWidth: navigation.scrollWidth,
          globalLeft: global.left,
          globalRight: global.right,
          footerLeft: footer.left,
          footerRight: footer.right,
          footerLast: Array.from(footerNode.parentElement.children)
            .filter((child) => !child.hasAttribute("data-menu-overflow-fixture"))
            .at(-1) === footerNode,
          cursorLeft: cursor.left,
          cursorRight: cursor.right,
          versionLeft: version.left,
          versionRight: version.right,
          themeLeft: theme.left,
          themeRight: theme.right,
          themeInGlobal: themeNode.closest(".canvas-menu-global") === globalNode,
          themeInFooter: footerNode.querySelector(".canvas-theme-selector") !== null,
          fastModeLeft: fastMode.left,
          fastModeRight: fastMode.right,
          cursorInFooter: node.querySelector(".canvas-menu-cursor").parentElement === footerNode,
        };
      });
      assert.ok(
        panelGeometry.left >= -0.5 && panelGeometry.right <= panelGeometry.viewportWidth + 0.5,
        `${label} keeps the Spice menu inside the horizontal viewport`,
      );
      assert.ok(
        panelGeometry.frameTop >= panelGeometry.summaryBottom - 0.5
          && panelGeometry.left >= panelGeometry.frameLeft - 0.5
          && panelGeometry.right <= panelGeometry.frameRight + 0.5
          && panelGeometry.bottom <= panelGeometry.frameBottom + 0.5
          && panelGeometry.frameBottom <= panelGeometry.viewportHeight + 0.5,
        `${label} bounds the context panel inside the disclosure frame`,
      );
      assert.ok(
        !["auto", "scroll"].includes(panelGeometry.commandbarOverflowX)
          && !["auto", "scroll"].includes(panelGeometry.commandbarOverflowY)
          && panelGeometry.navigationOverflowX === "auto"
          && panelGeometry.navigationScrollWidth <= panelGeometry.navigationClientWidth + 1,
        `${label} avoids command-bar scrollbars while retaining a scroll-ready destination strip`,
      );
      assert.equal(
        panelGeometry.cursorInFooter && panelGeometry.footerLast,
        true,
        `${label} places the global cursor in the final footer row`,
      );
      assert.ok(
        panelGeometry.versionLeft >= panelGeometry.footerLeft - 0.5
          && panelGeometry.versionRight <= panelGeometry.cursorLeft + 0.5
          && panelGeometry.cursorRight <= panelGeometry.footerRight + 0.5,
        `${label} keeps version and cursor ordered without overlap inside the footer`,
      );
      assert.ok(
        panelGeometry.globalLeft >= panelGeometry.left - 0.5
          && panelGeometry.globalRight <= panelGeometry.right + 0.5
          && panelGeometry.fastModeLeft >= panelGeometry.globalLeft - 0.5
          && panelGeometry.fastModeRight <= panelGeometry.themeLeft + 0.5
          && panelGeometry.themeRight <= panelGeometry.globalRight + 0.5
          && panelGeometry.themeInGlobal
          && !panelGeometry.themeInFooter,
        `${label} keeps compact Fast and theme controls ordered inside the Global section`,
      );
      const longFooter = await panel.locator(".canvas-menu-footer").evaluate((footer) => {
        const version = footer.querySelector(".canvas-menu-version");
        const cursor = footer.querySelector(".canvas-menu-cursor");
        const original = [version.textContent, cursor.textContent];
        version.textContent = "Canvas version authority with a deliberately long build label";
        cursor.textContent = `cursor ${"1234567890".repeat(12)}`;
        const footerBounds = footer.getBoundingClientRect();
        const versionBounds = version.getBoundingClientRect();
        const cursorBounds = cursor.getBoundingClientRect();
        const result = {
          contained: versionBounds.left >= footerBounds.left - 0.5
            && cursorBounds.right <= footerBounds.right + 0.5,
          cursorClipped: cursor.scrollWidth > cursor.clientWidth,
          cursorOverflow: getComputedStyle(cursor).textOverflow,
          nonOverlapping: versionBounds.right <= cursorBounds.left + 0.5,
          versionClipped: version.scrollWidth > version.clientWidth,
          versionOverflow: getComputedStyle(version).textOverflow,
        };
        [version.textContent, cursor.textContent] = original;
        return result;
      });
      assert.deepEqual(
        longFooter,
        {
          contained: true,
          cursorClipped: true,
          cursorOverflow: "ellipsis",
          nonOverlapping: true,
          versionClipped: true,
          versionOverflow: "ellipsis",
        },
        `${label} ellipsizes long version and cursor labels without overlap`,
      );
      assert.ok(
        panelGeometry.documentWidth <= panelGeometry.viewportWidth,
        `${label} keeps the relocated cursor from causing page overflow`,
      );
      if (viewport.width <= 560) {
        await assertOverflowReachability(panel, label);
      }
      await page.screenshot({
        path: join(
          artifacts,
          viewport.width <= 560
            ? `canvas-menu-overflow-${engine}-${viewport.width}x${viewport.height}.png`
            : `canvas-menu-desktop-${engine}-${viewport.width}x${viewport.height}.png`,
        ),
        fullPage: false,
      });
      await panel.locator('[data-menu-overflow-fixture]').evaluateAll((fixtures) => {
        for (const fixture of fixtures) fixture.remove();
      });
      await page.locator(".canvas-menu > summary").click();
      await waitForDisclosure(page, false, `${label} closes after cursor geometry checks`);
      assert.deepEqual(
        await laneGeometry(),
        collapsedLanes,
        `${label} close preserves lane geometry and scroll offsets`,
      );
      assert.equal(await cursor.count(), 1, `${label} keeps one global cursor after closing`);
      assert.equal(await cursor.isVisible(), false, `${label} hides the global cursor after closing`);
      assert.equal(
        await cursor.getAttribute("data-relocation-identity"),
        cursorIdentity,
        `${label} preserves the global cursor node while closing`,
      );
      assert.equal(await page.locator(".brand-dots").count(), 0, `${label} does not restore the agent-light roster`);
    }
  } catch (error) {
    const viewport = page.viewportSize() ?? previous;
    await captureCursorFailureArtifact(
      page,
      join(artifacts, `canvas-menu-cursor-failure-${engine}-${viewport.width}x${viewport.height}.png`),
      error,
    );
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
      const cursors = document.querySelectorAll(".canvas-menu-cursor");
      const cursorVisible = cursors.length === 1 && cursors[0].checkVisibility();
      return trigger?.getAttribute("aria-expanded") === String(expected)
        && menu.hasAttribute("open") === expected
        && Boolean(document.querySelector(".canvas-navigation")) === expected
        && Boolean(document.querySelector(".task-pill-strip"))
        && cursorVisible === expected
        && !document.querySelector(".brand-dots");
    },
    expanded,
  );
}

async function assertUniversalShelfOuterEdge(page, label, emptyTaskShelf = false) {
  const geometry = await page.locator(".commandbar--mosaic").evaluate((bar, emptyShelf) => {
    const activeWorkspace = [...bar.parentElement.querySelectorAll(":scope > .workspace")]
      .find((workspace) => !workspace.hidden);
    const content = activeWorkspace?.querySelector(":scope > .content");
    const strip = bar.querySelector(".task-pill-strip");
    const originalPills = [...strip.querySelectorAll(":scope > .task-pill")];
    const detached = document.createDocumentFragment();
    if (emptyShelf) {
      for (const pill of originalPills) detached.append(pill);
    }
    let measured;
    try {
      const firstPill = strip.querySelector(":scope > .task-pill");
      const trigger = bar.querySelector(".canvas-menu > summary");
      const barStyle = getComputedStyle(bar);
      const contentStyle = getComputedStyle(content);
      const stripBounds = strip.getBoundingClientRect();
      const summary = trigger.getBoundingClientRect();
      measured = {
        contentPaddingLeft: Number.parseFloat(contentStyle.paddingLeft),
        contentPaddingRight: Number.parseFloat(contentStyle.paddingRight),
        documentWidth: document.documentElement.scrollWidth,
        firstPillLeft: firstPill?.getBoundingClientRect().left ?? null,
        originalTaskPillCount: originalPills.length,
        paddingLeft: Number.parseFloat(barStyle.paddingLeft),
        paddingRight: Number.parseFloat(barStyle.paddingRight),
        stripLeft: stripBounds.left,
        taskPillCount: strip.querySelectorAll(":scope > .task-pill").length,
        triggerRight: summary.right,
        viewportWidth: innerWidth,
      };
    } finally {
      strip.append(detached);
    }
    return {
      ...measured,
      restoredTaskPillCount: strip.querySelectorAll(":scope > .task-pill").length,
    };
  }, emptyTaskShelf);
  const expected = geometry.viewportWidth <= 720 ? 8 : 10;
  assert.deepEqual(
    [geometry.paddingLeft, geometry.paddingRight],
    [expected, expected],
    `${label} gives the universal shelf symmetric shell-owned edges`,
  );
  assert.ok(
    geometry.contentPaddingLeft >= expected && geometry.contentPaddingRight >= expected,
    `${label} keeps active workspace content inside the shell gutter`,
  );
  assert.ok(
    geometry.stripLeft >= expected - 0.5
      && geometry.triggerRight <= geometry.viewportWidth - expected + 0.5
      && geometry.documentWidth <= geometry.viewportWidth,
    `${label} keeps shelf controls and document paint inside the outer gutter`,
  );
  assert.equal(
    geometry.taskPillCount,
    emptyTaskShelf ? 0 : geometry.originalTaskPillCount,
    `${label} measures the requested task-shelf population`,
  );
  assert.equal(
    geometry.restoredTaskPillCount,
    geometry.originalTaskPillCount,
    `${label} restores every mounted task pill after the geometry probe`,
  );
  if (!emptyTaskShelf) {
    assert.ok(
      geometry.firstPillLeft >= expected - 0.5,
      `${label} keeps the first task pill inside the outer gutter`,
    );
  }
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
    const retainedPill = taskStrip.locator(".task-pill").last();
    const identity = await retainedPill.evaluate((node) => {
      node.dataset.menuIdentity = crypto.randomUUID();
      node.focus();
      return node.dataset.menuIdentity;
    });

    await trigger.evaluate((node) => node.dispatchEvent(new MouseEvent("click", {
      bubbles: true,
      cancelable: true,
    })));
    await waitForDisclosure(page, true, "programmatic open retains the task strip");
    assert.equal(await retainedPill.getAttribute("data-menu-identity"), identity);
    assert.equal(await retainedPill.evaluate((node) => document.activeElement === node), true);
    await trigger.evaluate((node) => node.dispatchEvent(new MouseEvent("click", {
      bubbles: true,
      cancelable: true,
    })));
    await waitForDisclosure(page, false, "programmatic close retains the task strip");
    assert.equal(await retainedPill.evaluate((node) => document.activeElement === node), true);

    await trigger.click();
    await waitForDisclosure(page, true, "the pepper disclosure exposes Canvas destinations");
    const destinations = navigation.getByRole("button");
    await destinations.first().focus();
    for (const [index, label] of ["Team", "Board", "Agents"].entries()) {
      assert.match(
        (await page.locator(":focus").innerText()).trim(),
        new RegExp(`^${label}`),
        `keyboard traversal follows visible navigation order at ${label}`,
      );
      if (index < 2) await page.keyboard.press("Tab");
    }
    assert.equal(
      await taskStrip.evaluate((node) => node.scrollLeft),
      maximumScroll,
      "opening the menu preserves task-pill scroll position",
    );
    await page.locator(".mosaic-workspace").click({ position: { x: 2, y: 2 } });
    await waitForDisclosure(page, false, "an outside pointer dismisses the pepper disclosure");
    assert.equal(
      await taskStrip.evaluate((node) => node.scrollLeft),
      maximumScroll,
      "closing the menu preserves task-pill scroll position",
    );
    const lastPill = await taskStrip.locator(".task-pill").last().boundingBox();
    const stripEnd = await taskStrip.boundingBox();
    assert.ok(
      lastPill && stripEnd && lastPill.x + lastPill.width <= stripEnd.x + stripEnd.width + 0.5,
      "the retained task strip keeps its last pill visible",
    );

    await trigger.click();
    await waitForDisclosure(page, true, "the pepper disclosure reopens for project navigation");
    await taskStrip.evaluate((node) => { node.scrollLeft = 0; });
    const exposedPill = taskStrip.locator(".task-pill").first();
    const exposedGeometry = await exposedPill.evaluate((node) => {
      const pill = node.getBoundingClientRect();
      const navigation = node.closest(".canvas-disclosure")
        .querySelector(":scope > .canvas-navigation").getBoundingClientRect();
      return { pillRight: pill.right, navigationLeft: navigation.left };
    });
    assert.ok(
      exposedGeometry.pillRight <= exposedGeometry.navigationLeft + 0.5,
      "project navigation uses the task-pill region before the global overlay boundary",
    );
    await exposedPill.click();
    await waitForDisclosure(page, false, "project navigation dismisses the pepper disclosure");
    assert.equal(
      await exposedPill.evaluate((node) => node === document.activeElement),
      true,
      "project navigation retains focus on its mounted pill",
    );
    await chooseCanvasView(page, "Lanes");
    await waitForDisclosure(page, false, "project navigation check restores the Lanes workspace");
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
  await navigation.getByRole("button", { name: /^Board/ }).focus();
  await page.keyboard.press("Escape");
  await waitForDisclosure(page, false, "Escape dismisses the pepper disclosure");
  assert.ok(
    await trigger.evaluate((node) => node === document.activeElement),
    "Escape restores focus to the pepper trigger",
  );

  await trigger.click();
  await waitForDisclosure(page, true, "the pepper disclosure reopens for destination selection");
  await navigation.getByRole("button", { name: /^Board/ }).click();
  await page.getByRole("heading", { name: "Board", exact: true }).waitFor({ timeout: TRANSITION_MS });
  await waitForDisclosure(page, false, "a Canvas destination closes the pepper disclosure");
  await assertUniversalShelfOuterEdge(page, "narrow Board destination");
  const panel = await openSpiceMenu(page);
  assert.equal(
    await panel.locator(".canvas-menu-page").getAttribute("data-menu-page"),
    "flow",
    "the reopened pepper panel belongs to the selected destination",
  );
  await panel.getByRole("heading", { name: "Board", exact: true }).waitFor();
  await chooseCanvasView(page, "Lanes");
  await waitForDisclosure(page, false, "the disclosure check restores the Lanes workspace");
}

async function assertAxisMenu(page, view, copy) {
  const visibleView = {
    Flow: "Board",
    Topology: "Team",
  }[view] ?? view;
  await chooseCanvasView(page, view);
  await assertUniversalShelfOuterEdge(page, `${visibleView} destination`);
  const panel = await openSpiceMenu(page);
  const menuPage = panel.locator(".canvas-menu-page");
  assert.equal(
    await menuPage.getAttribute("data-menu-page"),
    view.toLowerCase(),
    `${view} opens its own Spice panel rather than the Lanes panel`,
  );
  assert.equal(await panel.locator(".canvas-organization").count(), 0, `${view} renders no organization manager`);
  await panel.getByRole("heading", { name: visibleView, exact: true }).waitFor();
  assert.match(await menuPage.innerText(), copy, `${view} describes its own page context`);
}

async function waitForMemberIn(scope, lane) {
  const member = scope.locator(`.canvas-organization-member[data-organization-member="${lane}"]`);
  await member.waitFor({ timeout: TRANSITION_MS });
  return member;
}

async function assertMemberAccents(scope, expected, message) {
  assert.deepEqual(
    await scope.locator(":scope > .canvas-organization-members > .canvas-organization-member")
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

/** Exercise the Lanes-page Spice panel, shared navigation, and durable organization moves. */
export async function assertOrgMenu(page, fixture, artifacts) {
  await chooseCanvasView(page, "Lanes");
  await assertCursorRelocation(page, artifacts, "chromium");
  await assertPepperDisclosure(page);
  await chooseCanvasView(page, "Topology");
  await page.getByRole("heading", { name: "Team", exact: true }).waitFor();
  await openSpiceMenu(page);
  assert.equal(
    await page.getByRole("navigation", { name: "Canvas views" })
      .getByRole("button", { name: /^Agents/ }).count(),
    1,
    "the analytics pages keep the same route back to live Agents",
  );
  await assertAxisMenu(page, "Topology", /resident organizations, seats, capabilities/i);
  await chooseCanvasView(page, "Lanes");

  let panel = await openSpiceMenu(page);
  assert.equal(await panel.locator(".canvas-menu-page").getAttribute("data-menu-page"), "lanes");
  await panel.getByRole("heading", { name: "Global", exact: true }).waitFor();
  const fastMode = panel.getByRole("menuitemcheckbox", { name: /Fast mode/i });
  const themeRail = panel.getByRole("group", { name: "Canvas theme" });
  const offThemeRight = await themeRail.evaluate((node) => node.getBoundingClientRect().right);
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
    assert.equal(
      await themeRail.evaluate((node) => node.getBoundingClientRect().right),
      offThemeRight,
      "Fast label changes leave the right-aligned theme rail fixed",
    );
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
  assert.equal(await panel.locator(".canvas-organization").count(), 1, "the four agents begin together");
  assert.equal(await panel.locator(".canvas-organization-member").count(), 4);
  await panel.getByRole("heading", { name: "Open Team", exact: true }).waitFor();
  await panel.getByRole("heading", { name: "New Team", exact: true }).waitFor();
  assert.match(
    (await panel.locator(".canvas-organization > header strong").innerText()).trim(),
    /^Team [0-9a-f]{8}$/,
    "the live team card presents its short internal identity without exposing Org vocabulary",
  );
  assert.equal(
    await panel.locator('[data-organization-drop="new"]').innerText(),
    "DROP AGENT HERE",
  );
  assert.match(
    await panel.locator(".canvas-unassigned").innerText(),
    /Agents Without Team[\s\S]*drop here to remove from team[\s\S]*No unassigned agents/i,
  );

  const initial = await reply(fixture, ["lane"]);
  const host = initial.body.lanes[0].lineage_id;
  const moved = initial.body.lanes.at(-1).lane;
  const surface = page.locator(`.organization-surface[data-lineage-id="${host}"]`);
  await surface.locator('.organization-lifetime input[type="range"]').fill("2");
  await waitForPageFunction(
    page,
    "organization lifetime command settles before membership edits",
    TRANSITION_MS,
    (lineage) => document
      .querySelector(`.organization-surface[data-lineage-id="${lineage}"] .organization-composer-controls`)
      ?.dataset.lifetimePending === "false",
    host,
  );
  await surface.getByRole("button", { name: "Drain", exact: true }).waitFor();
  const drained = await reply(fixture, ["lane"]);
  assert.equal(
    drained.body.subscriptions.find((subscription) => subscription.lineage_id === host)?.policy,
    "drain",
    "the organization begins membership edits with a committed Drain policy",
  );
  await page.evaluate(() => {
    window.__orgMenuLineageCommands = [];
    const socket = window.__canvasSockets.at(-1);
    if (!socket) throw new Error("canvas WebSocket was not captured");
    const send = socket.send.bind(socket);
    const record = (bytes) => {
      const command = JSON.parse(new TextDecoder().decode(bytes));
      if (command.kind === "lineage::configure") {
        window.__orgMenuLineageCommands.push(command.body);
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
  let originalOrg = panel.locator(`.canvas-organization[data-organization-lineage="${host}"]`);
  await assertMemberAccents(
    originalOrg,
    ["3", "2", "1", "0"],
    "the organization menu deals accents from the right control edge",
  );
  let member = await waitForMemberIn(originalOrg, moved);
  await member.dragTo(panel.locator('[data-organization-drop="unassigned"]'));

  panel = await openSpiceMenu(page);
  member = await waitForMemberIn(panel.locator(".canvas-unassigned"), moved);
  assert.equal(await member.getAttribute("data-accent"), "0", "a solo member starts at green");
  assert.equal(await panel.locator(".canvas-organization").count(), 1);
  assert.equal(await panel.locator(".canvas-organization").locator(".canvas-organization-member").count(), 3);
  await assertMemberAccents(
    panel.locator(`.canvas-organization[data-organization-lineage="${host}"]`),
    ["2", "1", "0"],
    "removing a member keeps green anchored to the surviving right edge",
  );
  await assertResidentLineage(fixture, moved, moved, host, "removing an agent from its organization");

  originalOrg = panel.locator(`.canvas-organization[data-organization-lineage="${host}"]`);
  await member.dragTo(originalOrg);
  panel = await openSpiceMenu(page);
  originalOrg = panel.locator(`.canvas-organization[data-organization-lineage="${host}"]`);
  member = await waitForMemberIn(originalOrg, moved);
  assert.equal(await originalOrg.locator(".canvas-organization-member").count(), 4);
  await assertMemberAccents(
    originalOrg,
    ["3", "2", "1", "0"],
    "returning the member redeals the sequence from the right edge",
  );
  await assertResidentLineage(fixture, moved, host, host, "returning an agent to its organization");

  await member.dragTo(panel.locator('[data-organization-drop="new"]'));
  panel = await openSpiceMenu(page);
  const createdOrg = panel.locator(
    `.canvas-organization:not([data-organization-lineage="${host}"])`
      + `:has(.canvas-organization-member[data-organization-member="${moved}"])`,
  );
  await createdOrg.waitFor({ timeout: TRANSITION_MS });
  const created = await createdOrg.getAttribute("data-organization-lineage");
  assert.notEqual(created, host, "the New Team target mints a distinct lineage identity");
  assert.notEqual(created, moved, "a new one-agent organization stays distinct from an unassigned agent");
  assert.equal(await panel.locator(".canvas-organization").count(), 2);
  await assertResidentLineage(fixture, moved, created, host, "creating an organization by drop");

  member = await waitForMemberIn(createdOrg, moved);
  originalOrg = panel.locator(`.canvas-organization[data-organization-lineage="${host}"]`);
  await member.dragTo(originalOrg);
  panel = await openSpiceMenu(page);
  originalOrg = panel.locator(`.canvas-organization[data-organization-lineage="${host}"]`);
  await waitForMemberIn(originalOrg, moved);
  assert.equal(await panel.locator(".canvas-organization").count(), 1, "the empty created organization disappears");
  assert.equal(await originalOrg.locator(".canvas-organization-member").count(), 4);
  await assertResidentLineage(fixture, moved, host, host, "restoring the original four-agent organization");
  await waitForPageFunction(
    page,
    "membership commands cross the browser socket",
    TRANSITION_MS,
    () => window.__orgMenuLineageCommands?.length >= 4,
    undefined,
  );
  const membershipCommands = await page.evaluate(() => window.__orgMenuLineageCommands);
  assert.equal(membershipCommands.length, 4, "each drag emits one lineage membership command");
  for (const body of membershipCommands) {
    assert.ok(Object.hasOwn(body, "member"), "membership command identifies the moved agent");
    assert.equal(
      Object.hasOwn(body, "life"),
      false,
      "membership command omits unrelated lifetime state",
    );
  }
  await surface.locator('.organization-lifetime input[type="range"]').fill("0");
  await waitForPageFunction(
    page,
    "organization lifetime returns to the scenario baseline",
    TRANSITION_MS,
    (lineage) => document
      .querySelector(`.organization-surface[data-lineage-id="${lineage}"] .organization-composer-controls`)
      ?.dataset.lifetimePending === "false",
    host,
  );
  await surface.getByRole("button", { name: "Steer", exact: true }).waitFor();
  const restored = await reply(fixture, ["lane"]);
  assert.equal(
    restored.body.subscriptions.find((subscription) => subscription.lineage_id === host)?.policy,
    "steer",
    "the organization-menu scenario restores its incoming lifetime",
  );
}
