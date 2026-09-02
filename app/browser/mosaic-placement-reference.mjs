import assert from "node:assert/strict";

export async function armPlacementProbe(plane, excluded = []) {
  return plane.evaluate((element, excludedKeys) => {
    const token = crypto.randomUUID();
    const attributes = ["data-track", "data-span", "data-base", "data-row", "data-rows", "style"];
    const cards = [...element.querySelectorAll('.mosaic-message-card[data-layout-phase="committed"]')]
      .filter((card) => !excludedKeys.includes(card.dataset.mosaicCardKey));
    const keys = cards.map((card) => card.dataset.mosaicCardKey);
    if (keys.length === 0) {
      throw new Error("the Mosaic placement probe armed on no committed card");
    }
    for (const card of cards) card.__mosaicPlacementProbe = token;
    const writes = [];
    const record = (mutation) => {
      if (mutation.target.__mosaicPlacementProbe !== token) return;
      writes.push({
        key: mutation.target.dataset.mosaicCardKey,
        attribute: mutation.attributeName,
        before: mutation.oldValue,
        after: mutation.target.getAttribute(mutation.attributeName),
      });
    };
    const observer = new MutationObserver((mutations) => mutations.forEach(record));
    observer.observe(element, {
      subtree: true,
      attributes: true,
      attributeOldValue: true,
      attributeFilter: attributes,
    });
    element.__mosaicPlacementObserver = { token, keys, writes, observer, record };
    return {
      token,
      commits: Number(element.dataset.planeCommits),
      committedWidth: Number(element.dataset.committedWidth),
      maxRow: Number(element.dataset.maxRow),
      planeRows: Number(element.dataset.planeRows),
      scrollHeight: element.scrollHeight,
      cards: cards.map((card) => {
        const box = card.getBoundingClientRect();
        return {
          key: card.dataset.mosaicCardKey,
          order: [...element.querySelectorAll(".mosaic-message-card")].indexOf(card),
          track: card.dataset.track,
          span: card.dataset.span,
          base: card.dataset.base,
          row: card.dataset.row,
          rows: card.dataset.rows,
          box: { x: box.x, y: box.y, width: box.width, height: box.height },
        };
      }),
    };
  }, excluded);
}

export async function finishPlacementProbe(plane, armed) {
  return plane.evaluate((element, expected) => {
    const probe = element.__mosaicPlacementObserver;
    if (!probe || probe.token !== expected.token) throw new Error("Mosaic placement probe changed");
    probe.observer.takeRecords().forEach(probe.record);
    probe.observer.disconnect();
    const cards = [...element.querySelectorAll(".mosaic-message-card")];
    const retained = probe.keys.every((key) => cards.some(
      (card) => card.dataset.mosaicCardKey === key && card.__mosaicPlacementProbe === probe.token,
    ));
    delete element.__mosaicPlacementObserver;
    return {
      retained,
      writes: probe.writes,
      commits: Number(element.dataset.planeCommits),
      transition: element.dataset.planeTransition,
      observedWidth: Number(element.dataset.observedWidth),
      committedWidth: Number(element.dataset.committedWidth),
      maxRow: Number(element.dataset.maxRow),
      planeRows: Number(element.dataset.planeRows),
      scrollHeight: element.scrollHeight,
      cards: cards
        .filter((card) => probe.keys.includes(card.dataset.mosaicCardKey))
        .map((card) => {
          const box = card.getBoundingClientRect();
          return {
            key: card.dataset.mosaicCardKey,
            order: cards.indexOf(card),
            track: card.dataset.track,
            span: card.dataset.span,
            base: card.dataset.base,
            row: card.dataset.row,
            rows: card.dataset.rows,
            box: { x: box.x, y: box.y, width: box.width, height: box.height },
          };
        }),
    };
  }, armed);
}

export function assertUnmoved(armed, finished, label) {
  assert.ok(finished.retained, `${label} retains every committed card node`);
  assert.deepEqual(finished.writes, [], `${label} writes no card placement`);
  assert.deepEqual(
    finished.cards,
    armed.cards,
    `${label} preserves DOM order, committed placement, and bounding geometry`,
  );
}

export function assertPlacementStable(armed, finished, label) {
  const withoutOrder = (cards) => cards
    .map(({ order: _order, ...card }) => card)
    .sort((left, right) => left.key.localeCompare(right.key));
  assert.ok(finished.retained, `${label} retains every committed card node`);
  assert.deepEqual(finished.writes, [], `${label} writes no card placement`);
  assert.deepEqual(
    withoutOrder(finished.cards),
    withoutOrder(armed.cards),
    `${label} preserves committed placement and viewport geometry`,
  );
}

export function assertPlacementRetained(armed, finished, label) {
  const withoutBox = (cards) => cards.map(({ box: _box, ...card }) => card);
  assert.ok(finished.retained, `${label} retains every committed card node`);
  assert.deepEqual(finished.writes, [], `${label} writes no card placement`);
  assert.deepEqual(
    withoutBox(finished.cards),
    withoutBox(armed.cards),
    `${label} preserves DOM order and committed placement`,
  );
}
