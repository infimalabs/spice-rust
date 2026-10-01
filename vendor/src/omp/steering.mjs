// Spice writes this oh-my-pi extension beside each seat's configuration
// overlay and loads it with `--extension`.
//
// omp wraps every steered user message in a generated notice telling the
// model it is a priority interjection that supersedes prior instructions.
// Spice's steering already says what it is, with its own authenticated key,
// so the provider sees the message as Spice wrote it. The `context` hook
// changes only the copies omp hands the provider: queue delivery, the stored
// transcript, attachments, and the steering flag on the persisted message are
// left as they are.
export default function (pi) {
  pi.on("context", (event) => ({
    messages: event.messages.map((message) =>
      message.role === "user" && message.steering === true
        ? { ...message, steering: false }
        : message,
    ),
  }));
}
