/**
 * pi-gpui: `/gpui` moves the current terminal session into Pi GPUI, the
 * native desktop app for pi on macOS and Linux
 * (https://github.com/bytesbay/pi-gpui).
 *
 * pi does not guard a session file against two writers, so the session is
 * handed over only after this pi has exited: a detached shell waits for the
 * process to end, then opens `pi-gpui://open?workspace=...&session=...`
 * (with `open` on macOS; on Linux the link goes straight to the `pi-gpui`
 * binary, which hands it to the running app).
 */
import { execFileSync, spawn } from "node:child_process";
import { existsSync, readFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const INSTALL_HINT =
	process.platform === "darwin"
		? "Install it with `brew install --cask bytesbay/tap/pi-gpui` or from https://github.com/bytesbay/pi-gpui/releases"
		: "Install it with `curl -fsSL https://pi-gpui.dev/install.sh | sh`";

export default function (pi: any) {
	pi.registerCommand("gpui", {
		description: "Continue this session in Pi GPUI, the native desktop app",
		handler: async (_args: string, ctx: any) => {
			if (ctx.mode !== "tui") {
				ctx.ui.notify("/gpui works from pi in a terminal.", "warning");
				return;
			}
			// The command that opens a pi-gpui:// link: its last argument is the link.
			let opener: string[];
			if (process.platform === "darwin") {
				const apps = findApps();
				if (apps.length === 0) {
					ctx.ui.notify(`Pi GPUI is not installed. ${INSTALL_HINT}`, "warning");
					return;
				}
				if (!apps.some(handlesLinks)) {
					ctx.ui.notify("This Pi GPUI is too old for /gpui. Update it to 0.1.10 or newer.", "warning");
					return;
				}
				opener = ["open"];
			} else if (process.platform === "linux") {
				const binary = findLinuxBinary();
				if (!binary) {
					ctx.ui.notify(`Pi GPUI is not installed. ${INSTALL_HINT}`, "warning");
					return;
				}
				opener = [binary];
			} else {
				ctx.ui.notify("Pi GPUI runs on macOS and Linux.", "warning");
				return;
			}

			const params = new URLSearchParams({ workspace: ctx.cwd });
			const session: string | undefined = ctx.sessionManager.getSessionFile();
			if (session) params.set("session", session);
			const url = `pi-gpui://open?${params}`;

			if (!session) {
				// Nothing on disk to hand over: just open the project there.
				spawn(opener[0], [url], { detached: true, stdio: "ignore" }).unref();
				ctx.ui.notify("Opened this project in Pi GPUI.", "info");
				return;
			}
			spawn(
				"/bin/sh",
				[
					"-c",
					'while kill -0 "$1" 2>/dev/null; do sleep 0.1; done; exec "$2" "$3"',
					"sh",
					String(process.pid),
					opener[0],
					url,
				],
				{ detached: true, stdio: "ignore" },
			).unref();
			ctx.ui.notify("Continuing in Pi GPUI.", "info");
			ctx.shutdown();
		},
	});
}

/** `pi-gpui` on PATH, or where the Linux installer puts it. */
function findLinuxBinary(): string | undefined {
	const candidates = (process.env.PATH ?? "")
		.split(":")
		.filter(Boolean)
		.map((directory) => join(directory, "pi-gpui"));
	candidates.push(join(homedir(), ".local/bin/pi-gpui"), join(homedir(), ".local/pi-gpui.app/bin/pi-gpui"));
	return candidates.find((path) => existsSync(path));
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
