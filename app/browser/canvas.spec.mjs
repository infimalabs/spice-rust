import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import { chromium } from "playwright";

const execute = promisify(execFile);
const here = dirname(fileURLToPath(import.meta.url));
const root = resolve(here, "../..");
const binary = join(root, "target/debug/spice");
const artifacts = join(root, "artifacts/canvas");

async function waitFor(predicate, description, timeout = 10_000) {
  const deadline = Date.now() + timeout;
  let lastError;
  while (Date.now() < deadline) {
    try {
      const value = await predicate();
      if (value) return value;
    } catch (error) {
      lastError = error;
    }
    await new Promise((resolveWait) => setTimeout(resolveWait, 50));
  }
  throw new Error(`timed out waiting for ${description}`, { cause: lastError });
}

async function startResident(fixture) {
  const child = (await import("node:child_process")).spawn(binary, ["--json", "up"], {
    cwd: fixture,
    stdio: ["ignore", "pipe", "pipe"],
  });
  let stderr = "";
  child.stderr.on("data", (chunk) => { stderr += chunk.toString(); });
  child.on("exit", (code) => {
    if (code && code !== 0) process.stderr.write(`resident exited ${code}: ${stderr}\n`);
  });
  const canvas = join(fixture, ".git/.spice-rust/canvas");
  const discovery = await waitFor(async () => {
    const bytes = await readFile(canvas, "utf8");
    return JSON.parse(bytes);
  }, "resident canvas discovery");
  return { child, discovery };
}

async function stopResident(fixture, resident) {
  await execute(binary, ["--json", "down"], { cwd: fixture, timeout: 10_000 });
  await waitFor(() => resident.child.exitCode !== null, "resident shutdown");
}

async function assertCanvas(page, scheme) {
  try {
    await page.waitForSelector(".status.online", { timeout: 10_000 });
  } catch (error) {
    const body = await page.locator("body").innerText().catch(() => "<body unavailable>");
    throw new Error(`canvas did not come online; body=${JSON.stringify(body)}`, { cause: error });
  }
  await page.waitForFunction(() => document.querySelector("#app")?.childElementCount > 0);
  await assert.doesNotReject(() => page.locator(".brand-name:visible, .mobile-brand:visible").waitFor());
  await assert.doesNotReject(() => page.getByRole("heading", { name: "Flow", exact: true }).waitFor());
  await page.waitForFunction(() => {
    const text = document.body.innerText;
    return text.includes("seated") && /JOURNAL\s+\d+/.test(text);
  });

  const pixels = await page.evaluate(() => {
    const root = document.querySelector("#app");
    const canvas = document.querySelector(".content");
    const rootStyle = root ? getComputedStyle(root) : null;
    const canvasBox = canvas?.getBoundingClientRect();
    const workspaceBox = document.querySelector(".workspace")?.getBoundingClientRect();
    const railBox = document.querySelector(".rail")?.getBoundingClientRect();
    return {
      appChildren: root?.childElementCount ?? 0,
      bodyWidth: document.body.scrollWidth,
      viewportWidth: window.innerWidth,
      canvasHeight: Math.round(canvasBox?.height ?? 0),
      ink: rootStyle?.color ?? "",
      background: getComputedStyle(document.body).backgroundColor,
      mobileNavOverlap: window.innerWidth <= 560
        && (workspaceBox?.bottom ?? 0) > (railBox?.top ?? window.innerHeight),
    };
  });
  assert.ok(pixels.appChildren > 0, "Wasm mounted a nonblank app");
  assert.ok(pixels.canvasHeight > 300, "primary canvas has visible content");
  assert.ok(pixels.bodyWidth <= pixels.viewportWidth, "page has no viewport overflow");
  assert.notEqual(pixels.ink, pixels.background, "foreground and background pixels differ");
  assert.equal(pixels.mobileNavOverlap, false, "mobile workspace stops above bottom navigation");

  for (const axis of ["Magnitude", "Topology", "Chronology", "Flow"]) {
    await page.getByRole("button", { name: axis }).click();
    await page.getByRole("heading", { name: axis, exact: true }).waitFor();
  }
  await page.keyboard.press("Tab");
  assert.ok(await page.locator(":focus-visible").count(), "keyboard focus remains visible");
  if ((page.viewportSize()?.width ?? 1000) <= 560) {
    await page.locator(".workspace").evaluate((element) => element.scrollTo(0, element.scrollHeight));
    assert.ok(await page.getByRole("button", { name: "Add note" }).isVisible());
    await page.locator(".workspace").evaluate((element) => element.scrollTo(0, 0));
  }
  await page.screenshot({ path: join(artifacts, `canvas-${scheme}.png`), fullPage: true });
}

