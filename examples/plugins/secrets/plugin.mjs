// Pi GPUI plugin: asks for secrets in a masked field inside the chat.
//
// Protocol (docs/PLUGINS.md): the app sends `tool.start` when pi calls
// `request_secret`. We open a `prompt.input` field in that tool call; the
// answer comes back as `prompt.result`. We store the value in
// ~/.pi/agent/secrets/<NAME> and leave the outcome (never the value) in
// ~/.pi/agent/secrets/.answers/<callId>.json, where the tool is waiting for it.
import { chmodSync, existsSync, mkdirSync, renameSync, writeFileSync } from "node:fs";
import { homedir } from "node:os";
import { join } from "node:path";
import readline from "node:readline";

const SECRETS_DIR = join(homedir(), ".pi", "agent", "secrets");
const ANSWERS_DIR = join(SECRETS_DIR, ".answers");
const NAME_RE = /^[A-Za-z0-9_.-]{1,100}$/;

/** Open prompts: call id → secret name and whether it existed. */
const pending = new Map();

const send = (method, params = {}) => process.stdout.write(`${JSON.stringify({ method, params })}\n`);

function privateDir(path) {
	mkdirSync(path, { recursive: true, mode: 0o700 });
	chmodSync(path, 0o700);
}

/** Writes a 0600 file in one step, so the reader never sees half of it. */
function writePrivate(path, text) {
	const temporary = `${path}.${process.pid}.tmp`;
	writeFileSync(temporary, text, { mode: 0o600 });
	chmodSync(temporary, 0o600);
	renameSync(temporary, path);
}

const answerPath = (callId) => join(ANSWERS_DIR, `${callId.replace(/[^A-Za-z0-9_.-]/g, "_")}.json`);

function answer(callId, result) {
	privateDir(SECRETS_DIR);
	privateDir(ANSWERS_DIR);
	writePrivate(answerPath(callId), JSON.stringify(result));
}

function ask(callId, args) {
	const name = String(args?.name ?? "").trim();
	if (!NAME_RE.test(name)) {
		return; // The tool rejects the name itself.
	}
	const exists = existsSync(join(SECRETS_DIR, name));
	pending.set(callId, { name, exists });
	const notes = [String(args?.description ?? "").trim()];
	if (exists) {
		notes.push("Already saved: a new value replaces it, Cancel keeps the current one.");
	}
	send("prompt.input", {
		id: callId,
		callId,
		title: `Secret ${name}`,
		description: notes.filter(Boolean).join(" ") || undefined,
		placeholder: "Paste or type the value",
		submitLabel: "Save",
		secret: true,
	});
}

function finish(params) {
	const request = pending.get(params.id);
	if (!request) {
		return;
	}
	pending.delete(params.id);
	if (params.closed) {
		return; // The tool call ended; nobody waits for the answer.
	}
	const value = typeof params.value === "string" ? params.value.trim() : "";
	if (params.cancelled || !value) {
		answer(params.id, { status: request.exists ? "kept" : "cancelled" });
		return;
	}
	privateDir(SECRETS_DIR);
	writePrivate(join(SECRETS_DIR, request.name), value);
	answer(params.id, { status: "saved", length: value.length });
}

readline.createInterface({ input: process.stdin }).on("line", (line) => {
	let message;
	try {
		message = JSON.parse(line);
	} catch {
		return;
	}
	const params = message.params ?? {};
	try {
		if (message.method === "tool.start" && params.tool === "request_secret") {
			ask(params.callId, params.args);
		} else if (message.method === "prompt.result") {
			finish(params);
		}
	} catch (error) {
		send("notify", { message: `Secrets: ${error.message}`, tone: "error" });
	}
});
