import assert from "node:assert/strict";

const EXPECTED_ENGINES = Object.freeze(["chromium", "firefox"]);
const READY_TIMEOUT_MS = 10_000;
const completedEngines = new Set();

function measurementViolations(samples) {
  return samples.flatMap((sample) => {
    if (!sample.paintable) return [];
    const violations = [];
    if (sample.measurementActive) violations.push("measurement-active");
    if (Math.abs(sample.width - sample.expectedWidth) > 1) {
      violations.push("intermediate-width");
    }
    if (sample.layoutPhase === "committed" && sample.columnEnd !== `span ${sample.allowedSpan}`) {
      violations.push("committed-column");
    }
    return violations.map((reason) => ({ reason, sample }));
  });
}

async function waitForCommit(page, priorCommit, expectedViewportWidth) {
  await page.waitForFunction(
    ({ prior, viewport }) => {
      const plane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
      return plane?.dataset.measurementPhase === "ready"
        && plane.dataset.measurementPending === "0"
        && Number(plane.dataset.planeCommits) > prior
        && window.innerWidth === viewport;
    },
    { prior: priorCommit, viewport: expectedViewportWidth },
    { timeout: READY_TIMEOUT_MS },
  );
}

async function armPaintProbe(page) {
  return page.evaluate(() => {
    const plane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    if (!plane) throw new Error("the Mosaic candidate paint probe found no plane");
    const twoReadCards = [...plane.querySelectorAll(".mosaic-message-card")]
      .filter((candidate) => (
        candidate.dataset.candidateSpans.split(",").filter(Boolean).length === 2
      ))
      .sort((left, right) => (
        Number(left.dataset.creationIndex) - Number(right.dataset.creationIndex)
      ));
    const card = twoReadCards[0];
    if (!card) throw new Error("the Mosaic candidate paint probe found no two-read live card");
    const scrollTop = plane.scrollTop;
    card.scrollIntoView({ block: "center" });
    const candidateSpans = card.dataset.candidateSpans
      .split(",")
      .filter(Boolean)
      .map(Number);
    if (candidateSpans.length !== 2 || candidateSpans[1] <= candidateSpans[0]) {
      throw new Error(`the tall witness has no distinct wide candidate: ${candidateSpans}`);
    }

    const token = crypto.randomUUID();
    const mutations = [];
    const samples = [];
    const styleProperty = (cssText, property) => {
      const scratch = document.createElement("div");
      scratch.style.cssText = cssText ?? "";
      return scratch.style.getPropertyValue(property).trim();
    };
    const record = (mutation) => {
      mutations.push({
        attribute: mutation.attributeName,
        oldValue: mutation.oldValue,
        value: mutation.target.getAttribute(mutation.attributeName),
        oldMeasureProperty: mutation.attributeName === "style"
          ? styleProperty(mutation.oldValue, "--mosaic-measure-span")
          : null,
      });
    };
    const observer = new MutationObserver((records) => records.forEach(record));
    observer.observe(card, {
      attributes: true,
      attributeOldValue: true,
      attributeFilter: ["data-measure-span", "data-mosaic-measurement-active", "style"],
    });

    card.__mosaicCandidatePaintToken = token;
    let frame;
    const sample = () => {
      const cardStyle = getComputedStyle(card);
      const track = plane.querySelector(":scope > .lane-mosaic-track");
      const trackStyle = getComputedStyle(track);
      const box = card.getBoundingClientRect();
      const planeBox = plane.getBoundingClientRect();
      const gap = Number.parseFloat(trackStyle.columnGap);
      const trackWidth = (track.clientWidth - gap * 11) / 12;
      const layoutPhase = card.dataset.layoutPhase;
      const baseSpan = candidateSpans[0];
      const committedSpan = Number(card.dataset.span);
      const allowedSpan = layoutPhase === "measuring" ? baseSpan : committedSpan;
      const expectedWidth = trackWidth * allowedSpan + gap * (allowedSpan - 1);
      samples.push({
        allowedSpan,
        columnEnd: cardStyle.gridColumnEnd,
        committedSpan,
        expectedWidth,
        layoutPhase,
        measureProperty: cardStyle.getPropertyValue("--mosaic-measure-span").trim(),
        measureSpan: Number(card.dataset.measureSpan),
        measurementActive: card.hasAttribute("data-mosaic-measurement-active"),
        paintable: cardStyle.display !== "none"
          && cardStyle.visibility !== "hidden"
          && Number.parseFloat(cardStyle.opacity) > 0
          && box.bottom > 0
          && box.right > 0
          && box.top < innerHeight
          && box.left < innerWidth
          && box.bottom > planeBox.top
          && box.right > planeBox.left
          && box.top < planeBox.bottom
          && box.left < planeBox.right,
        planePhase: plane.dataset.measurementPhase,
        planePending: Number(plane.dataset.measurementPending),
        width: box.width,
      });
      frame = requestAnimationFrame(sample);
    };
    frame = requestAnimationFrame(sample);
    plane.__mosaicCandidatePaintProbe = { card, frame, mutations, observer, record, samples, token };
    return {
      baseSpan: candidateSpans[0],
      commit: Number(plane.dataset.planeCommits),
      scrollTop,
      token,
      wideSpan: candidateSpans[1],
    };
  });
}

