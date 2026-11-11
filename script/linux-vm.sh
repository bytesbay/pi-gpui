#!/usr/bin/env bash
# Drive the local Linux test VM (Lima "pi-linux", Ubuntu 24.04, GNOME on Xorg).
# The repo is mounted read-write in the VM at /home/pi-gpu-ui; builds go to ~/target in the VM.
#
#   script/linux-vm.sh start | stop | status
#   script/linux-vm.sh sh '<cmd>'          run a shell command in the VM (cargo, pi and node on PATH)
#   script/linux-vm.sh build [--release]   cargo build in the VM
#   script/linux-vm.sh run [args]          (re)start the app on the VM desktop, log in /tmp/pi-gpui.log
#   script/linux-vm.sh kill                stop the app
#   script/linux-vm.sh log [n]             tail the app log
#   script/linux-vm.sh shot [out.png]      screenshot of the VM desktop to the Mac (default /tmp/vm.png)
#   script/linux-vm.sh click X Y [button]  click at desktop coordinates (1920x1200)
#   script/linux-vm.sh type 'text'         type text into the focused window
#   script/linux-vm.sh key ctrl+k ...      send key combos (xdotool names)
#   script/linux-vm.sh x '<cmd>'           run any X11 command (xdotool, wmctrl, xprop...)
set -euo pipefail

VM=pi-linux
export PATH="/opt/homebrew/bin:$PATH"

in_vm() {
  limactl shell --workdir /home/pi-gpu-ui "$VM" -- bash -lc "
    . ~/.cargo/env 2>/dev/null; . ~/.nvm/nvm.sh 2>/dev/null
    export CARGO_TARGET_DIR=\$HOME/target
    export DISPLAY=:0 XAUTHORITY=/run/user/\$(id -u)/gdm/Xauthority
    export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/\$(id -u)/bus XDG_RUNTIME_DIR=/run/user/\$(id -u)
    $1"
}

q() { [ $# -eq 0 ] || printf '%q ' "$@"; }

cmd=${1:-status}; shift || true
case "$cmd" in
  start)  limactl start "$VM" --tty=false ;;
  stop)   limactl stop "$VM" ;;
  status) limactl list "$VM" ;;
  sh|x)   in_vm "$*" ;;
  build)  in_vm "cargo build $(q "$@")" ;;
  run)
    in_vm "pkill -x pi-gpui || true; while pgrep -x pi-gpui >/dev/null; do sleep 0.1; done; profile=debug; [ -x \$CARGO_TARGET_DIR/release/pi-gpui ] && [ \$CARGO_TARGET_DIR/release/pi-gpui -nt \$CARGO_TARGET_DIR/debug/pi-gpui ] && profile=release
      setsid nohup \$CARGO_TARGET_DIR/\$profile/pi-gpui $(q "$@") > /tmp/pi-gpui.log 2>&1 < /dev/null & disown; sleep 2; pgrep -a pi-gpui || tail -20 /tmp/pi-gpui.log" ;;
  kill)   in_vm "pkill -x pi-gpui || true" ;;
  log)    in_vm "tail -n ${1:-50} /tmp/pi-gpui.log" ;;
  shot)
    out=${1:-/tmp/vm.png}
    # Nudge the pointer so mutter repaints before the capture (avoids stale frames).
    in_vm "xdotool mousemove_relative 1 0 sleep 0.1 mousemove_relative -- -1 0; sleep 0.3; import -window root /tmp/vm-shot.png"
    limactl copy "$VM:/tmp/vm-shot.png" "$out" >/dev/null
    echo "$out" ;;
  click)  in_vm "xdotool mousemove $1 $2 click ${3:-1}" ;;
  type)   in_vm "xdotool type --delay 20 -- $(q "$*")" ;;
  key)    in_vm "xdotool key $(q "$@")" ;;
  *) sed -n '2,17p' "$0"; exit 1 ;;
esac
