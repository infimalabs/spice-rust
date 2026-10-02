import { spawnSync } from "node:child_process";

function run(command, args) {
  const result = spawnSync(command, args, { encoding: "utf8" });
  if (result.error) throw result.error;
  return result;
}

export function systemColorScheme(platform = process.platform) {
  if (platform === "darwin") {
    const result = run("/usr/bin/defaults", [
      "read",
      "-g",
      "AppleInterfaceStyle",
    ]);
    if (result.status === 0) {
      return result.stdout.trim().toLowerCase() === "dark" ? "dark" : "light";
    }
    if (result.status === 1) return "light";
  } else if (platform === "win32") {
    const result = run("reg.exe", [
      "query",
      String.raw`HKCU\Software\Microsoft\Windows\CurrentVersion\Themes\Personalize`,
      "/v",
      "AppsUseLightTheme",
    ]);
    if (result.status === 0) {
      return /\b0x0\b/i.test(result.stdout) ? "dark" : "light";
    }
  } else if (platform === "linux") {
    const result = run("gsettings", [
      "get",
      "org.gnome.desktop.interface",
      "color-scheme",
    ]);
    if (result.status === 0) {
      return result.stdout.toLowerCase().includes("dark") ? "dark" : "light";
    }
  }

  throw new Error(
    `Cannot detect the ${platform} system theme; set contextOptions.colorScheme explicitly`,
  );
}

/**
 * Launch one disposable Playwright browser session for app verification.
 *
 * The default color scheme follows the host system. Pass an explicit
 * `contextOptions.colorScheme` only when intentionally verifying one theme.
 *
 * @param {import("playwright").BrowserType} browserType
 * @param {{
 *   launchOptions?: import("playwright").LaunchOptions,
 *   contextOptions?: import("playwright").BrowserContextOptions,
 * }} [options]
 */
export default async function (browserType, options = {}) {
  const { colorScheme: configuredColorScheme, ...contextOptions } =
    options.contextOptions ?? {};
  const colorScheme = configuredColorScheme ?? systemColorScheme();
  const colorSchemeSource =
    configuredColorScheme === undefined ? "system" : "explicit";
  const browser = await browserType.launch({
    headless: true,
    ...options.launchOptions,
  });

  try {
    const context = await browser.newContext({
      ...contextOptions,
      colorScheme,
    });
    const page = await context.newPage();

    return {
      browser,
      context,
      page,
      colorScheme,
      colorSchemeSource,
      close: () => browser.close(),
    };
  } catch (error) {
    await browser.close();
    throw error;
  }
}
