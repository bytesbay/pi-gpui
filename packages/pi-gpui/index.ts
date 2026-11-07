/**
 * pi-gpui: `/gpui` moves the current terminal session into Pi GPUI, the
 * native Mac app for pi (https://github.com/bytesbay/pi-gpui).
 *
 * pi does not guard a session file against two writers, so the session is
 * handed over only after this pi has exited: a detached shell waits for the
 * process to end, then opens `pi-gpui://open?workspace=...&session=...`.
 */
import { execFileSync, spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const INSTALL_HINT =
	"Install it with `brew install --cask bytesbay/tap/pi-gpui` or from https://github.com/bytesbay/pi-gpui/releases";

export default function (pi: any) {
	pi.registerCommand("gpui", {
		description: "Continue this session in Pi GPUI, the native Mac app",
		handler: async (_args: string, ctx: any) => {
			if (ctx.mode !== "tui") {
				ctx.ui.notify("/gpui works from pi in a terminal.", "warning");
				return;
			}
			if (process.platform !== "darwin") {
				ctx.ui.notify("Pi GPUI runs on macOS only for now.", "warning");
				return;
			}
			const apps = findApps();
			if (apps.length === 0) {
				ctx.ui.notify(`Pi GPUI is not installed. ${INSTALL_HINT}`, "warning");
				return;
			}
			if (!apps.some(handlesLinks)) {
				ctx.ui.notify("This Pi GPUI is too old for /gpui. Update it to 0.1.10 or newer.", "warning");
				return;
			}

			const params = new URLSearchParams({ workspace: ctx.cwd });
			const session: string | undefined = ctx.sessionManager.getSessionFile();
			if (session) params.set("session", session);
			const url = `pi-gpui://open?${params}`;

			if (!session) {
				// Nothing on disk to hand over: just open the project there.
				spawn("open", [url], { detached: true, stdio: "ignore" }).unref();
				ctx.ui.notify("Opened this project in Pi GPUI.", "info");
				return;
			}
			spawn(
				"/bin/sh",
				["-c", 'while kill -0 "$1" 2>/dev/null; do sleep 0.1; done; exec open "$2"', "sh", String(process.pid), url],
				{ detached: true, stdio: "ignore" },
			).unref();
			ctx.ui.notify("Continuing in Pi GPUI.", "info");
			ctx.shutdown();
		},
	});
}

function findApps(): string[] {
	const apps = ["/Applications/Pi GPUI.app", join(homedir(), "Applications/Pi GPUI.app")].filter((path) =>
		existsSync(path),
	);
	// Anywhere else Spotlight knows about, e.g. still in Downloads.
	try {
		const found = execFileSync("mdfind", ["kMDItemCFBundleIdentifier == 'dev.pi-gpui.app'"], {
			encoding: "utf8",
			timeout: 3000,
		});
		apps.push(...found.split("\n").filter((path) => path.endsWith(".app")));
	} catch {}
	return apps;
}

/** Pi GPUI 0.1.10 and newer register the pi-gpui:// scheme. */
function handlesLinks(app: string): boolean {
	try {
		return readFileSync(join(app, "Contents/Info.plist"), "utf8").includes("CFBundleURLSchemes");
	} catch {
		return false;
	}
}
