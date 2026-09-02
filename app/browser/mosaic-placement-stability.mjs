import assert from "node:assert/strict";

function indexed(snapshot, label) {
  const cards = new Map(snapshot.map((card) => [card.key, card]));
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

/// Require movement only for the cards named before the transition executes.
export function assertDeclaredPlacementMovement(before, after, declaration) {
  const { cause, label, targets } = declaration;
  assert.ok(typeof cause === "string" && cause.length > 0, `${label} declares a governing cause`);
  assert.ok(Array.isArray(targets) && targets.length > 0, `${label} declares movement targets`);
  const targetSet = new Set(targets);
  assert.equal(targetSet.size, targets.length, `${label} declares each movement target once`);
  const earlier = indexed(before, `${label} before snapshot`);
  const later = indexed(after, `${label} after snapshot`);
  assert.deepEqual([...later.keys()], [...earlier.keys()], `${label} preserves card identities`);

  for (const [key, prior] of earlier) {
    const current = later.get(key);
    if (targetSet.has(key)) {
      assert.notEqual(
        current.bytes,
        prior.bytes,
        `${label}: target ${JSON.stringify(key)} did not move for ${cause}; tuple=${prior.bytes}`,
      );
    } else {
      assert.equal(
        current.bytes,
        prior.bytes,
        `${label}: card ${JSON.stringify(key)} moved unexpectedly for ${cause}; before=${prior.bytes} after=${current.bytes}`,
      );
    }
  }
  for (const key of targetSet) {
    assert.ok(earlier.has(key), `${label}: movement target ${JSON.stringify(key)} is absent`);
  }
}
