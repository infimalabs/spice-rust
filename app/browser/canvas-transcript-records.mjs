// Exact fake-vendor records shared by the Canvas resident fixture scenarios.
export const sessionRecord = JSON.stringify({
  type: "system",
  subtype: "init",
  session_id: "018f7777-7777-7444-8888-999999999999",
});

function assistantRecord(content) {
  return JSON.stringify({
    type: "assistant",
    message: {
      type: "message",
      role: "assistant",
      content,
      usage: {
        input_tokens: 1,
        cache_read_input_tokens: 0,
        cache_creation_input_tokens: 0,
        output_tokens: 1,
      },
    },
  });
}

export const activityRecord = assistantRecord([
  { type: "text", text: "periodic live witness" },
]);
export const triggeredActivityRecord = assistantRecord([
  { type: "text", text: "newest lane prose witness" },
]);

// The vendor correlation id joins the call to its result and is the exact
// string an operator must never be shown, so the fixture spells a realistic
// one rather than a friendly placeholder.
export const toolCallId = "toolu_01LaneStatusStripWitness";
/// The one strip text both the Canvas fixture and activity replay must match.
///
/// Keeping it beside the source record makes a preview-rule change red one
/// constant instead of leaving a hand-copied replay matcher capturing nothing.
export const TOOL_ACTIVITY_TEXT = "Bash: git status --short";
export const toolActivityRecord = assistantRecord([
  {
    type: "tool_use",
    id: toolCallId,
    name: "Bash",
    input: { command: "git status --short", description: "read the tree" },
  },
]);
export const toolResultRecord = JSON.stringify({
  type: "user",
  message: {
    role: "user",
    content: [
      { type: "tool_result", tool_use_id: toolCallId, is_error: false, content: "M app/src/lib.rs" },
    ],
  },
});
