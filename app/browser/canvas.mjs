const DEFAULT_COLOR_SCHEME = "dark";

/**
 * Launch one disposable Playwright browser session for Canvas verification.
 *
 * The repository default matches the operator's dark Canvas. Pass an explicit
 * `contextOptions.colorScheme` only when verifying another theme.
 *
 * @param {import("playwright").BrowserType} browserType
 * @param {{
 *   launchOptions?: import("playwright").LaunchOptions,
 *   contextOptions?: import("playwright").BrowserContextOptions,
 * }} [options]
 */
export default async function (browserType, options = {}) {
  const browser = await browserType.launch({
    headless: true,
    ...options.launchOptions,
  });

  try {
    const context = await browser.newContext({
      colorScheme: DEFAULT_COLOR_SCHEME,
      ...options.contextOptions,
    });
    const page = await context.newPage();

    return {
      browser,
      context,
      page,
      close: () => browser.close(),
    };
  } catch (error) {
    await browser.close();
    throw error;
  }
}
