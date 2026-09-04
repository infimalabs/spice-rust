import assert from "node:assert/strict";
import { join } from "node:path";

const VIEWPORTS = Object.freeze([
  Object.freeze(["wide", { width: 1000, height: 568 }]),
  Object.freeze(["narrow", { width: 480, height: 720 }]),
]);
const SCHEMES = Object.freeze(["light", "dark"]);

function close(actual, expected, label) {
  assert.ok(Math.abs(actual - expected) <= 1, `${label}: ${actual} ~= ${expected}`);
}

async function snapshot(stack) {
  return stack.evaluate((node) => {
    const bounds = node.getBoundingClientRect();
    const toolbar = node.closest(".team-toolbar")?.getBoundingClientRect();
    const rail = node.parentElement?.querySelector(":scope > .team-mode-rail")
      ?.getBoundingClientRect();
    return {
      bounds: { height: bounds.height, left: bounds.left, right: bounds.right, width: bounds.width },
      clientWidth: node.clientWidth,
      description: node.getAttribute("aria-describedby")
        ? document.getElementById(node.getAttribute("aria-describedby"))?.textContent.trim()
        : null,
      describedBy: node.getAttribute("aria-describedby"),
      dragHandle: node.hasAttribute("data-lane-drag-handle"),
      draggable: node.getAttribute("draggable"),
      expanded: node.getAttribute("aria-expanded"),
      firstToolbarElement: node.closest(".team-toolbar")?.firstElementChild === node,
      hasPopup: node.getAttribute("aria-haspopup"),
      legacyHandles: node.closest(".team-toolbar")?.querySelectorAll(":scope > .team-drag-handle").length,
      role: node.getAttribute("role"),
      scrollWidth: node.scrollWidth,
      tabIndex: node.tabIndex,
      toolbarHeight: toolbar?.height,
      railLeft: rail?.left,
      pips: [...node.querySelectorAll(":scope > .team-light")].map((pip) => {
        const box = pip.getBoundingClientRect();
        return {
          aria: pip.getAttribute("aria-label"),
          column: Number(pip.dataset.pipColumn),
          height: box.height,
          left: box.left,
          row: Number(pip.dataset.pipRow),
          title: pip.getAttribute("title"),
          top: box.top,
          width: box.width,
        };
      }),
    };
  });
}

function assertStack(actual, single, label, arrangeable) {
  assert.ok(actual.pips.length >= 2, `${label}: fixture carries a multi-pip team`);
  const rows = Math.min(2, actual.pips.length);
  const columns = Math.ceil(actual.pips.length / 2);
  close(actual.bounds.width, Math.max(18, columns * 9 + (columns - 1) * 5), `${label} width`);
  close(actual.bounds.height, rows * 9 + (rows - 1) * 4, `${label} height`);
  assert.ok(actual.scrollWidth <= actual.clientWidth, `${label}: stack does not clip`);
  assert.equal(actual.dragHandle, arrangeable, `${label}: pip handle follows peer availability`);
  assert.equal(actual.draggable, arrangeable.toString(), `${label}: native dragging follows peer availability`);
  assert.equal(actual.role, arrangeable ? "button" : "group", `${label}: pip semantics follow peer availability`);
  assert.equal(actual.tabIndex, arrangeable ? 0 : -1, `${label}: keyboard focus follows peer availability`);
  assert.equal(actual.hasPopup, arrangeable ? "menu" : null, `${label}: only actionable pips announce a menu`);
  assert.equal(actual.expanded, arrangeable ? "false" : null, `${label}: only actionable pips expose menu state`);
  assert.equal(Boolean(actual.describedBy), arrangeable, `${label}: only actionable pips reference instructions`);
  assert.equal(
    actual.description,
    arrangeable
      ? "Press Enter or Space, or tap, to choose where to move or join this team."
      : null,
    `${label}: actionable pips expose keyboard and touch instructions`,
  );
  assert.equal(actual.firstToolbarElement, true, `${label}: pips occupy the first toolbar column`);
  assert.equal(actual.legacyHandles, 0, `${label}: standalone drag symbol is absent`);
  close(actual.toolbarHeight, single.toolbarHeight, `${label} stack does not inflate toolbar`);
  assert.ok(actual.bounds.right <= actual.railLeft, `${label}: stack does not overlap mode rail`);
  for (const [index, pip] of actual.pips.entries()) {
    const expected = {
      column: columns - Math.floor(index / 2),
      row: rows - index % 2,
    };
    assert.deepEqual(
      { column: pip.column, row: pip.row },
      expected,
      `${label}: member ${index} fills bottom-to-top then right-to-left`,
    );
    close(pip.width, 9, `${label} pip ${index} width`);
    close(pip.height, 9, `${label} pip ${index} height`);
    assert.ok(pip.aria, `${label}: pip ${index} retains an accessible label`);
    assert.equal(pip.title, pip.aria, `${label}: pip ${index} exposes its label on hover`);
  }
  const byColumn = Map.groupBy(actual.pips, (pip) => pip.column);
  for (const pips of byColumn.values()) {
    if (pips.length === 2) close(Math.abs(pips[0].top - pips[1].top), 13, `${label} row pitch`);
  }
  const columnLefts = [...byColumn.entries()]
    .map(([column, pips]) => [column, pips[0].left])
    .sort(([left], [right]) => left - right);
  for (let index = 1; index < columnLefts.length; index += 1) {
    close(columnLefts[index][1] - columnLefts[index - 1][1], 14, `${label} column pitch`);
  }
}

