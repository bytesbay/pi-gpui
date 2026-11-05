"""Pi GPUI plugin that draws Freedoom (via ViZDoom) in the chat.

Protocol (docs/PLUGINS.md): the app sends `view.open` when pi calls the
`doom` tool, then `view.focus` and `view.key` while the user plays. We send
`view.frame` with JPEG frames for the one call we stream to; a newer call
takes the stream over. The game only advances while the view has the keyboard.
"""
import base64
import io
import json
import os
import sys
import threading
import time

import vizdoom as vzd
from PIL import Image

TICRATE = 35
B = vzd.Button
BUTTONS = [
    B.MOVE_FORWARD, B.MOVE_BACKWARD, B.TURN_LEFT, B.TURN_RIGHT, B.MOVE_LEFT, B.MOVE_RIGHT,
    B.ATTACK, B.USE, B.SPEED, B.SELECT_WEAPON1, B.SELECT_WEAPON2, B.SELECT_WEAPON3, B.SELECT_WEAPON4,
]
KEYS = {
    "up": B.MOVE_FORWARD, "w": B.MOVE_FORWARD,
    "down": B.MOVE_BACKWARD, "s": B.MOVE_BACKWARD,
    "left": B.TURN_LEFT, "right": B.TURN_RIGHT,
    "a": B.MOVE_LEFT, "d": B.MOVE_RIGHT,
    "control": B.ATTACK, "f": B.ATTACK, "j": B.ATTACK,
    "space": B.USE, "e": B.USE, "enter": B.USE,
    "shift": B.SPEED,
    "1": B.SELECT_WEAPON1, "2": B.SELECT_WEAPON2, "3": B.SELECT_WEAPON3, "4": B.SELECT_WEAPON4,
}
# The same physical keys on a Russian layout (GPUI reports the typed character).
for latin, cyrillic in zip("wasdfje", "цфываоу"):
    KEYS[cyrillic] = KEYS[latin]

out_lock = threading.Lock()
state_lock = threading.Lock()
live_call = None   # the tool call we stream to
focused = False
held = set()


def send(method, **params):
    with out_lock:
        sys.stdout.write(json.dumps({"method": method, "params": params}) + "\n")
        sys.stdout.flush()


def make_game():
    game = vzd.DoomGame()
    game.set_doom_game_path(os.path.join(os.path.dirname(vzd.__file__), "freedoom1.wad"))
    game.set_doom_map("E1M1")
    game.set_doom_skill(2)
    game.set_screen_resolution(vzd.ScreenResolution.RES_640X400)
    game.set_screen_format(vzd.ScreenFormat.RGB24)
    game.set_render_hud(True)
    game.set_render_weapon(True)
    game.set_render_crosshair(False)
    game.set_render_messages(True)
    game.set_window_visible(False)
    game.set_mode(vzd.Mode.PLAYER)
    game.set_available_buttons(BUTTONS)
    game.set_episode_timeout(0)
    game.init()
    game.new_episode()
    return game


def encode(game):
    state = game.get_state()
    if state is None:
        return None
    buffer = io.BytesIO()
    Image.fromarray(state.screen_buffer).save(buffer, "JPEG", quality=78)
    return base64.b64encode(buffer.getvalue()).decode()


game = None


def game_loop():
    global game
    sent_to = None
    while True:
        with state_lock:
            call, playing, keys = live_call, focused, set(held)
        if call is None:
            time.sleep(0.05)
            continue
        if game is None:
            game = make_game()
        if playing:
            start = time.time()
            if game.is_player_dead() or game.is_episode_finished():
                if B.USE in keys or B.ATTACK in keys:
                    game.new_episode()
            else:
                game.make_action([1 if button in keys else 0 for button in BUTTONS], 1)
            frame = encode(game)
            if frame:
                send("view.frame", callId=call, data=frame, mimeType="image/jpeg")
            sent_to = call
            time.sleep(max(0.0, 1 / TICRATE - (time.time() - start)))
        else:
            # Paused: one frame for a newly opened call, then wait.
            if sent_to != call:
                frame = encode(game)
                if frame:
                    send("view.frame", callId=call, data=frame, mimeType="image/jpeg")
                sent_to = call
            time.sleep(0.05)


def main():
    global live_call, focused
    threading.Thread(target=game_loop, daemon=True).start()
    for line in sys.stdin:
        try:
            message = json.loads(line)
        except ValueError:
            continue
        method, params = message.get("method"), message.get("params") or {}
        with state_lock:
            if method == "view.open":
                live_call = params.get("callId")
                focused = False
                held.clear()
            elif method == "view.focus" and params.get("callId") == live_call:
                focused = bool(params.get("focused"))
                held.clear()
            elif method == "view.key" and params.get("callId") == live_call:
                button = KEYS.get(str(params.get("key", "")).lower())
                if button is not None:
                    (held.add if params.get("down") else held.discard)(button)
    # stdin closed: the app quit. Stop the engine, it is a separate process.
    if game is not None:
        game.close()


if __name__ == "__main__":
    main()