const fixture = await mkdtemp(join(tmpdir(), "spice-canvas-browser-"));
await mkdir(join(fixture, ".git"));
await mkdir(artifacts, { recursive: true });

let resident;
let browser;
try {
  resident = await startResident(fixture);
  await execute(
    binary,
    ["--json", "task", "add", "Browser fixture task", "--project", "app.browser"],
    { cwd: fixture, timeout: 10_000 },
  );
  browser = await chromium.launch({ headless: true });

  const light = await browser.newContext({
    colorScheme: "light",
    viewport: { width: 1440, height: 900 },
  });
  const lightPage = await light.newPage();
  lightPage.on("console", (message) => process.stderr.write(`browser console: ${message.text()}\n`));
  lightPage.on("pageerror", (error) => process.stderr.write(`browser error: ${error.stack ?? error}\n`));
  lightPage.on("requestfailed", (request) => {
    process.stderr.write(`browser request failed: ${request.url()} ${request.failure()?.errorText}\n`);
  });
  await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await assertCanvas(lightPage, "light");
  const task = await lightPage.locator("#note-tasks option").first().getAttribute("value");
  assert.ok(task, "current task suggestion is available");
  await lightPage.getByLabel("Task").fill(task);
  await lightPage.getByLabel("Observation").fill("browser mutation reached the resident");
  await lightPage.getByRole("button", { name: "Add note" }).click();
  await lightPage.getByRole("button", { name: "Chronology" }).click();
  await lightPage.getByText(/Note appended to/).waitFor({ timeout: 10_000 });
  await lightPage.getByRole("button", { name: "Flow" }).click();
  const existingTasks = await lightPage.locator("#note-tasks option").evaluateAll(
    (options) => options.map((option) => option.value),
  );
  const absentTask = ["SPICE-aBcD", "SPICE-zYxW"].find(
    (candidate) => !existingTasks.includes(candidate),
  );
  assert.ok(absentTask, "an absent canonical task handle is available");
  await lightPage.getByLabel("Task").fill(absentTask);
  await lightPage.getByLabel("Observation").fill("this task does not exist");
  await lightPage.getByRole("button", { name: "Add note" }).click();
  await lightPage.getByRole("button", { name: "Chronology" }).click();
  await lightPage.locator(".event.refusal").waitFor({ timeout: 10_000 });
  await lightPage.getByRole("button", { name: "Flow" }).click();

  const dark = await browser.newContext({
    colorScheme: "dark",
    viewport: { width: 390, height: 844 },
  });
  const darkPage = await dark.newPage();
  await darkPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await assertCanvas(darkPage, "dark-narrow");

  const firstEpoch = resident.discovery.epoch;
  await stopResident(fixture, resident);
  await lightPage.waitForSelector(".status.offline", { timeout: 10_000 });
  await lightPage.getByRole("button", { name: "Chronology" }).click();
  await lightPage.locator(".event.fault").waitFor({ timeout: 10_000 });
  resident = await startResident(fixture);
  assert.notEqual(resident.discovery.epoch, firstEpoch, "restart publishes a new generation");
  await lightPage.goto(resident.discovery.endpoint, { waitUntil: "networkidle" });
  await lightPage.waitForSelector(".status.online", { timeout: 10_000 });

  await dark.close();
  await light.close();
} finally {
  if (browser) await browser.close();
  if (resident?.child.exitCode === null) resident.child.kill("SIGKILL");
  await rm(fixture, { recursive: true, force: true });
}
