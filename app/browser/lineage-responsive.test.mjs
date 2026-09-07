import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { dirname, resolve } from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { launchBrowser } from "./headless-launch.mjs";
import { observeStableRightEdge, settleFleetScroll } from "./lineage-responsive.mjs";

const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const TASK_HANDLES = [
  "EXTRAORDINARILYLONGTASKSTEM-1kKbC3j4",
  "LEASE-1kShort",
  "MISSING-1kLease",
];

test("right-edge settlement rejects predicate-pass followed by geometry drift", () => {
  const geometry = {
    clientWidth: 600,
    newestRight: 600,
    right: 600,
    scrollLeft: 40,
    scrollWidth: 640,
  };
  const node = {
    get clientWidth() { return geometry.clientWidth; },
    get lastElementChild() {
      return { getBoundingClientRect: () => ({ right: geometry.newestRight }) };
    },
    get scrollLeft() { return geometry.scrollLeft; },
    get scrollWidth() { return geometry.scrollWidth; },
    getBoundingClientRect: () => ({ right: geometry.right }),
  };
  const witness = { node, state: { previous: null } };

  assert.equal(
    Math.abs(node.scrollLeft - (node.scrollWidth - node.clientWidth)) <= 1,
    true,
    "the former single-sample predicate would accept the first observation",
  );
  assert.equal(observeStableRightEdge(witness), false, "one anchored observation cannot settle");

  geometry.scrollWidth = 680;
  geometry.scrollLeft = 80;
  geometry.newestRight = 640;
  assert.equal(
    Math.abs(node.scrollLeft - (node.scrollWidth - node.clientWidth)) <= 1,
    true,
    "the former predicate still accepts after the scroll range changes",
  );
  assert.equal(
    observeStableRightEdge(witness),
    false,
    "newest-lane drift clears the witness even while scrollLeft matches overflow",
  );

  geometry.newestRight = 600;
  assert.equal(observeStableRightEdge(witness), false, "the first recovered observation rearms it");
  assert.equal(observeStableRightEdge(witness), true, "two stable anchored observations settle");
});

async function productionTaskContract() {
  const [model, renderer] = await Promise.all([
    readFile(resolve(root, "app/src/watchband.rs"), "utf8"),
    readFile(resolve(root, "app/src/browser/watchband.rs"), "utf8"),
  ]);
  const copy = (variant) => {
    const match = model.match(new RegExp(`Self::${variant} => Some\\("([^"]+)"\\)`));
    assert.ok(match, `production task state exposes ${variant}`);
    return match[1];
  };
  const missingLease = renderer.match(
    /lease_countdown\(standing\.expires_at, "team-task-age", "([^"]+)"\)/,
  );
  assert.ok(missingLease, "production task card exposes missing-lease copy");
  assert.match(
    renderer,
    /role=\{active_state\.unavailable\(\)\.then_some\("status"\)\}/,
    "production unavailable task state owns its status role",
  );
  return {
    empty: copy("NoActiveTasks"),
    missingLease: missingLease[1],
    unavailable: copy("TaskSnapshotUnavailable"),
  };
}

async function responsiveDocument(taskContract) {
  const [style, mosaic, responsive] = await Promise.all([
    readFile(resolve(root, "app/style.css"), "utf8"),
    readFile(resolve(root, "app/mosaic.css"), "utf8"),
    readFile(resolve(root, "app/responsive.css"), "utf8"),
  ]);
  return `<!doctype html><style>${style}\n${mosaic}\n${responsive}
    body { margin: 0; }
    .mosaic-workspace { width: 932px; height: 600px; }
    .mosaic-workspace .content { padding: 0; }
    .lineage-fleet { height: 600px; }
  </style><main class="mosaic-workspace"><div class="content">
    <div class="lineage-fleet" tabindex="0" aria-label="Lineage lanes">${surface(1, taskContract)}${surface(4, taskContract)}</div>
  </div></main>`;
}

