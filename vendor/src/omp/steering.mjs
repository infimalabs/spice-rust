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
//
// Background notices replace one another by purpose while a step runs. The
// SDK aside queue delivers their single batch before the next model step.
export default function (pi) {
  // RPC exposes steer/follow-up but not aside. This command bridges that one
  // SDK operation. Replacements remain local until the next step boundary.
  const pending = new Map();
  const flush = async () => {
    const messages = [...pending.values()];
    pending.clear();
    if (messages.length) {
      await pi.sendUserMessage(messages.join("\n\n"), { deliverAs: "aside" });
    }
  };
  pi.registerCommand("spice-aside", {
    description: "Deliver a Spice background notice at the next step boundary",
    handler: async (args, ctx) => {
      const { purpose, message } = JSON.parse(args);
      if (typeof purpose !== "string" || !purpose || typeof message !== "string") {
        throw new Error("spice-aside requires a purpose and a message");
      }
      if (message) pending.set(purpose, message);
      else pending.delete(purpose);
      if (ctx.isIdle()) await flush();
    },
  });
  pi.on("turn_end", flush);
  pi.on("agent_end", flush);
  pi.on("context", (event) => ({
    messages: event.messages.map((message) =>
      message.role === "user" && message.steering === true
        ? { ...message, steering: false }
        : message,
    ),
  }));
}
