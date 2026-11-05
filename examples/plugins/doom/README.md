# DOOM plugin

An example of a Pi GPUI plugin that draws a live view in the chat. It ships a
pi tool, `doom`; when pi calls it, the app shows a playable Freedoom E1M1 in
that tool call. Click the game to play; Esc gives the keyboard back. A newer
`doom` call takes the game over, older ones keep their last frame.

Install: copy this folder to `~/Library/Application Support/pi-gpui/plugins/doom`.
The first start creates a Python environment with ViZDoom (it bundles the
Freedoom WADs), which takes a minute.

Keys: arrows or WASD move, Ctrl or F fires, Space or E opens doors, Shift runs,
1 to 4 switch weapons.