function surface(weight, taskContract) {
  return `<section class="team-surface" data-lineage-weight="${weight}">
    <header class="team-toolbar">
      <span class="team-lights"><span class="team-light"></span></span>
      <nav class="team-mode-rail" role="tablist">
        ${["Metrics", "Team", "Tasks", "Agents"].map((label) => `<button class="team-mode">
          <span class="team-mode-label">${label}</span><span class="team-mode-badge">88</span>
        </button>`).join("")}
      </nav>
      <button class="team-menu" aria-label="Team details">•••</button>
    </header>
    <main class="team-task-workspace" aria-label="Team tasks">
      <section class="team-task-section"><header><h3>Active tasks</h3></header>
        <ol class="team-task-list">
          ${taskCard(TASK_HANDLES[0], "A long active task title wraps without hiding its operational meaning", "Extraordinarily long agent identity", "review", "19m")}
          ${taskCard(TASK_HANDLES[1], "Renewal countdown geometry", "Agent two", "todo", "999d+", "expiring")}
          ${taskCard(TASK_HANDLES[2], "Missing lease remains explicit", "Agent three", "plan", taskContract.missingLease, "missing")}
        </ol>
      </section><section class="team-task-section"><header><h3>Direction queue</h3></header>
        <p class="team-task-empty">No queued direction.</p></section>
    </main>
    <div class="composer-shard-heading">
      <span class="composer-shard-link">48m</span>
      <strong>Extraordinarily long configured agent identity on a long worktree branch</strong>
      <span class="composer-shard-image-indicators"></span><span></span>
    </div>
    <textarea>draft</textarea><table><tbody><tr><td>Shared evidence</td><td>Readable value</td></tr></tbody></table>
  </section>`;
}

function taskCard(handle, title, agent, phase, lease, state = "ordinary") {
  return `<li class="team-task-row" data-accent="0" data-task-handle="${handle}">
    <div class="team-task-row-main"><code>${handle}</code>
      <strong class="team-task-title">${title}</strong></div>
    <div class="team-task-row-meta"><span class="team-task-member">
      <span class="agent-presence"></span><span class="team-task-member-name">${agent}</span></span>
      <span class="team-task-phase">${phase}</span><span class="team-task-renewed">expires <span class="team-task-age" data-test-lease="${state}">${lease}</span></span></div>
  </li>`;
}

async function setWidth(page, width) {
  await page.locator(".mosaic-workspace").evaluate((element, value) => {
    element.style.width = `${value}px`;
  }, width);
  await page.evaluate(() => new Promise((resolveFrame) => requestAnimationFrame(resolveFrame)));
}

async function geometry(page) {
  return page.locator(".lineage-fleet").evaluate((fleet) => {
    const bounds = fleet.getBoundingClientRect();
    const style = getComputedStyle(fleet);
    const minimumToken = style.getPropertyValue("--lineage-surface-min-inline").trim();
    const surfaces = [...fleet.children].map((surface) => {
      const rect = surface.getBoundingClientRect();
      const surfaceStyle = getComputedStyle(surface);
      const toolbar = surface.querySelector(".team-toolbar");
      const identity = surface.querySelector(".composer-shard-heading strong");
      const workspace = surface.querySelector(".team-task-workspace");
      const workspaceBounds = workspace.getBoundingClientRect();
      const taskRows = [...surface.querySelectorAll(".team-task-row")];
      const tasks = taskRows.map((task) => {
        const taskBounds = task.getBoundingClientRect();
        const age = task.querySelector(".team-task-age");
        const metadata = task.querySelector(".team-task-row-meta");
        const title = task.querySelector(".team-task-title");
        return {
          bottom: taskBounds.bottom,
          ageOverflow: age.scrollWidth - age.clientWidth,
          handle: task.dataset.taskHandle,
          left: taskBounds.left,
          metadataOverflow: metadata.scrollWidth - metadata.clientWidth,
          overflow: task.scrollWidth - task.clientWidth,
          right: taskBounds.right,
          titleText: title.textContent,
          titleWhiteSpace: getComputedStyle(title).whiteSpace,
          top: taskBounds.top,
          width: taskBounds.width,
          withinWorkspace: taskBounds.left >= workspaceBounds.left
            && taskBounds.right <= workspaceBounds.right,
        };
      });
      const clipped = [...surface.querySelectorAll("button, textarea, table")].some((control) => {
        const controlRect = control.getBoundingClientRect();
        return controlRect.left < rect.left - 1 || controlRect.right > rect.right + 1;
      });
      return {
        weight: Number(surface.dataset.lineageWeight),
        minWidth: Number.parseFloat(minimumToken) * Number.parseFloat(surfaceStyle.fontSize),
        minWidthToken: minimumToken,
        top: Math.round(rect.top),
        left: rect.left,
        right: rect.right,
        width: rect.width,
        height: rect.height,
        direction: surfaceStyle.direction,
        overflow: surface.scrollWidth - surface.clientWidth,
        clipped,
        toolbar: {
          height: toolbar.getBoundingClientRect().height,
          children: [...toolbar.children].map((child) => child.getBoundingClientRect().toJSON()),
        },
        identity: {
          clientWidth: identity.clientWidth,
          scrollWidth: identity.scrollWidth,
          overflow: getComputedStyle(identity).overflow,
          textOverflow: getComputedStyle(identity).textOverflow,
          whiteSpace: getComputedStyle(identity).whiteSpace,
        },
        labels: [...surface.querySelectorAll(".team-mode-label")].map((label) => ({
          box: label.getBoundingClientRect().toJSON(),
          button: label.parentElement.getBoundingClientRect().toJSON(),
          overflow: getComputedStyle(label).overflow,
          fontSize: getComputedStyle(label).fontSize,
          textOverflow: getComputedStyle(label).textOverflow,
          whiteSpace: getComputedStyle(label).whiteSpace,
        })),
        tasks: {
          borderRadius: getComputedStyle(taskRows[0]).borderRadius,
          handles: tasks.map(({ handle }) => handle),
          listTag: surface.querySelector(".team-task-list")?.tagName,
          rows: tasks,
          workspaceOverflow: workspace.scrollWidth - workspace.clientWidth,
        },
      };
    });
    return {
      rows: new Set(surfaces.map(({ top }) => top)).size,
      height: bounds.height,
      left: bounds.left,
      right: bounds.right,
      direction: style.direction,
      flexDirection: style.flexDirection,
      overflowStyleX: style.overflowX,
      overflowStyleY: style.overflowY,
      tabIndex: fleet.tabIndex,
      scrollLeft: fleet.scrollLeft,
      maxScroll: fleet.scrollWidth - fleet.clientWidth,
      overflowX: fleet.scrollWidth - fleet.clientWidth,
      overflowY: fleet.scrollHeight - fleet.clientHeight,
      pageOverflowX: document.documentElement.scrollWidth - innerWidth,
      surfaces,
    };
  });
}

