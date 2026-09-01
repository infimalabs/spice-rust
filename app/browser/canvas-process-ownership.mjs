// Canvas cleanup must inspect owned command paths without decoding unrelated
// host argv. A single invalid byte makes macOS pgrep refuse its whole scan.
import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { access } from "node:fs/promises";

function alive(pid) {
  try {
    process.kill(pid, 0);
    return true;
  } catch (error) {
    return error.code === "EPERM";
  }
}

async function absent(path) {
  try {
    await access(path);
    return false;
  } catch (error) {
    return error.code === "ENOENT";
  }
}

export function readProcessTable() {
  // Keep the snapshot as a Buffer: only the known UTF-8 path needles are encoded.
  return execFileSync("/bin/ps", ["-axww", "-o", "pid=,command="]);
}

function recordPid(record) {
  let start = 0;
  while (record[start] === 0x20 || record[start] === 0x09) start += 1;
  let end = start;
  while (record[end] >= 0x30 && record[end] <= 0x39) end += 1;
  if (start === end) return null;
  return Number(record.subarray(start, end).toString("ascii"));
}

export function processIdsMatchingPaths(table, paths) {
  const needles = paths.map((path) => Buffer.from(path));
  const matches = [];
  let start = 0;
  while (start < table.length) {
    const newline = table.indexOf(0x0a, start);
    const end = newline === -1 ? table.length : newline;
    const record = table.subarray(start, end);
    if (needles.some((needle) => record.includes(needle))) {
      const pid = recordPid(record);
      if (pid !== null) matches.push(pid);
    }
    start = end + 1;
  }
  return matches;
}

export async function auditOwnership(name, ownership) {
  const pids = ownership.pids;
  assert.deepEqual(pids.filter(alive), [], `${name} leaves no captured process alive`);
  assert.equal(await absent(ownership.fixture), true, `${name} removes its fixture`);
  assert.equal(await absent(ownership.linkedRoot), true, `${name} removes its lanes`);
  assert.deepEqual(
    processIdsMatchingPaths(readProcessTable(), [ownership.fixture, ownership.linkedRoot]),
    [],
    `${name} leaves no process matched by either exact fixture path`,
  );
  return pids;
}