async function finishPaintProbe(page, armed) {
  return page.evaluate((expected) => {
    const plane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    const probe = plane?.__mosaicCandidatePaintProbe;
    if (!probe || probe.token !== expected.token) {
      throw new Error("the Mosaic candidate paint probe changed before settlement");
    }
    probe.observer.takeRecords().forEach(probe.record);
    probe.observer.disconnect();
    cancelAnimationFrame(probe.frame);
    const retained = probe.card.__mosaicCandidatePaintToken === probe.token
      && probe.card.isConnected;
    plane.scrollTop = expected.scrollTop;
    delete probe.card.__mosaicCandidatePaintToken;
    delete plane.__mosaicCandidatePaintProbe;
    return {
      candidateSpans: probe.card.dataset.candidateSpans.split(",").filter(Boolean).map(Number),
      finalMeasureProperty: getComputedStyle(probe.card)
        .getPropertyValue("--mosaic-measure-span").trim(),
      finalMeasureSpan: Number(probe.card.dataset.measureSpan),
      mutations: probe.mutations,
      retained,
      samples: probe.samples,
    };
  }, armed);
}

async function negativeControl(page) {
  return page.evaluate(async () => {
    const sourcePlane = document.querySelector('[data-pane="agent-console"] .lane-mosaic-plane');
    const source = [...sourcePlane.querySelectorAll(".mosaic-message-card")].find(
      (candidate) => candidate.dataset.candidateSpans.split(",").filter(Boolean).length === 2,
    );
    if (!source) throw new Error("the Mosaic paint negative control found no two-read card");
    const [baseSpan, wideSpan] = source.dataset.candidateSpans.split(",").map(Number);
    const plane = sourcePlane.cloneNode(false);
    plane.removeAttribute("data-agent-plane");
    plane.dataset.measurementPhase = "ready";
    plane.dataset.measurementPending = "0";
    plane.dataset.mosaicPaintControl = "true";
    plane.style.cssText = [
      "position:fixed",
      "inset:8px auto auto 8px",
      `width:${sourcePlane.clientWidth}px`,
      "height:240px",
      "z-index:2147483647",
      "pointer-events:none",
      "overflow:hidden",
    ].join(";");
    const track = sourcePlane.querySelector(":scope > .lane-mosaic-track").cloneNode(false);
    const card = source.cloneNode(true);
    card.dataset.layoutPhase = "committed";
    card.dataset.span = String(baseSpan);
    card.dataset.mosaicPaintControl = "true";
    card.style.setProperty("--mosaic-span", String(baseSpan));
    card.style.setProperty("--mosaic-measure-span", String(baseSpan));
    card.style.setProperty("top", "0");
    card.style.setProperty("height", "auto");
    track.append(card);
    plane.append(track);
    document.body.append(plane);

    const sample = () => {
      const cardStyle = getComputedStyle(card);
      const track = plane.querySelector(":scope > .lane-mosaic-track");
      const trackStyle = getComputedStyle(track);
      const box = card.getBoundingClientRect();
      const planeBox = plane.getBoundingClientRect();
      const gap = Number.parseFloat(trackStyle.columnGap);
      const trackWidth = (track.clientWidth - gap * 11) / 12;
      const committedSpan = Number(card.dataset.span);
      return {
        allowedSpan: committedSpan,
        columnEnd: cardStyle.gridColumnEnd,
        committedSpan,
        expectedWidth: trackWidth * committedSpan + gap * (committedSpan - 1),
        layoutPhase: card.dataset.layoutPhase,
        measureProperty: cardStyle.getPropertyValue("--mosaic-measure-span").trim(),
        measureSpan: Number(card.dataset.measureSpan),
        measurementActive: card.hasAttribute("data-mosaic-measurement-active"),
        paintable: cardStyle.display !== "none"
          && cardStyle.visibility !== "hidden"
          && Number.parseFloat(cardStyle.opacity) > 0
          && box.bottom > 0
          && box.right > 0
          && box.top < innerHeight
          && box.left < innerWidth
          && box.bottom > planeBox.top
          && box.right > planeBox.left
          && box.top < planeBox.bottom
          && box.left < planeBox.right,
        planePhase: plane.dataset.measurementPhase,
        planePending: Number(plane.dataset.measurementPending),
        width: box.width,
      };
    };
    const frame = () => new Promise((resolve) => requestAnimationFrame(() => resolve(sample())));
    card.setAttribute("data-mosaic-measurement-active", "true");
    card.setAttribute("data-measure-span", String(wideSpan));
    card.style.setProperty("--mosaic-measure-span", String(wideSpan));
    const held = await frame();
    card.removeAttribute("data-mosaic-measurement-active");
    card.setAttribute("data-measure-span", String(baseSpan));
    card.style.setProperty("--mosaic-measure-span", String(baseSpan));
    const restored = await frame();
    plane.remove();
    return { baseSpan, held, restored, wideSpan };
  });
}