function assertTrack(layout, label, overflow) {
  assert.equal(layout.rows, 1, `${label} remains one row`);
  assert.equal(layout.direction, "ltr");
  assert.equal(layout.flexDirection, "row");
  assert.equal(layout.overflowStyleX, "auto");
  assert.equal(layout.overflowStyleY, "hidden");
  assert.equal(layout.tabIndex, 0);
  assert.ok(layout.overflowY <= 1, `${label} has no vertical overflow`);
  assert.ok(layout.pageOverflowX <= 1, `${label} does not widen the page`);
  assert.ok(layout.surfaces.every(({ width, minWidth, minWidthToken, height, direction, overflow: inner, clipped }) => (
    width >= minWidth - 0.5
      && minWidthToken === "20em"
      && height <= layout.height + 1
      && direction === "ltr"
      && inner <= 1
      && !clipped
  )), `${label} surfaces remain readable: ${JSON.stringify(layout)}`);
  assert.ok(layout.surfaces.every((surface, index) => (
    index === 0 || layout.surfaces[index - 1].left < surface.left
  )), `${label} preserves DOM order west to east`);
  for (const surface of layout.surfaces) {
    assert.ok(
      surface.toolbar.height <= 47,
      `${label} toolbar does not grow: ${JSON.stringify(surface.toolbar)}`,
    );
    assert.ok(surface.toolbar.children.every((child, index, children) => (
      child.left >= surface.left - 0.5
        && child.right <= surface.right + 0.5
        && (index === 0 || children[index - 1].right <= child.left + 0.5)
    )), `${label} toolbar controls remain ordered and contained`);
    assert.ok(surface.labels.every(({ box, button, overflow: inner, fontSize, textOverflow, whiteSpace }) => (
      box.left >= button.left - 0.5
        && box.right <= button.right + 0.5
        && inner === "hidden"
        && fontSize === "12px"
        && textOverflow === "ellipsis"
        && whiteSpace === "nowrap"
    )), `${label} mode labels compress inside their tabs`);
    assert.deepEqual(
      [surface.identity.overflow, surface.identity.textOverflow, surface.identity.whiteSpace],
      ["hidden", "ellipsis", "nowrap"],
      `${label} long identity owns controlled truncation`,
    );
    assert.deepEqual(
      [surface.tasks.listTag, surface.tasks.handles],
      ["OL", TASK_HANDLES],
      `${label} active tasks retain semantic source order`,
    );
    const widths = new Set(surface.tasks.rows.map(({ width }) => Math.round(width * 2) / 2));
    assert.ok(
      Number.parseFloat(surface.tasks.borderRadius) > 0
        && surface.tasks.rows.length === 3
        && widths.size === 1
        && surface.tasks.workspaceOverflow <= 1
        && surface.tasks.rows.every((task, taskIndex, tasks) => (
          task.width > 0
            && task.ageOverflow <= 1
            && task.overflow <= 1
            && task.metadataOverflow <= 1
            && task.titleText.length > 0
            && task.titleWhiteSpace === "normal"
            && task.withinWorkspace
            && (taskIndex === 0 || tasks[taskIndex - 1].bottom < task.top)
        )),
      `${label} active task cards remain framed and contained: ${JSON.stringify(surface.tasks)}`,
    );
    if (surface.width <= surface.minWidth + 1) {
      assert.ok(surface.identity.scrollWidth > surface.identity.clientWidth, `${label} identity truncates at floor`);
    }
  }
  assert.equal(layout.maxScroll > 1, overflow, `${label} horizontal overflow`);
}

