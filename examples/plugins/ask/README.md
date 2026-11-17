# Ask plugin

Lets the agent ask several questions at once and shows them as a form in the
chat. It ships a pi tool, `ask_user_question`; when pi calls it, the plugin
opens a `prompt.form` in that tool call:

- one question at a time, with a `2 / 4` counter and Back / Next / Submit;
- radio buttons for one choice, checkboxes for several;
- a text field for an answer of your own, alone or next to picked options;
- answers stay while you move between questions; Esc cancels and the agent
  still gets the answers given so far.

Keys: arrows move, Space or a digit picks, Enter goes on, Left and Right
change the question.

Install: copy this folder to `~/Library/Application Support/pi-gpui/plugins/ask`
(`~/.config/pi-gpui/plugins/ask` on Linux). Needs Node.js (found the same way
as pi, so nvm works).

If you already have an `ask_user_question` extension for the terminal, let it
step aside in the app (see the comment at the top of `pi-extension.ts`).
