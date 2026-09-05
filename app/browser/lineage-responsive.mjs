import assert from "node:assert/strict";

async function settleLayout(page) {
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => resolve())));
}

async function fleetGeometry(fleet) {
  return fleet.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const style = getComputedStyle(element);
    const minimumToken = style.getPropertyValue("--lineage-surface-min-inline").trim();
    const surfaces = [...element.querySelectorAll(":scope > .team-surface")];
    const surfaceGeometry = surfaces.map((surface) => {
      const rect = surface.getBoundingClientRect();
      const surfaceStyle = getComputedStyle(surface);
      const controls = [...surface.querySelectorAll("button, textarea, table")]
        .filter((control) => control.closest('.team-mode-pane[data-active="false"]') === null);
      const clippedControls = controls.flatMap((control) => {
        const controlRect = control.getBoundingClientRect();
        if (controlRect.left >= rect.left - 1 && controlRect.right <= rect.right + 1) return [];
        return [{
          tag: control.tagName,
          className: control.className,
          left: controlRect.left,
          right: controlRect.right,
        }];
      });
      return {
        top: Math.round(rect.top),
        left: rect.left,
        right: rect.right,
        width: rect.width,
        minWidth: Number.parseFloat(minimumToken) * Number.parseFloat(surfaceStyle.fontSize),
        minWidthToken: minimumToken,
        height: rect.height,
        direction: surfaceStyle.direction,
        overflow: surface.scrollWidth - surface.clientWidth,
        clipped: clippedControls,
      };
    });
    return {
      rows: new Set(surfaceGeometry.map(({ top }) => top)).size,
      height: bounds.height,
      right: bounds.right,
      direction: style.direction,
      flexDirection: style.flexDirection,
      overflowX: element.scrollWidth - element.clientWidth,
      overflowY: element.scrollHeight - element.clientHeight,
      scrollLeft: element.scrollLeft,
      lastRight: surfaceGeometry.at(-1)?.right,
      pageOverflowY: document.documentElement.scrollHeight - innerHeight,
      ids: surfaces.map((surface) => surface.dataset.lineageId),
      modes: surfaces.map((surface) => surface.querySelector(".team-pane")?.dataset.teamMode),
      drafts: surfaces.flatMap((surface) => [...surface.querySelectorAll("textarea")]
        .map((draft) => draft.value)),
      laneScrolls: surfaces.flatMap((surface) => [...surface.querySelectorAll(".lane-mosaic-plane")]
        .map((lane) => lane.scrollTop)),
      surfaces: surfaceGeometry,
    };
  });
}

function assertTrack(geometry, label, overflow) {
  assert.equal(geometry.rows, 1, `${label} remains one row`);
  assert.equal(geometry.direction, "ltr");
  assert.equal(geometry.flexDirection, "row");
  assert.equal(geometry.overflowX > 1, overflow, `${label} horizontal overflow`);
  assert.ok(geometry.overflowY <= 1, `${label} has no vertical fleet overflow`);
  assert.ok(geometry.pageOverflowY <= 1, `${label} has no page-level vertical overflow`);
  assert.ok(Math.abs(geometry.scrollLeft - geometry.overflowX) <= 1, `${label} is right anchored`);
  assert.ok(Math.abs(geometry.lastRight - geometry.right) <= 1, `${label} reveals the newest lane`);
  assert.ok(geometry.surfaces.every(({ width, minWidth, minWidthToken, height, direction, overflow, clipped }) => (
    width >= minWidth - 0.5
      && minWidthToken === "20em"
      && height <= geometry.height + 1
      && direction === "ltr"
      && overflow <= 1
      && clipped.length === 0
  )), `${label} surfaces remain readable: ${JSON.stringify(geometry)}`);
}

export async function assertLineageResponsiveLayout(page, console_) {
  const originalViewport = page.viewportSize();
  const fleet = console_;
  await fleet.locator(".lane-mosaic-plane").evaluateAll((lanes) => {
    lanes.forEach((lane) => { lane.scrollTop = Math.min(24, lane.scrollHeight - lane.clientHeight); });
  });
  const original = await fleetGeometry(fleet);
  assert.equal(original.surfaces.length, 2);
  let cloned = false;
  try {
    await page.setViewportSize({ width: 932, height: 988 });
    await settleLayout(page);
    const reproduction = await fleetGeometry(fleet);
    assertTrack(reproduction, "932px reproduction", false);

    await fleet.evaluate((element) => element.append(element.firstElementChild.cloneNode(true)));
    cloned = true;
    await page.setViewportSize({ width: 700, height: 988 });
    await settleLayout(page);
    assertTrack(await fleetGeometry(fleet), "narrow three-lane track", true);
    await fleet.focus();
    await page.keyboard.press("ArrowLeft");
    await page.waitForTimeout(50);
    const traversed = await fleetGeometry(fleet);
    assert.ok(traversed.scrollLeft < traversed.overflowX, "keyboard traverses toward older lanes");
    await fleet.evaluate((element) => element.lastElementChild.remove());
    cloned = false;

    const restored = await fleetGeometry(fleet);
    assert.deepEqual(restored.ids, original.ids, "resize retains lineage identity and order");
    assert.deepEqual(restored.modes, original.modes, "resize retains each selected lineage mode");
    assert.deepEqual(restored.drafts, original.drafts, "resize retains lineage drafts");
    assert.deepEqual(restored.laneScrolls, original.laneScrolls, "resize retains lane scroll positions");
  } finally {
    if (cloned) await fleet.evaluate((element) => element.lastElementChild.remove());
    if (originalViewport) await page.setViewportSize(originalViewport);
    await settleLayout(page);
  }
}