async function expectRightEdge(page, label, overflow = true) {
  await page.locator(".lineage-fleet").evaluate((fleet) => { fleet.scrollLeft = fleet.scrollWidth; });
  const layout = await geometry(page);
  assertTrack(layout, label, overflow);
  assert.ok(Math.abs(layout.scrollLeft - layout.maxScroll) <= 1, `${label} reaches the right edge`);
  assert.ok(
    Math.abs(layout.surfaces.at(-1).right - layout.right) <= 1,
    `${label} reveals newest lane: ${JSON.stringify(layout)}`,
  );
  return layout;
}

async function setCount(page, count) {
  await page.locator(".lineage-fleet").evaluate((fleet, nextCount) => {
    while (fleet.childElementCount > nextCount) fleet.lastElementChild.remove();
    while (fleet.childElementCount < nextCount) {
      fleet.append(fleet.firstElementChild.cloneNode(true));
    }
  }, count);
  await page.evaluate(() => new Promise((resolveFrame) => requestAnimationFrame(resolveFrame)));
}

async function setWeights(page, weights) {
  await setCount(page, weights.length);
  await page.locator(".lineage-fleet").evaluate((fleet, values) => {
    [...fleet.children].forEach((surface, index) => {
      surface.dataset.lineageWeight = String(values[index]);
    });
  }, weights);
  await page.evaluate(() => new Promise((resolveFrame) => requestAnimationFrame(resolveFrame)));
}

function taskBoxes(layout) {
  return layout.surfaces[0].tasks.rows.map(({ bottom, left, right, top, width }) => (
    [left, right, top, bottom, width].map((value) => Math.round(value * 2) / 2)
  ));
}

async function assertTaskCardMatrix(page, engine, width, taskContract) {
  await page.setViewportSize({ width, height: 900 });
  await setWeights(page, [1]);
  await setWidth(page, width);
  const label = `${engine} ${width}px three-card matrix`;
  const before = await geometry(page);
  assertTrack(before, label, false);
  const surface = page.locator(".team-surface").first();
  assert.equal(
    (await surface.locator('[data-test-lease="missing"]').innerText()).trim(),
    taskContract.missingLease,
    `${label} names its missing lease`,
  );

  await surface.locator('[data-test-lease="expiring"]').evaluate((age) => {
    const renewed = age.cloneNode(false);
    renewed.textContent = "now";
    age.replaceWith(renewed);
  });
  await page.evaluate(() => new Promise((resolveFrame) => requestAnimationFrame(resolveFrame)));
  const after = await geometry(page);
  assertTrack(after, `${label} after countdown update`, false);
  assert.deepEqual(
    taskBoxes(after),
    taskBoxes(before),
    `${label} authoritative countdown replacement cannot move or resize cards`,
  );
}

async function assertTaskSectionStates(page, engine, taskContract) {
  const workspace = page.locator(".team-task-workspace").first();
  const active = workspace.locator(":scope > .team-task-section").first();
  const original = await active.innerHTML();
  assert.deepEqual(
    await workspace.locator(":scope > .team-task-section h3").allTextContents(),
    ["Active tasks", "Direction queue"],
    `${engine} keeps active ownership before queued direction`,
  );
  for (const state of [
    { copy: taskContract.empty, role: null },
    { copy: taskContract.unavailable, role: "status" },
  ]) {
    await active.evaluate((section, next) => {
      section.innerHTML = `<header><h3>Active tasks</h3></header><p class="team-task-empty"${
        next.role ? ` role="${next.role}"` : ""
      }>${next.copy}</p>`;
    }, state);
    const message = active.locator(".team-task-empty");
    assert.equal((await message.innerText()).trim(), state.copy);
    assert.equal(await message.getAttribute("role"), state.role);
    assert.ok(
      await workspace.evaluate((node) => node.scrollWidth <= node.clientWidth),
      `${engine} ${state.copy} remains contained`,
    );
  }
  await active.evaluate((section, markup) => { section.innerHTML = markup; }, original);
}

