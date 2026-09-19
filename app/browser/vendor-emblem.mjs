import assert from "node:assert/strict";
import { join } from "node:path";

import { CONNECT_MS, chooseCanvasView, reply, waitForLocator } from "./canvas-fixture.mjs";

const KNOWN_FAMILY_MODEL = "anthropic/claude-opus-5";
const UNKNOWN_FAMILY_MODEL = "google/gemini-3-pro";

/// Everything a composer emblem paints, read off the live DOM.
///
/// The main emblem is the icon's own `svg`; its first path is the glyph in
/// the emblem position. The badge is the `.composer-vendor-badge` group
/// inside it, or nothing: its pi is the gradient-filled path, `plate`
/// reports any filled shape standing behind that pi, and `halo` describes
/// the soft silhouette painted under it, or nothing. Computed colour and
/// opacity are read from the icon so two seats can be held to the same
/// paint rather than the same class, and `surface` is the composer's own
/// surface colour as the icon resolves it, so the halo can be held to it.
function readEmblem(member) {
  const icon = member.querySelector(".composer-vendor-icon");
  if (!icon) throw new Error(`${member.dataset.agentLane} paints no composer emblem`);
  const emblem = icon.querySelector(":scope > svg");
  const badge = icon.querySelector(".composer-vendor-badge");
  const pi = badge?.querySelector('path[fill^="url(#"]') ?? null;
  const halo = badge?.querySelector(".composer-vendor-halo") ?? null;
  const softener = badge?.querySelector("filter") ?? null;
  const blur = softener && [...softener.children].find((child) => child.localName === "feGaussianBlur");
  const style = getComputedStyle(icon);
  const box = icon.getBoundingClientRect();
  const probe = document.createElement("span");
  probe.style.color = "var(--control)";
  icon.append(probe);
  const surface = getComputedStyle(probe).color;
  probe.remove();
  return {
    vendor: icon.dataset.composerVendorIcon,
    provider: icon.dataset.composerVendorProvider ?? null,
    title: icon.getAttribute("title"),
    viewBox: emblem.getAttribute("viewBox"),
    glyph: emblem.querySelector(":scope > path")?.getAttribute("d") ?? null,
    badge: badge && {
      plate: badge.querySelector("circle, rect, ellipse") !== null,
      glyph: pi?.getAttribute("d") ?? null,
      fill: pi?.getAttribute("fill") ?? null,
      gradient: badge.querySelector("linearGradient")?.id ?? null,
      halo: halo && {
        glyph: halo.getAttribute("d"),
        fill: getComputedStyle(halo).fill,
        stroke: getComputedStyle(halo).stroke,
        strokeWidth: Number.parseFloat(getComputedStyle(halo).strokeWidth),
        opacity: Number(getComputedStyle(halo).opacity),
        filter: halo.getAttribute("filter"),
        softener: softener?.id ?? null,
        blur: blur ? Number(blur.getAttribute("stdDeviation")) : null,
        underPi: pi !== null && (halo.compareDocumentPosition(pi) & Node.DOCUMENT_POSITION_FOLLOWING) !== 0,
      },
    },
    surface,
    color: style.color,
    opacity: style.opacity,
    width: box.width,
    height: box.height,
  };
}

