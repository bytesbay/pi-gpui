/**
 * The agent's side of the Secrets plugin: a `request_secret` tool. The tool
 * does not ask anything itself; Pi GPUI sees the call and the plugin shows a
 * masked field in that row of the chat. The plugin stores the value and
 * leaves only the outcome for this tool, so the value never reaches pi.
 *
 * As a safety net, stored values are replaced with *** in tool results and
 * messages, so an accidental `cat` does not leak them.
 *
 * pi refuses two extensions that register the same tool. A terminal secrets
 * extension can step aside in the app by checking the marker set below:
 *   if ((globalThis as Record<symbol, unknown>)[Symbol.for("pi-gpui.secrets")]) return;
 * The app passes this file with --extension, so it loads before the user's
 * own extensions.
 */
import { Type } from "@earendil-works/pi-ai";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { type FSWatcher, existsSync, mkdirSync, readdirSync, readFileSync, statSync, unlinkSync, watch } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";

const SECRETS_DIR = join(homedir(), ".pi", "agent", "secrets");
const ANSWERS_DIR = join(SECRETS_DIR, ".answers");
const NAME_RE = /^[A-Za-z0-9_.-]{1,100}$/;
/** Safety net in case a file event is missed; events do the real work. */
const FALLBACK_POLL_MS = 2000;
const WAIT_LIMIT_MS = 30 * 60 * 1000;
const MASK = "***";
const MIN_REDACT_LENGTH = 6;

const answerPath = (callId: string) => join(ANSWERS_DIR, `${callId.replace(/[^A-Za-z0-9_.-]/g, "_")}.json`);

type Answer = { status: "saved" | "kept" | "cancelled"; length?: number };

function readAnswer(path: string): Answer | null {
	try {
		const answer = JSON.parse(readFileSync(path, "utf8")) as Answer;
		unlinkSync(path);
		return answer;
	} catch {
		return null; // Not there yet.
	}
}

/**
 * Waits for the plugin to leave the outcome of this call. Sleeps on a file
 * watch instead of polling, so a waiting tool costs nothing.
 */
function waitForAnswer(callId: string, signal?: AbortSignal): Promise<Answer | null> {
	mkdirSync(ANSWERS_DIR, { recursive: true, mode: 0o700 });
	const path = answerPath(callId);
	const file = path.slice(ANSWERS_DIR.length + 1);
	return new Promise((resolve) => {
		let watcher: FSWatcher | undefined;
		const finish = (answer: Answer | null) => {
			watcher?.close();
			clearInterval(poll);
			clearTimeout(limit);
			signal?.removeEventListener("abort", abort);
			resolve(answer);
		};
		const check = () => {
			const answer = readAnswer(path);
			if (answer) {
				finish(answer);
			}
		};
		const abort = () => finish(null);
		const poll = setInterval(check, FALLBACK_POLL_MS);
		const limit = setTimeout(abort, WAIT_LIMIT_MS);
		if (signal?.aborted) {
			return abort();
		}
		signal?.addEventListener("abort", abort, { once: true });
		try {
			watcher = watch(ANSWERS_DIR, (_event, name) => {
				if (!name || name === file) {
					check();
				}
			});
		} catch {
			// No file events here; the poll still answers.
		}
		check();
	});
}

function usageHint(name: string, path: string): string {
	return [
		`Secret ${name} is stored in file: ${path}`,
		"The value is intentionally hidden. Never print, cat, echo or otherwise output it, and never write it into project files or commit it.",
		`Use it only inside commands, e.g. export ${name.replace(/[^A-Za-z0-9_]/g, "_")}="$(cat ${path})" or curl -H "Authorization: Bearer $(cat ${path})" ...`,
		"Any tool output containing the value is automatically redacted to ***.",
	].join("\n");
}

let cache: { key: string; values: string[] } | null = null;
/** While the folder is watched, a cached list stays valid until a file event. */
let cacheWatcher: FSWatcher | undefined;

function watchSecrets(): void {
	if (cacheWatcher) {
		return;
	}
	try {
		cacheWatcher = watch(SECRETS_DIR, () => {
			cache = null;
		});
		cacheWatcher.on("error", () => {
			cacheWatcher?.close();
			cacheWatcher = undefined;
			cache = null;
		});
		cacheWatcher.unref();
	} catch {
		cacheWatcher = undefined;
	}
}

