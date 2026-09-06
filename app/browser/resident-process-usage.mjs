import { execFile } from "node:child_process";
import { performance } from "node:perf_hooks";
import { promisify } from "node:util";

const execute = promisify(execFile);
const SAMPLE_MS = 20;

function cpuMillis(value) {
  const fields = value.trim().split(":").map(Number);
  if (fields.some((field) => !Number.isFinite(field))) return 0;
  return fields.reduce((total, field) => total * 60 + field, 0) * 1_000;
}

async function readUsage(pid) {
  const { stdout } = await execute("ps", ["-o", "rss=", "-o", "time=", "-p", String(pid)]);
  const match = stdout.trim().match(/^(\d+)\s+(\S+)$/);
  if (!match) throw new Error(`resident usage is unreadable: ${JSON.stringify(stdout)}`);
  return { rssBytes: Number(match[1]) * 1_024, cpuMs: cpuMillis(match[2]) };
}

export function sampleResidentUsage(child) {
  const started = performance.now();
  let peakRssBytes = 0;
  let cpuMs = 0;
  let pending = null;
  const sample = () => {
    if (pending) return pending;
    pending = (async () => {
      const usage = await readUsage(child.pid);
      peakRssBytes = Math.max(peakRssBytes, usage.rssBytes);
      cpuMs = Math.max(cpuMs, usage.cpuMs);
    })().finally(() => { pending = null; });
    return pending;
  };
  sample();
  const timer = setInterval(sample, SAMPLE_MS);
  return async () => {
    clearInterval(timer);
    await pending;
    await sample();
    return { wallMs: performance.now() - started, peakRssBytes, cpuMs };
  };
}
