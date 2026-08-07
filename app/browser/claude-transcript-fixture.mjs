export const sessionRecord = JSON.stringify({
  type: "system",
  subtype: "init",
  session_id: "018f7777-7777-7444-8888-999999999999",
});
export const activityRecord = JSON.stringify({
  type: "assistant",
  message: {
    type: "message",
    role: "assistant",
    content: [{ type: "text", text: "periodic live witness" }],
    usage: {
      input_tokens: 1,
      cache_read_input_tokens: 0,
      cache_creation_input_tokens: 0,
      output_tokens: 1,
    },
  },
});
export const triggeredActivityRecord = JSON.stringify({
  type: "assistant",
  message: {
    type: "message",
    role: "assistant",
    content: [{ type: "text", text: "newest lane prose witness" }],
    usage: {
      input_tokens: 1,
      cache_read_input_tokens: 0,
      cache_creation_input_tokens: 0,
      output_tokens: 1,
    },
  },
});

// The vendor correlation id joins the call to its result and is the exact
// string an operator must never be shown, so the fixture spells a realistic
// one rather than a friendly placeholder.
export const toolCallId = "toolu_01LaneStatusStripWitness";
/// The strip text production publishes for the tool call below.
///
/// Activity replay imports this through canvas-fixture.mjs, retaining the
/// shared fixture boundary while these records keep that file within shape.
export const TOOL_ACTIVITY_TEXT = "Bash: git status --short";
export const toolActivityRecord = JSON.stringify({
  type: "assistant",
  message: {
    type: "message",
    role: "assistant",
    content: [{
      type: "tool_use",
      id: toolCallId,
      name: "Bash",
      input: { command: "git status --short", description: "read the tree" },
    }],
    usage: {
      input_tokens: 1,
      cache_read_input_tokens: 0,
      cache_creation_input_tokens: 0,
      output_tokens: 1,
    },
  },
});
export const toolResultRecord = JSON.stringify({
  type: "user",
  message: {
    role: "user",
    content: [
      { type: "tool_result", tool_use_id: toolCallId, is_error: false, content: "M app/src/lib.rs" },
    ],
  },
});
