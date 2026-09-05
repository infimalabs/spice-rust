import assert from "node:assert/strict";

const TWO_LINEAGE_THRESHOLD = 64.5;
const THREE_LINEAGE_THRESHOLD = 97;

async function settleLayout(page) {
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(() => resolve())));
}

async function setContentWidth(page, content, widthRem) {
  for (let attempt = 0; attempt < 2; attempt += 1) {
    const geometry = await content.evaluate((element) => {
      const style = getComputedStyle(element);
      const padding = parseFloat(style.paddingLeft) + parseFloat(style.paddingRight);
      return {
        content: element.clientWidth - padding,
        root: parseFloat(getComputedStyle(document.documentElement).fontSize),
        viewport: innerWidth,
      };
    });
    await page.setViewportSize({
      width: Math.round(geometry.viewport + widthRem * geometry.root - geometry.content),
      height: 988,
    });
    await settleLayout(page);
  }
}

async function fleetGeometry(fleet) {
  return fleet.evaluate((element) => {
    const bounds = element.getBoundingClientRect();
    const surfaces = [...element.querySelectorAll(":scope > .team-surface")];
    return {
      direction: getComputedStyle(element).flexDirection,
      ids: surfaces.map((surface) => surface.dataset.lineageId),
      modes: surfaces.map((surface) => surface.querySelector(".team-pane")?.dataset.teamMode),
      surfaces: surfaces.map((surface) => {
        const rect = surface.getBoundingClientRect();
        const clipped = [...surface.querySelectorAll("button, textarea, table")].some((control) => {
          const controlRect = control.getBoundingClientRect();
          return controlRect.left < rect.left - 1 || controlRect.right > rect.right + 1;
        });
        return {
          width: rect.width,
          contained: rect.left >= bounds.left - 1 && rect.right <= bounds.right + 1,
          overflow: surface.scrollWidth - surface.clientWidth,
          clipped,
        };
      }),
    };
  });
}

async function assertDirection(page, content, fleet, width, expected) {
  await setContentWidth(page, content, width);
  const geometry = await fleetGeometry(fleet);
  assert.equal(geometry.direction, expected, `${width}rem fleet direction`);
  return geometry;
}

export async function assertLineageResponsiveLayout(page, console_) {
  const originalViewport = page.viewportSize();
  const content = page.locator(".mosaic-workspace .content");
  const fleet = console_;
  const original = await fleetGeometry(fleet);
  assert.equal(original.surfaces.length, 2);
  let cloned = false;
  try {
    await page.setViewportSize({ width: 932, height: 988 });
    await settleLayout(page);
    const reproduction = await fleetGeometry(fleet);
    assert.equal(reproduction.direction, "column", "932px stacks two lineages");
    assert.ok(reproduction.surfaces.every(
      ({ width, contained, overflow, clipped }) => width >= 512 && contained && overflow <= 1 && !clipped,
    ), `932px surfaces remain readable: ${JSON.stringify(reproduction)}`);
    await assertDirection(page, content, fleet, TWO_LINEAGE_THRESHOLD - 0.0625, "column");
    await assertDirection(page, content, fleet, TWO_LINEAGE_THRESHOLD, "row");
    await assertDirection(page, content, fleet, TWO_LINEAGE_THRESHOLD + 0.0625, "row");

    await fleet.evaluate((element) => element.append(element.firstElementChild.cloneNode(true)));
    cloned = true;
    await assertDirection(page, content, fleet, THREE_LINEAGE_THRESHOLD - 0.0625, "column");
    await assertDirection(page, content, fleet, THREE_LINEAGE_THRESHOLD, "row");
    await assertDirection(page, content, fleet, THREE_LINEAGE_THRESHOLD + 0.0625, "row");
    await fleet.evaluate((element) => element.lastElementChild.remove());
    cloned = false;

    const restored = await fleetGeometry(fleet);
    assert.deepEqual(restored.ids, original.ids, "resize retains lineage identity and order");
    assert.deepEqual(restored.modes, original.modes, "resize retains each selected lineage mode");
  } finally {
    if (cloned) await fleet.evaluate((element) => element.lastElementChild.remove());
    if (originalViewport) await page.setViewportSize(originalViewport);
    await settleLayout(page);
  }
}
