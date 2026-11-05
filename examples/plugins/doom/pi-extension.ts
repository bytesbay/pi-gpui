/**
 * The agent's side of the DOOM plugin: a `doom` tool. The tool does not run
 * the game; Pi GPUI sees the call and the plugin draws a playable game in
 * that row of the chat.
 */
import { Type } from "@earendil-works/pi-ai";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

export default function (pi: ExtensionAPI) {
	pi.registerTool({
		name: "doom",
		label: "DOOM",
		description:
			"Opens a playable DOOM (Freedoom, E1M1) right in the chat for the user. Call it when the user wants to play DOOM or asks whether it can run DOOM. Calling it again moves the game to the new message.",
		parameters: Type.Object({}),
		async execute() {
			return {
				content: [
					{
						type: "text",
						text: "DOOM is open in the chat. Click it to play: arrows or WASD move, Ctrl or F fires, Space opens doors, 1 to 4 switch weapons, Esc gives the keyboard back.",
					},
				],
				details: undefined,
			};
		},
	});
}
