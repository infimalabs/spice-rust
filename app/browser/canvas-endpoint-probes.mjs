// Endpoint diagnostics and their executable TCP fixtures for the Canvas suite.
import assert from "node:assert/strict";
import { once } from "node:events";
import { connect, createServer } from "node:net";

/// The page-side connection state, read from the topbar that carries it.
///
/// The class names Online against Offline and the label carries the reason the
/// client last recorded, so a page that never saw the socket close reads
/// differently from one that saw it and did not render the change.
///
/// The status element is NOT always present. The topbar holding it is rendered
/// only outside the mosaic workspace, so a wait on a status class is deaf while
/// the mosaic is active. An absent status therefore reports the layout that
/// hides it rather than a bare false, because the two are opposite diagnoses:
/// one is a client that never changed state, the other is a client whose state
/// is simply not on screen to be waited for.
export async function describeConnection(page) {
  return await page
    .evaluate(() => {
      const status = document.querySelector(".status");
      if (!status) {
        return {
          rendered: false,
          topbar: Boolean(document.querySelector(".topbar")),
          mosaic: Boolean(document.querySelector(".mosaic-workspace")),
        };
      }
      return {
        rendered: true,
        classes: status.className,
        label: status.querySelector(".status-copy")?.textContent ?? null,
      };
    })
    .catch((error) => ({ unavailable: String(error) }));
}

/// Whether the resident is gone, and whether anything still answers where it
/// used to listen.
///
/// These are separate questions. A listening socket that outlives the exited
/// process leaves the client connecting to something that never replies, which
/// is a different diagnosis from a refused connection and must not be reported
/// as the same silence.
export async function describeResident(resident, probeTimeout = 2_000) {
  if (!resident) return { known: false };
  const endpoint = resident.discovery?.endpoint ?? null;
  return {
    known: true,
    exited: resident.child.exitCode !== null || resident.child.signalCode !== null,
    exitCode: resident.child.exitCode,
    signal: resident.child.signalCode,
    endpoint,
    endpointState: await probeEndpoint(endpoint, probeTimeout),
  };
}

/// Ask the endpoint for a connection and one minimal HTTP answer.
///
/// A completed TCP handshake proves only that something still owns the port.
/// The stale-listener diagnosis is exactly a socket that accepts that handshake
/// and never answers, so only response bytes establish the healthy accepting
/// state; silence reaches the distinct timeout answer.
function probeEndpoint(endpoint, timeout = 2_000) {
  const target = parseEndpoint(endpoint);
  if (!target) return Promise.resolve("unknown");
  return new Promise((done) => {
    const socket = connect({ host: target.host, port: target.port });
    let settled = false;
    const settle = (outcome) => {
      if (settled) return;
      settled = true;
      socket.destroy();
      done(outcome);
    };
    socket.setTimeout(timeout);
    socket.on("connect", () => {
      socket.write(
        `GET / HTTP/1.1\r\nHost: ${target.host}:${target.port}\r\nConnection: close\r\n\r\n`,
      );
    });
    socket.on("data", () => settle("accepting"));
    socket.on("end", () => settle("accepted and closed before a response"));
    socket.on("timeout", () => settle("accepted nothing before the probe timed out"));
    socket.on("error", (error) => settle(`refused: ${error.code ?? error.message}`));
  });
}

function parseEndpoint(endpoint) {
  if (!endpoint) return null;
  try {
    const url = new URL(endpoint);
    return { host: url.hostname, port: Number(url.port) };
  } catch {
    return null;
  }
}

async function endpointFixture(onConnection = () => {}) {
  const sockets = new Set();
  const server = createServer((socket) => {
    sockets.add(socket);
    socket.on("close", () => sockets.delete(socket));
    onConnection(socket);
  });
  server.listen(0, "127.0.0.1");
  await once(server, "listening");
  const address = server.address();
  assert.ok(address && typeof address === "object", "endpoint fixture has a TCP address");
  return {
    endpoint: `http://127.0.0.1:${address.port}`,
    async close() {
      for (const socket of sockets) socket.destroy();
      await new Promise((resolveClose, rejectClose) => {
        server.close((error) => error ? rejectClose(error) : resolveClose());
      });
    },
  };
}

function residentAt(endpoint) {
  return {
    child: { exitCode: null, signalCode: null },
    discovery: { endpoint },
  };
}

/// A peer can accept the TCP connection and close cleanly without answering.
///
/// Once the peer sends FIN, the socket no longer generates an inactivity
/// timeout. Keep this fixture under its own outer bound so removing the `end`
/// settlement fails by name instead of hanging the rest of the Canvas suite.
export async function assertEndpointProbeClosesWithoutResponse() {
  const closingFixture = await endpointFixture((socket) => socket.end());
  let deadline;
  try {
    const closed = await Promise.race([
      describeResident(residentAt(closingFixture.endpoint), 100),
      new Promise((_, reject) => {
        deadline = setTimeout(
          () => reject(new Error("the accepted-and-closed endpoint probe did not settle")),
          250,
        );
      }),
    ]);
    assert.equal(
      closed.endpointState,
      "accepted and closed before a response",
      "an accepted socket that ends without bytes reports its own bounded state",
    );
    assert.notEqual(closed.endpointState, "accepting");
    assert.notEqual(closed.endpointState, "accepted nothing before the probe timed out");
    assert.doesNotMatch(closed.endpointState, /^refused:/);
  } finally {
    clearTimeout(deadline);
    await closingFixture.close();
  }
}

/// Drive every endpoint report answer against a real TCP endpoint.
///
/// The refused fixture reserves and closes an ephemeral port before probing it.
/// The healthy fixture answers the minimal request, while the stale fixture
/// accepts and deliberately stays silent. Their reports must remain pairwise
/// distinct so `accepting` cannot impersonate either failure cause.
export async function assertEndpointProbeStates() {
  const refusedFixture = await endpointFixture();
  const refusedEndpoint = refusedFixture.endpoint;
  await refusedFixture.close();
  const refused = await describeResident(residentAt(refusedEndpoint), 250);
  assert.match(
    refused.endpointState,
    /^refused: E[A-Z]+$/,
    "a port with no listener reports refusal and its errno",
  );

  const answeringFixture = await endpointFixture((socket) => {
    socket.once("data", () => {
      socket.end("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
    });
  });
  const accepting = await describeResident(residentAt(answeringFixture.endpoint), 250);
  await answeringFixture.close();
  assert.equal(
    accepting.endpointState,
    "accepting",
    "an endpoint that accepts and answers reports healthy",
  );

  const silentFixture = await endpointFixture();
  const silent = await describeResident(residentAt(silentFixture.endpoint), 100);
  await silentFixture.close();
  assert.equal(
    silent.endpointState,
    "accepted nothing before the probe timed out",
    "an endpoint that accepts but never answers reports the stale-listener state",
  );
  assert.notEqual(silent.endpointState, accepting.endpointState);
  assert.notEqual(silent.endpointState, refused.endpointState);
}
