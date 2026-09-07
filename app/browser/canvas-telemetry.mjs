import assert from "node:assert/strict";
import { performance } from "node:perf_hooks";

const NAME = /^[a-z0-9]+(?:-[a-z0-9]+)*$/;
const OUTCOMES = ["passed", "failed"];

function named(value, label) {
  assert.equal(typeof value, "string", `${label} is text`);
  assert.match(value, NAME, `${label} uses stable kebab-case`);
  return value;
}

function timing(event, label) {
  for (const field of ["elapsedMs", "cpuMs"]) {
    assert.ok(
      Number.isSafeInteger(event[field]) && event[field] >= 0,
      `${label} ${field} is a nonnegative safe integer`,
    );
  }
}

function outcome(event, label) {
  assert.ok(OUTCOMES.includes(event.outcome), `${label} outcome is passed or failed`);
}

export function phaseName(name) {
  return named(name, "Canvas phase name");
}

export function startTiming() {
  return { wall: performance.now(), cpu: process.cpuUsage() };
}

export function elapsedTiming(started) {
  const used = process.cpuUsage(started.cpu);
  return {
    elapsedMs: Math.round(performance.now() - started.wall),
    cpuMs: Math.round((used.user + used.system) / 1_000),
  };
}

export function createTelemetryLedger(shard) {
  return {
    shard: named(shard, "Canvas telemetry shard"),
    currentScenario: null,
    phases: [],
    phaseStack: [],
    scenarios: [],
    summary: null,
  };
}

export function recordTelemetryEvent(ledger, event) {
  assert.ok(event && typeof event === "object" && !Array.isArray(event),
    "Canvas telemetry event is an object");
  assert.equal(ledger.summary, null, "Canvas shard summary is the terminal telemetry event");
  assert.equal(event.shard, ledger.shard, "Canvas telemetry stays inside its shard");
  switch (event.kind) {
    case "scenario-start": {
      const scenario = named(event.scenario, "Canvas scenario name");
      assert.equal(ledger.currentScenario, null, "Canvas scenarios do not nest");
      assert.deepEqual(ledger.phaseStack, [], "a scenario starts with no open phase");
      ledger.currentScenario = scenario;
      break;
    }
    case "phase-start": {
      const phase = named(event.phase, "Canvas phase name");
      assert.equal(event.scenario, ledger.currentScenario,
        "a Canvas phase belongs to the active scenario");
      const path = [...ledger.phaseStack.map((entry) => entry.phase), phase];
      assert.deepEqual(event.path, path, "a Canvas phase reports its exact nested path");
      ledger.phaseStack.push({ phase, path });
      break;
    }
    case "phase-end": {
      const phase = named(event.phase, "Canvas phase name");
      assert.equal(event.scenario, ledger.currentScenario,
        "a completed Canvas phase belongs to the active scenario");
      const active = ledger.phaseStack.at(-1);
      assert.ok(active, "a completed Canvas phase has an open phase");
      assert.equal(phase, active.phase, "Canvas phases close in LIFO order");
      assert.deepEqual(event.path, active.path, "a completed Canvas phase retains its path");
      timing(event, `Canvas phase ${event.path.join("/")}`);
      outcome(event, `Canvas phase ${event.path.join("/")}`);
      ledger.phaseStack.pop();
      ledger.phases.push(event);
      break;
    }
    case "scenario-end": {
      assert.equal(event.scenario, ledger.currentScenario,
        "a completed Canvas scenario is the active scenario");
      assert.deepEqual(ledger.phaseStack, [], "a Canvas scenario ends with no open phase");
      timing(event, `Canvas scenario ${event.scenario}`);
      outcome(event, `Canvas scenario ${event.scenario}`);
      ledger.scenarios.push(event);
      ledger.currentScenario = null;
      break;
    }
    case "summary":
      assert.equal(ledger.currentScenario, null, "Canvas summary follows every scenario");
      assert.deepEqual(ledger.phaseStack, [], "Canvas summary has no open phase");
      timing(event, `Canvas shard ${event.shard}`);
      ledger.summary = event;
      break;
    case "ownership":
      break;
    default:
      assert.fail(`unknown Canvas telemetry event kind: ${event.kind}`);
  }
}

export function assertTelemetryComplete(ledger) {
  assert.equal(ledger.currentScenario, null, "Canvas telemetry has no active scenario");
  assert.deepEqual(ledger.phaseStack, [], "Canvas telemetry has no open phase");
  assert.ok(ledger.summary, "Canvas telemetry has one shard summary");
  assert.ok(ledger.phases.every((event) => event.outcome === "passed"),
    "a successful Canvas shard has no failed phase");
  assert.ok(ledger.scenarios.every((event) => event.outcome === "passed"),
    "a successful Canvas shard has no failed scenario");
}
