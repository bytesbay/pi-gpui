/**
 * The agent's side of the Ask plugin: an `ask_user_question` tool. The tool
 * does not draw anything itself; Pi GPUI sees the call and the plugin shows
 * the questions as a form in that row of the chat (radio buttons, checkboxes,
 * a field for the user's own answer, Back / Next / Submit). The plugin leaves
 * the answers in a file this tool waits for.
 *
 * pi refuses two extensions that register the same tool. A terminal
 * ask_user_question extension can step aside in the app by checking the
 * marker set below:
 *   if ((globalThis as Record<symbol, unknown>)[Symbol.for("pi-gpui.ask")]) return;
 * The app passes this file with --extension, so it loads before the user's
 * own extensions.
 */
import { Type } from "@earendil-works/pi-ai";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { type FSWatcher, mkdirSync, readFileSync, unlinkSync, watch } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";

const ANSWERS_DIR = join(tmpdir(), "pi-gpui-ask");
/** Safety net in case a file event is missed; events do the real work. */
const FALLBACK_POLL_MS = 2000;
const MAX_QUESTIONS = 8;

const answerPath = (callId: string) => join(ANSWERS_DIR, `${callId.replace(/[^A-Za-z0-9_.-]/g, "_")}.json`);

/** One entry of `prompt.result`'s `answers` (docs/PLUGINS.md). */
type FormAnswer = { id: string; selected: number[]; labels: string[]; text?: string };
type Outcome = { status: "answered" | "cancelled"; answers: FormAnswer[]; error?: string };

function readOutcome(path: string): Outcome | null {
	try {
		const outcome = JSON.parse(readFileSync(path, "utf8")) as Outcome;
		unlinkSync(path);
		return outcome;
	} catch {
		return null; // Not there yet.
	}
}

/**
 * Waits for the plugin to leave the answers of this call. Sleeps on a file
 * watch instead of polling, so a waiting tool costs nothing.
 */
function waitForOutcome(callId: string, signal?: AbortSignal): Promise<Outcome | null> {
	mkdirSync(ANSWERS_DIR, { recursive: true, mode: 0o700 });
	const path = answerPath(callId);
	const file = path.slice(ANSWERS_DIR.length + 1);
	return new Promise((resolve) => {
		let watcher: FSWatcher | undefined;
		const finish = (outcome: Outcome | null) => {
			watcher?.close();
			clearInterval(poll);
			signal?.removeEventListener("abort", abort);
			resolve(outcome);
		};
		const check = () => {
			const outcome = readOutcome(path);
			if (outcome) {
				finish(outcome);
			}
		};
		const abort = () => finish(null);
		const poll = setInterval(check, FALLBACK_POLL_MS);
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

function describe(answer: FormAnswer | undefined): string {
	const parts = [...(answer?.labels ?? [])];
	if (answer?.text) {
		parts.push(parts.length > 0 ? `Other: ${answer.text}` : answer.text);
	}
	// Typed answers can have several lines; keep them under their question.
	return parts.length > 0 ? parts.join("; ").replace(/\n/g, "\n   ") : "(no answer)";
}

export default function (pi: ExtensionAPI) {
	(globalThis as Record<symbol, unknown>)[Symbol.for("pi-gpui.ask")] = true;
	pi.registerTool({
		name: "ask_user_question",
		label: "Ask User",
		description: `Ask the user up to ${MAX_QUESTIONS} questions in one form and wait for the answers. Use it when requirements are unclear, a preference is needed, or a decision would change the implementation. Each question can offer options (one or several can be picked); the user can always type an answer of their own.`,
		promptSnippet: "Ask the user one or several questions in a form and wait for the answers",
		promptGuidelines: [
			"Prefer ask_user_question over guessing when requirements, preferences or implementation choices are unclear.",
			"Put related questions into one ask_user_question call; the user moves between them and submits once.",
			'When recommending an ask_user_question option, put it first and append "(Recommended)" to its label.',
		],
		parameters: Type.Object({
			questions: Type.Array(
				Type.Object({
					question: Type.String({ description: "The question, one sentence." }),
					details: Type.Optional(Type.String({ description: "Extra context shown under the question." })),
					options: Type.Optional(
						Type.Array(
							Type.Object({
								label: Type.String({ description: "Short name of the option." }),
								description: Type.Optional(Type.String({ description: "What picking it means." })),
							}),
							{ description: "Choices to pick from. Omit for a free-text answer." },
						),
					),
					multiSelect: Type.Optional(Type.Boolean({ description: "Several options can be picked." })),
				}),
				{ minItems: 1, maxItems: MAX_QUESTIONS },
			),
		}),
		executionMode: "sequential",

		async execute(toolCallId, params, signal) {
			const questions = params.questions.filter((question) => question.question.trim());
			if (questions.length === 0) {
				throw new Error("ask_user_question needs at least one question.");
			}
			const outcome = await waitForOutcome(toolCallId, signal);
			if (!outcome) {
				return {
					content: [{ type: "text", text: "The question was not answered: the run was stopped." }],
					details: { status: "cancelled", answers: [] },
				};
			}
			if (outcome.error) {
				throw new Error(`The form could not be shown: ${outcome.error}`);
			}
			const lines = questions.map(
				(question, index) => `${index + 1}. ${question.question.trim()}\n   ${describe(outcome.answers[index])}`,
			);
			const header =
				outcome.status === "answered"
					? "The user answered:"
					: "The user cancelled the questions. Answers given before that:";
			return {
				content: [{ type: "text", text: `${header}\n${lines.join("\n")}` }],
				details: { status: outcome.status, answers: outcome.answers },
			};
		},
	});
}
