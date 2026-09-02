// Browser error and request-failure accounting shared by Canvas scenarios.
import assert from "node:assert/strict";

export function monitorPage(page) {
  const pageErrors = [];
  const pageWarnings = [];
  const requestFailures = [];
  let expectedNetworkInterruption = false;
  page.on("pageerror", (error) => pageErrors.push(error.stack ?? error.toString()));
  page.on("console", (message) => {
    if (message.type() === "warning") {
      pageWarnings.push(`console: ${message.text()}`);
      return;
    }
    if (message.type() !== "error") return;
    if (
      expectedNetworkInterruption
      && /^Failed to load resource: net::ERR_CONNECTION_REFUSED$/.test(message.text())
    ) {
      return;
    }
    pageErrors.push(`console: ${message.text()}`);
  });
  page.on("requestfailed", (request) => {
    if (!expectedNetworkInterruption) {
      requestFailures.push(`${request.url()} ${request.failure()?.errorText ?? "unknown failure"}`);
    }
  });
  return {
    expectNetworkInterruption(value) {
      expectedNetworkInterruption = value;
    },
    evidence() {
      return {
        errors: [...pageErrors],
        requestFailures: [...requestFailures],
        warnings: [...pageWarnings],
      };
    },
    assertClean() {
      assert.deepEqual(pageErrors, [], "page emits no uncaught errors");
      assert.deepEqual(requestFailures, [], "page emits no unexpected request failures");
    },
  };
}
