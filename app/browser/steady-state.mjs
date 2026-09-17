import assert from "node:assert/strict";

const STABLE_SELECTOR = [
  ".shell--mosaic",
  ".commandbar--mosaic",
  ".mosaic-workspace",
  ".team-fleet",
  ".team-surface",
  ".team-toolbar",
  ".ticker",
  ".team-pane",
  ".team-activity-grid",
  ".lane-mosaic-plane",
  '.mosaic-message-card[data-layout-phase="committed"]',
  ".message-body",
  ".message-image-stack",
  ".message-attachments",
  ".message-image",
  ".message-footer",
  "[data-age-slot]",
].join(",");

const LAYOUT_ATTRIBUTES = [
  "data-track",
  "data-span",
  "data-base",
  "data-row",
  "data-rows",
  "data-plane-commits",
  "data-plane-transition",
  "data-measurement-phase",
  "style",
];

export async function armSteadyStateProbe(page, label) {
  await page.waitForFunction(() => document.getAnimations({ subtree: true })
    .every((animation) => animation.playState !== "running" && animation.playState !== "pending"));
  return page.evaluate(({ attributes, label, selector }) => {
    const token = crypto.randomUUID();
    const round = (value) => Math.round(value * 64) / 64;
    const box = (element) => {
      const bounds = element.getBoundingClientRect();
      return [bounds.x, bounds.y, bounds.width, bounds.height].map(round);
    };
    const visible = (element) => {
      const style = getComputedStyle(element);
      const bounds = element.getBoundingClientRect();
      return style.display !== "none" && style.visibility !== "hidden"
        && bounds.width > 0 && bounds.height > 0;
    };
    const elements = [...document.querySelectorAll(selector)].filter(visible);
    if (elements.length === 0) throw new Error(`${label} found no stable application regions`);
    const snapshots = elements.map((element, index) => ({
      box: box(element),
      client: [element.clientWidth, element.clientHeight],
      element,
      index,
      key: element.dataset.mosaicCardKey ?? element.dataset.agentPlane ?? null,
      scroll: [element.scrollLeft, element.scrollTop],
      scrollSize: [element.scrollWidth, element.scrollHeight],
      tag: `${element.tagName.toLowerCase()}.${[...element.classList].join(".")}`,
    }));
    const movements = [];
    const sample = () => {
      for (const snapshot of snapshots) {
        const current = box(snapshot.element);
        const scroll = [snapshot.element.scrollLeft, snapshot.element.scrollTop];
        if (JSON.stringify(current) !== JSON.stringify(snapshot.box)
            || JSON.stringify(scroll) !== JSON.stringify(snapshot.scroll)) {
          movements.push({
            after: current,
            before: snapshot.box,
            index: snapshot.index,
            key: snapshot.key,
            scrollAfter: scroll,
            scrollBefore: snapshot.scroll,
            tag: snapshot.tag,
          });
        }
      }
      probe.frame = requestAnimationFrame(sample);
    };
    const writes = [];
    const recordMutations = (mutations) => {
      for (const mutation of mutations) {
        if (mutation.type === "attributes") {
          writes.push({
            attribute: mutation.attributeName,
            key: mutation.target.dataset.mosaicCardKey ?? mutation.target.dataset.agentPlane ?? null,
            tag: mutation.target.tagName.toLowerCase(),
          });
          continue;
        }
        const changed = [...mutation.addedNodes, ...mutation.removedNodes].some((node) => (
          node.nodeType === Node.ELEMENT_NODE
            && (node.matches(selector) || node.querySelector(selector))
        ));
        if (changed) writes.push({ childList: true, tag: mutation.target.tagName.toLowerCase() });
      }
    };
    const observer = new MutationObserver(recordMutations);
    const motion = [];
    const recordMotion = (event) => motion.push({
      name: event.animationName ?? event.propertyName ?? null,
      target: event.target?.className ?? event.target?.tagName ?? null,
      type: event.type,
    });
    const shell = document.querySelector(".shell--mosaic");
    if (!shell) throw new Error(`${label} found no Mosaic shell`);
    observer.observe(shell, {
      attributeFilter: attributes,
      attributes: true,
      childList: true,
      subtree: true,
    });
    shell.addEventListener("animationstart", recordMotion, true);
    shell.addEventListener("transitionrun", recordMotion, true);
    const probe = {
      elements,
      frame: requestAnimationFrame(sample),
      label,
      motion,
      movements,
      observer,
      recordMutations,
      recordMotion,
      shell,
      snapshots,
      writes,
    };
    window.__steadyStateProbes ??= new Map();
    window.__steadyStateProbes.set(token, probe);
    return { count: snapshots.length, token };
  }, { attributes: LAYOUT_ATTRIBUTES, label, selector: STABLE_SELECTOR });
}

export async function finishSteadyStateProbe(page, armed, label) {
  const observed = await page.evaluate(({ token }) => {
    const probe = window.__steadyStateProbes?.get(token);
    if (!probe) throw new Error(`steady-state probe ${token} is not armed`);
    cancelAnimationFrame(probe.frame);
    probe.recordMutations(probe.observer.takeRecords());
    probe.observer.disconnect();
    probe.shell.removeEventListener("animationstart", probe.recordMotion, true);
    probe.shell.removeEventListener("transitionrun", probe.recordMotion, true);
    window.__steadyStateProbes.delete(token);
    const round = (value) => Math.round(value * 64) / 64;
    const current = probe.snapshots.map((snapshot) => {
      const bounds = snapshot.element.getBoundingClientRect();
      return {
        box: [bounds.x, bounds.y, bounds.width, bounds.height].map(round),
        client: [snapshot.element.clientWidth, snapshot.element.clientHeight],
        connected: snapshot.element.isConnected,
        index: snapshot.index,
        key: snapshot.key,
        scroll: [snapshot.element.scrollLeft, snapshot.element.scrollTop],
        scrollSize: [snapshot.element.scrollWidth, snapshot.element.scrollHeight],
        tag: snapshot.tag,
      };
    });
    const before = probe.snapshots.map(({ box, client, index, key, scroll, scrollSize, tag }) => ({
      box, client, connected: true, index, key, scroll, scrollSize, tag,
    }));
    return {
      activeAnimations: document.getAnimations({ subtree: true })
        .filter((animation) => animation.playState === "running" || animation.playState === "pending")
        .map((animation) => ({ playState: animation.playState })),
      before,
      current,
      motion: probe.motion,
      movements: probe.movements,
      writes: probe.writes,
    };
  }, armed);
  assert.ok(armed.count > 0, `${label} arms visible application regions`);
  assert.deepEqual(observed.current, observed.before,
    `${label} preserves every visible application box, scroll offset, and node`);
  assert.deepEqual(observed.movements, [], `${label} moves no region on any sampled frame`);
  assert.deepEqual(observed.writes, [], `${label} writes no layout or membership state`);
  assert.deepEqual(observed.motion, [], `${label} starts no CSS animation or transition`);
  assert.deepEqual(observed.activeAnimations, [], `${label} leaves no active animation`);
  return observed;
}
