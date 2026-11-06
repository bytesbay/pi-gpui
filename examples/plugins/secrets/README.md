# Secrets plugin

Lets the agent ask for an API key, token or password without it ever entering
the chat. It ships a pi tool, `request_secret`; when pi calls it, the plugin
shows a masked field in that tool call. Enter saves, Esc cancels, the eye
button shows what you typed.

The value is stored in `~/.pi/agent/secrets/<NAME>` (folder 0700, file 0600).
The agent only gets the file path and uses it as `$(cat <path>)` in commands.
Stored values are replaced with `***` in tool results and messages.

Install: copy this folder to `~/Library/Application Support/pi-gpui/plugins/secrets`.
Needs Node.js (found the same way as pi, so nvm works).
