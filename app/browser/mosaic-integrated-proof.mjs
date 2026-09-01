import assert from "node:assert/strict";
import { writeFile } from "node:fs/promises";
import { join } from "node:path";

import { assertMosaicLattice } from "./agent-conversation-mosaic.mjs";
import { assertAgentWorkspaceGeometry } from "./canvas-readiness.mjs";

export async function assertIntegratedMosaicProof(
  page,
  team,
  artifacts,
  expectedLabels,
  hydration,
) {
  const native = await page.evaluate(() => ({
    devicePixelRatio,
    height: innerHeight,
    width: innerWidth,
  }));
  assert.deepEqual(
    { width: native.width, height: native.height },
    { width: 2018, height: 1728 },
    "the integrated Mosaic proof uses the reported native viewport",
  );

  const placements = await assertMosaicLattice(page, true);
  const workspace = await assertAgentWorkspaceGeometry(page, "integrated 2018x1728 Mosaic");
  const cards = await team.locator(".mosaic-message-card").evaluateAll((elements) => (
    elements.map((card) => {
      const box = card.getBoundingClientRect();
      const footer = card.querySelector(".message-footer");
      const footerRight = card.querySelector(".message-footer-right");
      const agent = card.querySelector(".message-agent-name");
      const age = card.querySelector(".message-age");
      const ageBox = age?.getBoundingClientRect();
      const footerBox = footer?.getBoundingClientRect();
      const footerRightBox = footerRight?.getBoundingClientRect();
      const agentBox = agent?.getBoundingClientRect();
      return {
        key: card.dataset.mosaicCardKey,
        kind: card.dataset.cardKind,
        lane: card.dataset.agentLane,
        box: { bottom: box.bottom, height: box.height, left: box.left, right: box.right, top: box.top },
        clientHeight: card.clientHeight,
        scrollHeight: card.scrollHeight,
        footer: footer ? {
          height: footerBox.height,
          clientWidth: footer.clientWidth,
          scrollWidth: footer.scrollWidth,
          top: footerBox.top,
          bottom: footerBox.bottom,
          right: {
            bottom: footerRightBox.bottom,
            flexWrap: getComputedStyle(footerRight).flexWrap,
            height: footerRightBox.height,
            top: footerRightBox.top,
          },
        } : null,
        agent: agent ? {
          bottom: agentBox.bottom,
          clientHeight: agent.clientHeight,
          clientWidth: agent.clientWidth,
          overflow: getComputedStyle(agent).overflow,
          scrollHeight: agent.scrollHeight,
          scrollWidth: agent.scrollWidth,
          text: agent.textContent.trim(),
          textOverflow: getComputedStyle(agent).textOverflow,
          top: agentBox.top,
          whiteSpace: getComputedStyle(agent).whiteSpace,
        } : null,
        age: ageBox ? { height: ageBox.height, width: ageBox.width } : null,
        actions: [...card.querySelectorAll(".message-actions button")].map((button) => {
          const bounds = button.getBoundingClientRect();
          return {
            ariaLabel: button.getAttribute("aria-label"),
            height: bounds.height,
            width: bounds.width,
          };
        }),
      };
    })
  ));

  assert.equal(cards.length, hydration.oneShot.cards.length, "the report contains every hydrated card");
  assert.ok(cards.length >= 40, "the integrated proof exercises a large transcript");
  assert.equal(cards.filter((card) => card.kind === "compaction").length, 1,
    "the integrated transcript retains one compaction boundary");
  assert.ok(cards.every((card) => card.clientHeight + 1 >= card.scrollHeight),
    "every integrated card contains its content");
  const agentCards = cards.filter((card) => card.kind === "agent-message");
  assert.ok(agentCards.every((card) => (
    card.footer.scrollWidth <= card.footer.clientWidth + 1
      && card.footer.height === 32
      && card.footer.right.height === 32
      && card.footer.right.flexWrap === "nowrap"
      && card.footer.right.top >= card.footer.top
      && card.footer.right.bottom <= card.footer.bottom
      && card.agent.top >= card.footer.top
      && card.agent.bottom <= card.footer.bottom
      && card.agent.overflow === "hidden"
      && card.agent.textOverflow === "ellipsis"
      && card.agent.whiteSpace === "nowrap"
  )), "integrated footers retain one truncating row without overlap or overflow");
  for (const label of expectedLabels) {
    assert.ok(agentCards.some((card) => card.agent.text === label),
      `the integrated artifact includes the complete ${label} identity`);
  }
  assert.equal(new Set(agentCards.map((card) => card.age.width)).size, 1,
    "integrated message ages retain one fixed-width slot");
  assert.ok(agentCards.every((card) => (
    card.actions.length === 3
      && card.actions.every((action) => action.width === 32 && action.height === 32)
  )), "integrated message actions retain their 32px targets");
  assert.ok(workspace.fixed, "the tall desktop report uses the bounded viewport owner");
  assert.ok(workspace.surfaces.some((surface) => (
    surface.plane.scrollHeight > surface.plane.clientHeight
  )), "large history scrolls inside a Mosaic plane");

  const report = {
    native,
    hydration,
    workspace,
    placements,
    cards,
  };
  await writeFile(
    join(artifacts, "canvas-mosaic-integrated-2018x1728.json"),
    `${JSON.stringify(report, null, 2)}\n`,
  );
  await page.screenshot({
    path: join(artifacts, "canvas-mosaic-integrated-2018x1728.png"),
    fullPage: true,
  });
}