export async function assertCandidateMeasurementPaintAtomic(page, engine) {
  assert.ok(EXPECTED_ENGINES.includes(engine), `candidate paint proof knows engine ${engine}`);
  const viewport = page.viewportSize();
  assert.ok(viewport, `${engine} candidate paint proof has a fixed viewport`);
  const alternate = { ...viewport, width: viewport.width - 48 };
  const armed = await armPaintProbe(page);
  await page.evaluate(() => new Promise((resolve) => requestAnimationFrame(resolve)));
  await page.setViewportSize(alternate);
  await waitForCommit(page, armed.commit, alternate.width);
  const alternateCommit = await page.locator(
    '[data-pane="agent-console"] .lane-mosaic-plane',
  ).getAttribute("data-plane-commits");
  await page.setViewportSize(viewport);
  await waitForCommit(page, Number(alternateCommit), viewport.width);
  const observed = await finishPaintProbe(page, armed);

  assert.ok(observed.retained, `${engine} retains the tall witness through both measurements`);
  assert.deepEqual(
    observed.candidateSpans,
    [armed.baseSpan, armed.wideSpan],
    `${engine} retains the distinct base and wide candidates`,
  );
  const spanMutations = observed.mutations.filter(
    (mutation) => mutation.attribute === "data-measure-span",
  );
  const spanTrail = [...spanMutations.map((mutation) => Number(mutation.oldValue)), armed.baseSpan];
  assert.ok(spanTrail.includes(armed.wideSpan), `${engine} observes the real wide span mutation`);
  assert.equal(spanTrail.at(-1), armed.baseSpan, `${engine} restores the base span mutation`);
  const propertyTrail = observed.mutations
    .filter((mutation) => mutation.attribute === "style")
    .map((mutation) => Number(mutation.oldMeasureProperty));
  assert.ok(propertyTrail.includes(armed.wideSpan), `${engine} observes the real wide CSS property`);
  assert.equal(observed.finalMeasureSpan, armed.baseSpan, `${engine} restores the span diagnostic`);
  assert.equal(
    Number(observed.finalMeasureProperty),
    armed.baseSpan,
    `${engine} restores the CSP-safe measurement property`,
  );
  const activeMutations = observed.mutations.filter(
    (mutation) => mutation.attribute === "data-mosaic-measurement-active",
  );
  assert.ok(
    activeMutations.some((mutation) => mutation.oldValue === null)
      && activeMutations.some((mutation) => mutation.oldValue === "true"),
    `${engine} observes measurement mode enter and exit`,
  );
  assert.ok(observed.samples.length >= 2, `${engine} samples paintable frame boundaries`);
  assert.ok(
    observed.samples.some((sample) => sample.paintable),
    `${engine} samples the live two-read card in a paintable frame`,
  );
  assert.deepEqual(
    measurementViolations(observed.samples),
    [],
    `${engine} paints no intermediate candidate geometry`,
  );

  const control = await negativeControl(page);
  const controlViolations = measurementViolations([control.held]);
  assert.ok(
    controlViolations.some((violation) => violation.reason === "measurement-active")
      && controlViolations.some((violation) => violation.reason === "intermediate-width"),
    `${engine} negative control detects a held intermediate candidate: ${JSON.stringify(control)}`,
  );
  assert.deepEqual(
    measurementViolations([control.restored]),
    [],
    `${engine} negative control restores its isolated card`,
  );
  completedEngines.add(engine);
}

export function assertCandidateMeasurementPaintEngineInventory() {
  assert.deepEqual(
    [...completedEngines].sort(),
    [...EXPECTED_ENGINES],
    "the Canvas interaction shard proves candidate paint atomicity in Chromium and Firefox",
  );
}
