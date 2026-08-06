// The one place this repository opens a browser, and the guard that keeps it
// the only one. Playwright launches headed unless told otherwise, so the way a
// Chromium window lands on the operator's desktop and takes their keyboard
// focus mid-session is a launch site that omits the option -- not one that sets
// it wrongly. An omission leaves nothing to read, which is why the rule here is
// stated over launch sites rather than over their arguments.
import assert from "node:assert/strict";
import { execFile } from "node:child_process";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { promisify } from "node:util";

import { chromium } from "playwright";

const execute = promisify(execFile);
const root = resolve(dirname(fileURLToPath(import.meta.url)), "../..");

/// This module's own repository path: the sole tracked file allowed to spell a
/// browser launch, which means the scan below has to recognise itself.
const OWNER = "app/browser/headless-launch.mjs";

/// Every spelling that opens a browser, across all three Playwright engines and
/// both launch shapes. Fixed strings rather than patterns, so a dot is a dot.
const LAUNCH_SPELLINGS = [
  "chromium.launch(",
  "firefox.launch(",
  "webkit.launch(",
  "browserType.launch(",
  ".launchPersistentContext(",
  ".launchServer(",
];

/// The Playwright MCP server opens a browser on an agent's behalf and defaults
/// to headed as well, so a harness that shells out to it through npm exec is a
/// launch site too. Every tracked configuration spells the package and its
/// argument list on one line, so the flag is required on that same line.
const MCP_SPELLING = "@playwright/mcp";
const HEADLESS_FLAG = "--headless";

/// How the library spells the same thing, which is what a launch inside this
/// module has to carry.
const HEADLESS_OPTION = "headless: true";

/// Chromium states its build in the user agent: a headed browser reports
/// `Chrome/<version>`, a headless one `HeadlessChrome/<version>`. Reading that
/// back from the running browser proves the option arrived; reading the source
/// only proves it was written.
const HEADLESS_USER_AGENT = "HeadlessChrome/";

/// Open the browser every fixture and diagnostic script shares.
export async function launchBrowser() {
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext();
  try {
    const page = await context.newPage();
    const agent = await page.evaluate(() => navigator.userAgent);
    assert.ok(
      agent.includes(HEADLESS_USER_AGENT),
      `the launched browser is headless and takes no operator focus: ${agent}`,
    );
  } catch (failure) {
    await browser.close();
    throw failure;
  }
  await context.close();
  return browser;
}

/// Fail when a tracked file other than this one opens a browser, when a launch
/// inside this one omits the option, or when a tracked Playwright MCP
/// configuration omits the flag.
export async function assertHeadlessLaunchSites() {
  const hits = await trackedLaunchSites();
  assert.ok(
    hits.some(({ path }) => path === OWNER),
    `the scan reaches ${OWNER}; a scan that matches nothing satisfies every rule below`,
  );
  for (const { path, line, text } of hits) {
    const site = `${path}:${line}`;
    if (path === OWNER) {
      // This module spells every launch the scan looks for, so it matches
      // itself. Those quoted spellings are the vocabulary; any other hit here
      // is a real call, and exempting the file wholesale would let a second,
      // headed one in through the one door the scan cannot see past.
      assert.ok(
        spellsVocabulary(text) || text.includes(HEADLESS_OPTION),
        `${site} is a launch inside the owner and passes ${HEADLESS_OPTION}: ${text.trim()}`,
      );
      continue;
    }
    if (text.includes(MCP_SPELLING)) {
      assert.ok(
        text.includes(HEADLESS_FLAG),
        `${site} configures the Playwright MCP browser headless: ${text.trim()}`,
      );
      continue;
    }
    assert.fail(
      `${site} opens a browser outside ${OWNER}, the only launch this suite proves headless: ${text.trim()}`,
    );
  }
}

/// Whether a line merely quotes one of the spellings the scan searches for.
function spellsVocabulary(text) {
  return [...LAUNCH_SPELLINGS, MCP_SPELLING].some((spelling) => text.includes(`"${spelling}"`));
}

/// Every tracked line spelling a browser launch or the MCP browser server.
///
/// The tracked set is asked of git rather than read off the filesystem, so an
/// operator's untracked scratch script is not mistaken for a launch site this
/// repository ships.
async function trackedLaunchSites() {
  const patterns = [...LAUNCH_SPELLINGS, MCP_SPELLING].flatMap((spelling) => ["-e", spelling]);
  const found = await execute("git", ["grep", "-I", "-n", "-F", ...patterns], { cwd: root }).catch(
    (failure) => {
      assert.fail(`the tracked launch-site scan ran: ${failure.message}`);
    },
  );
  return found.stdout
    .split("\n")
    .filter((hit) => hit.length > 0)
    .map((hit) => {
      const [path, line, ...rest] = hit.split(":");
      return { path, line, text: rest.join(":") };
    });
}
