// Pi GPUI plugin: the agent's questions as a form inside the chat.
//
// Protocol (docs/PLUGINS.md): the app sends `tool.start` when pi calls
// `ask_user_question`. We open a `prompt.form` in that tool call; the app
// draws the questions, keeps the answers while the user moves between them
// and sends all of them back as `prompt.result`. We leave them in
// <tmp>/pi-gpui-ask/<callId>.json, where the tool is waiting.
import { chmodSync, mkdirSync, renameSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import readline from "node:readline";

const ANSWERS_DIR = join(tmpdir(), "pi-gpui-ask");

/** Calls with an open form. */
const pending = new Set();

const send = (method, params = {}) => process.stdout.write(`${JSON.stringify({ method, params })}\n`);

const answerPath = (callId) => join(ANSWERS_DIR, `${callId.replace(/[^A-Za-z0-9_.-]/g, "_")}.json`);

/** Writes the file in one step, so the tool never reads half of it. */
function answer(callId, result) {
	mkdirSync(ANSWERS_DIR, { recursive: true, mode: 0o700 });
	chmodSync(ANSWERS_DIR, 0o700);
	const path = answerPath(callId);
	const temporary = `${path}.${process.pid}.tmp`;
	writeFileSync(temporary, JSON.stringify(result), { mode: 0o600 });
	renameSync(temporary, path);
}

function ask(callId, args) {
	const questions = (Array.isArray(args?.questions) ? args.questions : [])
		.map((question) => ({
			title: String(question?.question ?? "").trim(),
			description: String(question?.details ?? "").trim() || undefined,
			options: Array.isArray(question?.options) ? question.options : [],
			multiple: question?.multiSelect === true,
			// The user can always answer in their own words.
			allowText: true,
		}))
		.filter((question) => question.title);
	if (questions.length === 0) {
		return; // The tool rejects the call itself.
	}
	pending.add(callId);
	send("prompt.form", { id: callId, callId, questions, submitLabel: "Send answers" });
}

function finish(params) {
	if (!pending.delete(params.id) || params.closed) {
		return; // Not ours, or the tool call ended and nobody waits.
	}
	answer(params.id, {
		status: params.cancelled ? "cancelled" : "answered",
		answers: Array.isArray(params.answers) ? params.answers : [],
		error: params.error,
	});
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
		if (message.method === "tool.start" && params.tool === "ask_user_question") {
			ask(params.callId, params.args);
		} else if (message.method === "prompt.result") {
			finish(params);
		}
	} catch (error) {
		send("notify", { message: `Ask: ${error.message}`, tone: "error" });
	}
});