async function assertSinglePip(stack, label) {
  const actual = await stack.evaluate((node) => {
    const witness = node.cloneNode(true);
    for (const pip of [...witness.querySelectorAll(":scope > .team-light")].slice(1)) pip.remove();
    const pip = witness.querySelector(":scope > .team-light");
    node.replaceWith(witness);
    const stackBox = witness.getBoundingClientRect();
    const pipBox = pip.getBoundingClientRect();
    const toolbarBox = witness.closest(".team-toolbar")?.getBoundingClientRect();
    witness.replaceWith(node);
    return {
      stackWidth: stackBox.width,
      toolbarHeight: toolbarBox?.height,
      pipHeight: pipBox.height,
      pipWidth: pipBox.width,
      centered: Math.abs((pipBox.left + pipBox.width / 2) - (stackBox.left + stackBox.width / 2)),
    };
  });
  close(actual.stackWidth, 18, `${label} single stack width`);
  close(actual.pipWidth, 9, `${label} single pip width`);
  close(actual.pipHeight, 9, `${label} single pip height`);
  close(actual.centered, 0, `${label} single pip centering`);
  return actual;
}

export async function assertLaneHeaderPipMatrix(page, scope, artifacts, engine, originalScheme) {
  const originalViewport = page.viewportSize();
  assert.ok(originalViewport, `${engine}: pip matrix requires a fixed viewport`);
  const stack = scope.locator(".team-lights:has(> .team-light:nth-child(2))").first();
  await stack.waitFor();
  const arrangeable = await scope.locator(".team-surface").count() > 1;
  try {
    for (const scheme of SCHEMES) {
      await page.emulateMedia({ colorScheme: scheme });
      for (const [viewport, dimensions] of VIEWPORTS) {
        await page.setViewportSize(dimensions);
        await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(resolve)));
        const label = `${engine} ${scheme} ${viewport}`;
        const single = await assertSinglePip(stack, label);
        assertStack(await snapshot(stack), single, label, arrangeable);
        await stack.locator("xpath=ancestor::header[contains(@class, 'team-toolbar')]").screenshot({
          path: join(artifacts, `${engine}-lane-pips-${scheme}-${viewport}.png`),
        });
      }
    }
  } finally {
    await page.emulateMedia({ colorScheme: originalScheme });
    await page.setViewportSize(originalViewport);
  }
}
