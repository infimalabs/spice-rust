# Canvas browser checks

## Browser evidence follows the host system theme

Use the repository-owned `launchCanvasBrowser` helper for disposable Canvas
browser checks. It detects the host appearance and gives the headless context
the matching `prefers-color-scheme`; callers do not repeat theme setup:

```js
import { chromium } from "playwright";
import launchCanvasBrowser from "./app/browser/canvas.mjs";

const canvas = await launchCanvasBrowser(chromium);
try {
  await canvas.page.goto(canvasUrl);
  // Inspect or capture the actual Canvas surface.
} finally {
  await canvas.close();
}
```

An intentional theme-matrix check overrides `contextOptions.colorScheme`.
Ordinary screenshots and visual checks retain the system default. The returned
`colorScheme` and `colorSchemeSource` report the selected value and whether it
came from `"system"` detection or `"explicit"` configuration. This browser
preference never changes an explicit Canvas preference: a stored
`spice:canvas-theme` still wins.

```js
const lightCanvas = await launchCanvasBrowser(chromium, {
  contextOptions: { colorScheme: "light" },
});
```

## Arrange whole teams

Pick up a team from its status-pip stack. After the same six-pixel lift
threshold used by composer movement, the source surface stays reserved and a
bounded ghost follows the pointer. The middle three fifths of another team
exchanges exactly those two team positions. Either outer fifth joins into the
team under the pointer: the receiver keeps its identity, primary composer, and
member order, then receives the source members in their existing order.

Interactive joins stop at six members. A seventh-member edge target is shown
as unavailable and sends no command. Press Enter or Space on the pip stack, or
tap it, for the equivalent **Swap positions** and **Join into** menu. Every
accepted interaction sends the complete topology at the revision currently
drawn by Canvas; cancellation, a changed revision, or a removed source clears
the lift without sending.

## Lane menu

Each Chat lane's menu button swaps that lane's composer for its seat panel
instead of opening a floating menu. The panel carries everything one agent
offers. It leads with what the agent needs from the operator now: consent
requests to Allow or Deny in place, held tasks iced, blocked, or in review,
a lease within two minutes of expiry, and direction waiting over five
minutes. Then come the Wake, Drain, Remove, Renew, and next-launch-model
commands and refusals, the presence and lease age, the current task, latest
activity, and waiting direction, copyable agent channel, occupant, session,
and worktree, and Create new team and Leave all teams. Wide lanes lay those
groups side by side; narrow lanes stack them and scroll within the lane. The
button, Escape, or a click elsewhere in the team restores the composer.

## Rich team directions

The team-local **Work** tab renders each direction from its complete admitted
body. Markdown links, internal Canvas routes, quotes, diagrams, and image
references use the same sanitized renderer and image viewer as Chat. Embedded
images resolve from their admitted bytes, keep their authored names and
alternatives, and are not repeated as loose attachments.

The recent retired-direction window carries that same body and media, so a
recipient moving from delivered to acknowledged, declined, lapsed,
dead-lettered, or canceled changes only its lifecycle badge. Retirement does
not replace the card with an attachment-free preview. Direction identity,
attribution, task chips, return links, and age remain stable.

## Card endings and relative ages

Invisible renderer portal hosts do not add trailing space. A card whose final
visible Markdown block precedes that host therefore keeps the same visual
bottom inset as its sides, including when its producer and age share the final
prose line.

A final line of text (paragraph, list, heading, or signoff line) keeps its
descent and leading below the baseline, so that ending settles 3px into the
card's bottom padding to read as near the edge as the side inset. Image stacks,
code, tables, and quotes do not move. The card's faces clip at its inner bottom
edge rather than at the content box, so the settled line is never cut, and
measurement and committed card sizes do not see the shift.

A signed prose card keeps its agent attribution in the lane accent, but its
linked top-level relative age is neutral. The age remains an underlined card
permalink and advances on its own visible-unit deadlines. Authored Markdown
links retain the ordinary accent, and quoted-source ages keep their separate
quote presentation.

## Profile Canvas latency in Firefox

Canvas exposes opt-in User Timing diagnostics without changing route parsing.
Open the ordinary Canvas URL, run
`sessionStorage.setItem("spice:canvas-trace", "1"); location.reload()` in its
developer console, and leave that tab open for the capture. A traced page
installs `window.__spiceCanvasTrace`; an ordinary page does not attach the
native input listeners or create timing entries.

Build the current browser body with `spice dev xtask -- app` and serve that
generation through the development Canvas. Then capture two short runs of the
same interaction:

1. Open `about:profiling` in Firefox, select the **Web Developer** preset, and
   start the profiler.
2. In the traced Canvas tab, run `window.__spiceCanvasTrace.clear()` in the
   developer console immediately before the scenario.
3. Perform one fixed scenario: use the same loaded board, focus the same
   composer, enter or dictate the same text, and trigger the same resident
   update. Keep the capture short enough that the entire interaction is easy to
   inspect.
4. For the loaded comparison, start
   `spice dev cargo -- check --locked -p spice` in another terminal and perform
   the scenario while that real compile is active. Stop and save the profile.
5. Wait for the command and all compiler processes to exit. Clear the Canvas
   trace, repeat the identical scenario without compilation, then stop and save
   the settled profile.
6. In each Firefox profile, use the Marker Chart and filter for
   `spice.canvas.`. Compare those markers on the Canvas content main thread
   against DOM events, task scheduling gaps, style recalculation, layout, and
   paint. Retain the profile URLs or exported profiles with the investigation
   notes.

Marker names carry only bounded structural metadata:

| Marker | Interpretation |
| --- | --- |
| `trace.start` | App source/build identity for the capture. |
| `frame.received`, `frame.decode.*` | Incoming frame size and browser-side decode/admission work. |
| `state.apply.*`, `state.publish` | Runtime mutation and the Yew publication that schedules rendering. |
| `render.canvas` | Rust/Yew virtual-DOM construction with the retained card count. |
| `layout.measure` | Mosaic DOM measurement, including card count and measured width. |
| `composer.textarea.mount`, `composer.textarea.unmount` | Native textarea instance lifetime, focus, and selection. A changed instance during one interaction is a remount. |
| `composer.input`, `composer.keydown` | Native event kind, composition state, focus, and selection without draft text. |

The browser retains at most 256 Canvas diagnostic records; completed spans use
one measure and two marks, and the oldest record is removed as the bound rolls.
`window.__spiceCanvasTrace.clear()` removes only entries owned by the current
Canvas trace. Run
`sessionStorage.removeItem("spice:canvas-trace"); location.reload()` to disable
collection.

Interpret ordering conservatively. `render.canvas` ends when Rust finishes
building Yew nodes, not when the DOM is committed or painted.
`layout.measure` covers synchronous Mosaic DOM reads and writes, but a later
paint must be established from Firefox's own markers. A DOM `beforeinput` or
`input` marker is not the start of speech recognition. The trace intentionally
has no animation-frame marker: an animation-frame callback would show callback
eligibility, not presentation.
