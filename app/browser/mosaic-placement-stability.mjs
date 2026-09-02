import assert from "node:assert/strict";

/// Read placement from committed attributes, not animated viewport geometry.
export async function captureCanonicalPlacements(plane, selectedKeys = null) {
  return plane.evaluate((element, requested) => {
    const requestedSet = requested === null ? null : new Set(requested);
    const canonicalInteger = /^(?:0|-?[1-9]\d*)$/;
    const cards = [...element.querySelectorAll('.mosaic-message-card[data-layout-phase="committed"]')]
      .filter((card) => requestedSet === null || requestedSet.has(card.dataset.mosaicCardKey))
      .map((card) => {
        const key = card.dataset.mosaicCardKey;
        const raw = [card.dataset.track, card.dataset.base, card.dataset.span, card.dataset.rows];
        if (!key || raw.some((value) => !canonicalInteger.test(value))) {
          throw new Error(`card has a non-canonical committed placement: ${JSON.stringify({ key, raw })}`);
        }
        const tuple = raw.map(Number);
        if (tuple.some((value) => !Number.isSafeInteger(value))) {
          throw new Error(`card has a non-canonical committed placement: ${JSON.stringify({ key, raw })}`);
        }
        return { key, tuple, bytes: JSON.stringify(tuple) };
      })
      .sort((left, right) => left.key.localeCompare(right.key));
    const keys = cards.map(({ key }) => key);
    if (new Set(keys).size !== keys.length) {
      throw new Error(`canonical placement snapshot contains duplicate identities: ${JSON.stringify(keys)}`);
    }
    if (requestedSet !== null) {
      const missing = requested.filter((key) => !keys.includes(key));
      if (missing.length > 0) {
        throw new Error(`canonical placement snapshot is missing selected cards: ${JSON.stringify(missing)}`);
      }
    }
    if (cards.length === 0) {
      throw new Error("canonical placement snapshot found no committed Mosaic cards");
    }
    return cards;
  }, selectedKeys);
}

function indexed(snapshot, label) {
  const cards = new Map(snapshot.map((card) => {
    assert.ok(typeof card.key === "string" && card.key.length > 0, `${label} has a card identity`);
    assert.ok(
      Array.isArray(card.tuple)
        && card.tuple.length === 4
        && card.tuple.every(Number.isSafeInteger),
      `${label} has a canonical integer placement for ${JSON.stringify(card.key)}`,
    );
    assert.equal(
      card.bytes,
      JSON.stringify(card.tuple),
      `${label} has canonical placement bytes for ${JSON.stringify(card.key)}`,
    );
    return [card.key, card];
  }));
  assert.equal(cards.size, snapshot.length, `${label} has unique card identities`);
  return cards;
}

/// Assert exact tuple retention while allowing only explicitly named additions.
export function assertCanonicalPlacementsStable(
  before,
  after,
  label,
  addedKeys = [],
) {
  const earlier = indexed(before, `${label} before snapshot`);
  const later = indexed(after, `${label} after snapshot`);
  const additions = new Set(addedKeys);
  assert.equal(additions.size, addedKeys.length, `${label} declares each addition once`);

  for (const [key, prior] of earlier) {
    const current = later.get(key);
    assert.ok(current, `${label}: card ${JSON.stringify(key)} disappeared; before=${prior.bytes}`);
    assert.equal(
      current.bytes,
      prior.bytes,
      `${label}: card ${JSON.stringify(key)} moved unexpectedly; before=${prior.bytes} after=${current.bytes}`,
    );
  }
  for (const [key, current] of later) {
    assert.ok(
      earlier.has(key) || additions.has(key),
      `${label}: undeclared card ${JSON.stringify(key)} appeared; after=${current.bytes}`,
    );
  }
  for (const key of additions) {
    assert.ok(!earlier.has(key), `${label}: declared addition ${JSON.stringify(key)} existed before`);
    assert.ok(later.has(key), `${label}: declared addition ${JSON.stringify(key)} did not appear`);
  }
}

/// Require each named card to land at its declared tuple and every other card to stay fixed.
export function assertDeclaredPlacementMovement(before, after, declaration) {
  const { cause, label, targets } = declaration;
  assert.ok(typeof cause === "string" && cause.length > 0, `${label} declares a governing cause`);
  assert.ok(Array.isArray(targets) && targets.length > 0, `${label} declares movement targets`);
  const expected = new Map(targets.map((target) => {
    assert.ok(
      typeof target?.key === "string" && target.key.length > 0,
      `${label} declares a target identity`,
    );
    assert.ok(
      Array.isArray(target.tuple)
        && target.tuple.length === 4
        && target.tuple.every(Number.isSafeInteger),
      `${label} declares an exact integer tuple for ${JSON.stringify(target.key)}`,
    );
    return [target.key, JSON.stringify(target.tuple)];
  }));
  assert.equal(expected.size, targets.length, `${label} declares each movement target once`);
  const earlier = indexed(before, `${label} before snapshot`);
  const later = indexed(after, `${label} after snapshot`);
  assert.deepEqual([...later.keys()], [...earlier.keys()], `${label} preserves card identities`);

  for (const [key, prior] of earlier) {
    const current = later.get(key);
    const expectedBytes = expected.get(key);
    if (expectedBytes !== undefined) {
      assert.notEqual(
        expectedBytes,
        prior.bytes,
        `${label}: target ${JSON.stringify(key)} declares no movement for ${cause}; tuple=${prior.bytes}`,
      );
      assert.equal(
        current.bytes,
        expectedBytes,
        `${label}: target ${JSON.stringify(key)} moved to an unexpected placement for ${cause}; expected=${expectedBytes} actual=${current.bytes}`,
      );
    } else {
      assert.equal(
        current.bytes,
        prior.bytes,
        `${label}: card ${JSON.stringify(key)} moved unexpectedly for ${cause}; before=${prior.bytes} after=${current.bytes}`,
      );
    }
  }
  for (const key of expected.keys()) {
    assert.ok(earlier.has(key), `${label}: movement target ${JSON.stringify(key)} is absent`);
  }
}