function secretValues(): string[] {
	if (cache && cacheWatcher) {
		return cache.values;
	}
	try {
		if (!existsSync(SECRETS_DIR)) {
			return [];
		}
		watchSecrets();
		const files = readdirSync(SECRETS_DIR)
			.filter((file) => !file.startsWith("."))
			.map((file) => ({ file, stat: statSync(join(SECRETS_DIR, file)) }))
			.filter(({ stat }) => stat.isFile());
		const key = files.map(({ file, stat }) => `${file}:${stat.mtimeMs}:${stat.size}`).join("|");
		if (cache?.key !== key) {
			const values = files
				.map(({ file }) => readFileSync(join(SECRETS_DIR, file), "utf8").trim())
				.filter((value) => value.length >= MIN_REDACT_LENGTH)
				.sort((a, b) => b.length - a.length);
			cache = { key, values };
		}
		return cache.values;
	} catch {
		return [];
	}
}

function redact(value: unknown, secrets: string[]): unknown {
	if (typeof value === "string") {
		return secrets.reduce((text, secret) => (text.includes(secret) ? text.split(secret).join(MASK) : text), value);
	}
	if (Array.isArray(value)) {
		const out = value.map((item) => redact(item, secrets));
		return out.some((item, index) => item !== value[index]) ? out : value;
	}
	if (value && typeof value === "object") {
		const object = value as Record<string, unknown>;
		if (object.type === "image" || object.type === "image_url") {
			return value;
		}
		let changed = false;
		const out: Record<string, unknown> = {};
		for (const [key, item] of Object.entries(object)) {
			out[key] = redact(item, secrets);
			changed ||= out[key] !== item;
		}
		return changed ? out : value;
	}
	return value;
}

export default function (pi: ExtensionAPI) {
	(globalThis as Record<symbol, unknown>)[Symbol.for("pi-gpui.secrets")] = true;
	pi.registerTool({
		name: "request_secret",
		label: "Request Secret",
		description: `Ask the user to enter a secret (API key, token, password) in a masked field in the chat. The value is saved to ${SECRETS_DIR}/<name> and is NOT returned; you only get the file path. Use this instead of asking the user to paste secrets into chat.`,
		promptSnippet: "Ask the user for a secret via a masked field; returns only the file path",
		promptGuidelines: [
			"Use request_secret whenever you need an API key, token or password from the user. Never ask them to paste secrets into chat.",
			`Existing secrets live in ${SECRETS_DIR} (one file per secret). Check with ls before requesting a new one. Never print their contents; reference them as $(cat <path>) inside commands.`,
		],
		parameters: Type.Object({
			name: Type.String({
				description: "Secret identifier used as the file name, e.g. OPENAI_API_KEY. Allowed: letters, digits, _ . -",
			}),
			description: Type.Optional(
				Type.String({ description: "Short explanation shown to the user: what the secret is and where to get it." }),
			),
		}),
		executionMode: "sequential",

		async execute(toolCallId, params, signal) {
			const name = params.name.trim();
			if (!NAME_RE.test(name)) {
				throw new Error(`Invalid secret name "${name}". Use only letters, digits, _ . - (max 100 chars).`);
			}
			const path = join(SECRETS_DIR, name);
			const answer = await waitForAnswer(toolCallId, signal);
			if (!answer || answer.status === "cancelled") {
				return {
					content: [
						{
							type: "text",
							text: answer
								? `User cancelled entering secret ${name}. Nothing was saved.`
								: `No answer for secret ${name}. Nothing was saved.`,
						},
					],
					details: { name, status: "cancelled", path },
				};
			}
			const prefix =
				answer.status === "kept" ? "User kept the existing secret." : `User saved the secret (${answer.length} chars).`;
			return {
				content: [{ type: "text", text: `${prefix}\n${usageHint(name, path)}` }],
				details: { name, status: answer.status, path },
			};
		},
	});

	pi.on("tool_result", async (event) => {
		const secrets = secretValues();
		if (secrets.length === 0) {
			return undefined;
		}
		const content = redact(event.content, secrets) as typeof event.content;
		const details = redact(event.details, secrets);
		return content === event.content && details === event.details ? undefined : { content, details };
	});

	pi.on("message_end", async (event) => {
		const secrets = secretValues();
		if (secrets.length === 0) {
			return undefined;
		}
		const message = redact(event.message, secrets) as typeof event.message;
		return message === event.message ? undefined : { message };
	});
}
