/// A socket that stays open and stops delivering, proven end to end.
///
/// Every other offline case in this suite is produced by something the page can
/// observe: the fixture closes the socket, or the resident exits. Neither can
/// detect this one, because the whole hazard is that nothing arrives to be
/// observed -- the socket is open, the reader is parked, and no close is coming.
/// So the traffic is withheld at the route rather than by stopping anything, and
/// the connection is left intact underneath it.
///
/// The route is also the instrument. Counting frames in each direction is what
/// separates "the client noticed" from "the client gave up": a client that only
/// timed out would send nothing during the silence, so the probe arriving at the
/// server is the evidence that the exchange this test is named for happened on
/// the wire and not just in the status text.
import assert from "node:assert/strict";

import {
  CONNECT_MS,
  HALF_OPEN_DETECT_MS,
  RECONNECT_MS,
  assertGenerationOnline,
  chooseCanvasView,
  observedBoundedWait,
} from "./canvas-fixture.mjs";

/// Intercept the canvas socket, forwarding both ways until told to stop.
///
/// Client-to-server always forwards, including while inbound is withheld. That
/// asymmetry is the point: the probe has to reach a resident that is perfectly
/// healthy and answers it, and the answer is what gets dropped. Withholding both
/// directions would instead test a client talking to nothing, which a plain
/// close already covers.
export async function routeCanvasSocket(page) {
  const connections = [];
  let withheld = false;
  await page.routeWebSocket(/\/socket\?/, (ws) => {
    const server = ws.connectToServer();
    const record = { toServer: 0, toClient: 0, withheld: 0 };
    connections.push(record);
    ws.onMessage((message) => {
      record.toServer += 1;
      server.send(message);
    });
    server.onMessage((message) => {
      if (withheld) {
        record.withheld += 1;
        return;
      }
      record.toClient += 1;
      ws.send(message);
    });
  });
  return {
    connections,
    withhold(value) {
      withheld = value;
    },
  };
}

/// Prove a half-open socket converges through the one reconnect path.
///
/// `duringOutage` runs while the page is offline and the resident is not, which
/// is the only window in which authority can advance past the client's cursor.
/// Without it the resumed stream would have nothing to resume and could carry
/// the cursor forward vacuously.
export async function assertHalfOpenSocketRecovers(page, monitor, route, epoch, duringOutage) {
  await chooseCanvasView(page, "Chronology");
  // Establishing first is what makes the count below mean something: a page that
  // had not connected yet would also report one socket, or none.
  await assertGenerationOnline(page, epoch, "the page connects", CONNECT_MS);
  assert.equal(route.connections.length, 1, "the page opened exactly one socket to begin with");
  const [stalled] = route.connections;
  const deliveredBefore = stalled.toClient;
  const sentBefore = stalled.toServer;

  monitor.expectNetworkInterruption(true);
  route.withhold(true);
  // Advance authority while the socket is still open and still being dropped, so
  // the fact the page must later resume is one the resident genuinely tried to
  // deliver to it. Doing this after the offline flip instead would race the
  // client's retry, which arms within the base delay and would then hand the
  // replacement socket a withheld handshake and a nondeterministic socket count.
  await duringOutage();
  await observedBoundedWait(
    page,
    "canvas reports offline after inbound frames are withheld without a close",
    HALF_OPEN_DETECT_MS,
    () => page.waitForSelector(".status.offline", { timeout: HALF_OPEN_DETECT_MS }),
  );
  // Restored before anything else is read. The counters below are already
  // recorded, but the client's next attempt is not: it arms on the base delay,
  // so any assertion placed ahead of this would be spending that budget.
  route.withhold(false);

  // The socket was never closed from either end, so an offline status here can
  // only have come from the client deciding the connection was dead.
  assert.ok(
    stalled.toServer > sentBefore,
    "the client spoke on the stalled socket rather than only waiting on it",
  );
  assert.equal(
    stalled.toClient,
    deliveredBefore,
    "nothing reached the client while inbound was withheld",
  );
  assert.ok(stalled.withheld > 0, "the resident kept serving a socket it still considered open");

  await assertGenerationOnline(page, epoch, "inbound frames resume", RECONNECT_MS);
  monitor.expectNetworkInterruption(false);

  // One, not two. A second reconnect entry point would show up right here, as an
  // extra socket racing the first and subscribing a second time.
  assert.equal(
    route.connections.length,
    2,
    "recovery opened exactly one replacement socket, so there is one reconnect path",
  );
  assert.ok(
    route.connections[1].toClient > 0,
    "the replacement socket carries inbound traffic",
  );
  return route.connections[1];
}
