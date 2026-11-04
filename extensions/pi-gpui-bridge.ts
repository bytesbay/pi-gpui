/**
 * Pi GPUI bridge.
 *
 * pi's RPC protocol has no command to move within the session tree, but
 * extension commands can (`ctx.navigateTree`). Pi GPUI loads this file with
 * `pi --mode rpc -e <path>` for its own pi processes only; nothing is
 * installed. The app sends `/pi-gpui-navigate <entryId> [summarize]` as a
 * prompt and reloads the transcript when pi answers "handled".
 */
export default function (pi: any) {
	pi.registerCommand("pi-gpui-navigate", {
		description: "Internal: used by Pi GPUI to move within the session tree",
		handler: async (args: string, ctx: any) => {
			const [targetId, mode] = args.trim().split(/\s+/);
			if (!targetId) return;
			await ctx.waitForIdle?.();
			await ctx.navigateTree(targetId, { summarize: mode === "summarize" });
		},
	});
}