/// The oh-my-pi emblem says which model answers, not which harness drives it.
///
/// Two vacant omp seats join the seeded team: one on an anthropic model and
/// one on a family the composer does not know. The known family paints the
/// claude glyph as its main emblem at the same paint and size as the native
/// claude seat beside it, with oh-my-pi's pi over the glyph's corner on no
/// plate but a soft halo of its own shape, blurred and translucent in the
/// surface colour, so the pi reads over the glyph without a cut edge; the
/// unknown family keeps the pi alone as its emblem.
/// Vacant seats suffice because the emblem reads the seat's recorded model,
/// not a running occupant. The roster is kept as a screenshot in each colour
/// scheme before anything is asserted, so the badged pi over the glyph can
/// be inspected as painted.
export async function assertOmpEmblemRoles(page, fixture, worktrees, seeded, artifacts) {
  const lanes = await reply(fixture, ["lane"]);
  const reference = lanes.body.lanes.find((lane) => lane.lane === seeded.lane);
  assert.equal(reference?.vendor, "claude", "the seeded team keeps its native claude seat");
  const seat = async (worktree, name, model) => {
    const added = await reply(fixture, [
      "lane", "add", worktree, "--team", reference.team, "--name", name, "--life", "Steer",
      "--vendor", "omp", "--model", model,
    ]);
    assert.equal(added.body.to, "vacant", `${name} joins the team as a vacant seat`);
    return added.body.lane;
  };
  const knownLane = await seat(worktrees[4], "Anthropic", KNOWN_FAMILY_MODEL);
  const unknownLane = await seat(worktrees[5], "Gemini", UNKNOWN_FAMILY_MODEL);
  const seated = await reply(fixture, ["lane"]);
  for (const [lane, model] of [[knownLane, KNOWN_FAMILY_MODEL], [unknownLane, UNKNOWN_FAMILY_MODEL]]) {
    const row = seated.body.lanes.find((candidate) => candidate.lane === lane);
    assert.deepEqual(
      { team: row?.team, vendor: row?.vendor, model: row?.model, occupant: row?.occupant ?? null },
      { team: reference.team, vendor: "omp", model, occupant: null },
      "the omp seat records the model its emblem reads without running an occupant",
    );
  }

  await page.reload({ waitUntil: "networkidle" });
  await chooseCanvasView(page, "Chat");
  const console_ = page.locator('[data-pane="agent-console"]');
  await console_.getByRole("tab", { name: /^Chat/ }).click();
  const member = (lane) => console_.locator(`.team-agent[data-agent-lane="${lane}"]`);
  for (const lane of [knownLane, unknownLane]) {
    await waitForLocator(page, "an oh-my-pi seat renders its composer emblem", CONNECT_MS,
      member(lane).locator(".composer-vendor-icon"));
  }
  try {
    for (const scheme of ["light", "dark"]) {
      await page.emulateMedia({ colorScheme: scheme });
      await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(resolve)));
      await console_.locator(".team-agents").screenshot({
        path: join(artifacts, `canvas-vendor-emblem-${scheme}.png`),
      });
    }
  } finally {
    await page.emulateMedia({ colorScheme: "light" });
  }
  const [native, known, unknown] = await Promise.all(
    [seeded.lane, knownLane, unknownLane].map((lane) => member(lane).evaluate(readEmblem)),
  );

  assert.equal(native.vendor, "claude", "the reference seat is a native claude lane");
  assert.equal(native.badge, null, "a native vendor emblem wears no badge");
  assert.deepEqual(
    { vendor: known.vendor, provider: known.provider },
    { vendor: "omp", provider: "claude" },
    "the omp seat keeps naming its vendor and its provider family",
  );
  assert.equal(
    known.title,
    `oh-my-pi vendor; model: ${KNOWN_FAMILY_MODEL}; effort: unrecorded; thread: unbound`,
    "the emblem title still reads the seat's own vendor and model",
  );
  assert.equal(known.viewBox, native.viewBox, "the provider glyph keeps its published canvas");
  assert.equal(known.glyph, native.glyph, "the provider family's glyph is the main emblem");
  assert.deepEqual(
    { color: known.color, opacity: known.opacity, width: known.width, height: known.height },
    { color: native.color, opacity: native.opacity, width: native.width, height: native.height },
    "the provider glyph is painted and sized exactly as the native seat's emblem",
  );
  assert.equal(
    known.badge?.plate,
    false,
    "the badged pi stands on the glyph itself, with no plate punched through it",
  );
  assert.equal(known.badge.glyph, unknown.glyph, "the badge is oh-my-pi's pi");
  assert.equal(
    known.badge.fill,
    `url(#${known.badge.gradient})`,
    "the badged pi keeps its own diagonal gradient, defined inside the badge",
  );
  assert.equal(known.badge.gradient, `composer-vendor-gradient-${knownLane}`);
  const halo = known.badge.halo;
  assert.deepEqual(
    {
      glyph: halo?.glyph, fill: halo?.fill, stroke: halo?.stroke,
      filter: halo?.filter, softener: halo?.softener, underPi: halo?.underPi,
    },
    {
      glyph: unknown.glyph, fill: known.surface, stroke: known.surface,
      filter: `url(#composer-vendor-halo-${knownLane})`,
      softener: `composer-vendor-halo-${knownLane}`, underPi: true,
    },
    "the pi stands on its own silhouette, painted under it in the surface colour and softened by the badge's own filter",
  );
  assert.ok(halo.strokeWidth > 0, `the halo is stroked wider than the pi, not ${halo.strokeWidth}`);
  assert.ok(halo.opacity > 0 && halo.opacity < 1, `the halo is translucent, not ${halo.opacity}`);
  assert.ok(halo.blur > 0, `the halo is blurred so it shows no edge, not ${halo.blur}`);

  assert.deepEqual(
    { vendor: unknown.vendor, provider: unknown.provider, badge: unknown.badge },
    { vendor: "omp", provider: null, badge: null },
    "a model of no known family keeps the pi as its emblem with no badge",
  );
  assert.equal(unknown.viewBox, "0 0 64 64", "the lone pi fills omp.sh's own canvas");
  assert.notEqual(unknown.glyph, native.glyph, "the lone pi is not a provider glyph");
  assert.equal(unknown.opacity, "0.9", "the lone pi keeps its own weight");
  assert.deepEqual(
    { width: unknown.width, height: unknown.height },
    { width: native.width, height: native.height },
    "every emblem occupies the same inset",
  );
}