function assertRatio(layout, expected, label) {
  const ratio = layout.surfaces[1].width / layout.surfaces[0].width;
  assert.ok(
    Math.abs(ratio - expected) <= expected * 0.04,
    `${label} renders ${expected}:1 within four percent: ${JSON.stringify(layout)}`,
  );
  assert.equal(layout.overflowX <= 1, true, `${label} does not overflow the fleet`);
}

test("lineage minima govern weighted fleet layout in Chromium and Firefox", async (t) => {
  const taskContract = await productionTaskContract();
  const document = await responsiveDocument(taskContract);
  for (const engine of ["chromium", "firefox"]) {
    await t.test(engine, async () => {
      const browser = await launchBrowser(engine);
      const context = await browser.newContext({ viewport: { width: 4000, height: 700 } });
      const page = await context.newPage();
      try {
        await page.setContent(document);
        for (const width of [932, 390]) {
          await assertTaskCardMatrix(page, engine, width, taskContract);
        }
        await assertTaskSectionStates(page, engine, taskContract);
        await page.setViewportSize({ width: 4000, height: 700 });
        await setWidth(page, 2560);
        for (const [weights, expected] of [[[1, 1], 1], [[1, 2], 2], [[1, 6], 6]]) {
          await setWeights(page, weights);
          const weighted = await geometry(page);
          assertTrack(weighted, `${engine} ${expected}:1 weighted row`, false);
          assertRatio(weighted, expected, `${engine} ${expected}:1 weighted row`);
          assert.deepEqual(weighted.surfaces.map(({ weight }) => weight), weights);
        }

        await setWidth(page, 700);
        const floorActive = await geometry(page);
        assertTrack(floorActive, `${engine} floor-active weighted row`, false);
        assert.ok(floorActive.surfaces[0].width <= 321, `${engine} solo lane holds its floor`);
        assert.ok(floorActive.surfaces[1].width > floorActive.surfaces[0].width);

        await setWidth(page, 600);
        await expectRightEdge(page, `${engine} two-lane floor overflow`);

        await setWeights(page, [1]);
        await page.setViewportSize({ width: 280, height: 700 });
        await setWidth(page, 280);
        const subFloor = await expectRightEdge(page, `${engine} sub-floor viewport`);
        assert.equal(subFloor.surfaces[0].width, 320, `${engine} viewport cannot lower the lane floor`);

        await page.evaluate(() => { document.documentElement.style.fontSize = "20px"; });
        await setWeights(page, [1, 1]);
        await page.setViewportSize({ width: 700, height: 700 });
        await setWidth(page, 700);
        const zoomed = await expectRightEdge(page, `${engine} zoomed floor overflow`);
        assert.ok(zoomed.surfaces.every(({ width, minWidth }) => width === 400 && minWidth === 400));
        await page.evaluate(() => { document.documentElement.style.fontSize = ""; });

        await setWeights(page, [1, 3, 6]);
        await page.setViewportSize({ width: 932, height: 700 });
        await setWidth(page, 932);
        const threeLane = await expectRightEdge(page, `${engine} three weighted lane floors`);
        assert.deepEqual(threeLane.surfaces.map(({ weight }) => weight), [1, 3, 6]);

        for (const count of [3, 7]) {
          await setCount(page, count);
          await setWidth(page, 932);
          await expectRightEdge(page, `${engine} ${count}-lane desktop track`);
        }

        await page.locator(".lineage-fleet").focus();
        await settleFleetScroll(
          page,
          page.locator(".lineage-fleet"),
          `${engine} keyboard traversal`,
          () => page.keyboard.press("ArrowLeft"),
        );
        let traversed = await geometry(page);
        assert.ok(traversed.scrollLeft < traversed.maxScroll, `${engine} keyboard traverses west`);
        await page.locator(".lineage-fleet").evaluate((fleet) => { fleet.scrollLeft = fleet.scrollWidth; });
        await page.locator(".lineage-fleet").hover();
        await page.mouse.wheel(-300, 0);
        await page.waitForTimeout(50);
        traversed = await geometry(page);
        assert.ok(traversed.scrollLeft < traversed.maxScroll, `${engine} horizontal wheel traverses west`);

        await page.locator(".lineage-fleet").evaluate((fleet) => { fleet.scrollLeft = fleet.scrollWidth; });
        await setWeights(page, [1, 3, 6]);
        await expectRightEdge(page, `${engine} added lane remains at east edge`);
        await setCount(page, 2);
        await page.setViewportSize({ width: 700, height: 700 });
        await setWidth(page, 600);
        await expectRightEdge(page, `${engine} narrow track after removal and resize`);
      } finally {
        await context.close();
        await browser.close();
      }
    });
  }
});
