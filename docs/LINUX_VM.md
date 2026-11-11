# Linux test VM

A local VM for building and testing the Linux port with the full UI.

- Lima instance `pi-linux` (config at the end of this page), Apple Virtualization, aarch64, 6 CPU, 8 GiB RAM, 80 GiB disk.
- Ubuntu 24.04, GNOME on **Xorg** (Wayland disabled in `/etc/gdm3/custom.conf`), autologin, screen lock off, 1920x1200.
- The VM display opens as a window on the Mac.
- This repo is mounted read-write at `/home/pi-gpu-ui`. Edit on the Mac, build in the VM.
- Builds go to `~/target` in the VM (`CARGO_TARGET_DIR`), separate from the macOS `target/`.
- Installed: Rust 1.90.0, GPUI build deps, Mesa (Vulkan via llvmpipe, software), xdotool, wmctrl, ImageMagick, ffmpeg, Weston, Xvfb, Node 22 (nvm), pi 1.5.2.
- User password in the VM: `pi` (sudo is passwordless).

## Driving it

Everything goes through `script/linux-vm.sh` on the Mac:

```sh
script/linux-vm.sh start                # boot the VM (stop / status too)
script/linux-vm.sh build                # cargo build in the VM (~6 min cold, incremental after)
script/linux-vm.sh run                  # start the app on the VM desktop
script/linux-vm.sh log 50               # app stdout/stderr
script/linux-vm.sh shot /tmp/vm.png     # screenshot to the Mac, then read the PNG
script/linux-vm.sh click 960 600        # click (desktop coordinates, 1920x1200)
script/linux-vm.sh type 'hello'         # type into the focused window
script/linux-vm.sh key ctrl+k Return    # key combos
script/linux-vm.sh x 'wmctrl -l'        # any command with DISPLAY set
script/linux-vm.sh sh 'cargo test'      # any command in the repo dir
```

Screenshots are 1920x1200. If you downscale for viewing, scale coordinates back before clicking. Run the screenshot and the image read one after another, not in parallel, or you'll see the previous frame.

For Wayland checks, run a nested Weston inside the session (`weston --width 1600 --height 1000 &`, then start the app with `WAYLAND_DISPLAY=wayland-1`).

## What was checked in this VM

The port runs on X11 (GNOME) and on Wayland (nested Weston): the window frame, moving and resizing, the folder picker, sending messages to pi, the session tree, the Files panel, clipboard text and images, the Trash, "Show in File Manager", `pi-gpui://` links, the installer (`site/install.sh` with `PI_GPUI_ARCHIVE=<tarball>`) and a launch from the desktop entry with a bare environment.

Not checked here: real GPU drivers (the VM renders with llvmpipe, so speed says nothing), x86_64, KDE and tiling compositors, input methods, and the updater against a real Linux release.

Notes for driving the app:

- `xdotool key` and `xdotool type` work. To hold a modifier (the Ctrl+1…9 tab hints) use `xdotool keydown Control_L`.
- `script/linux-vm.sh run` starts the debug build unless a newer release build exists. Debug builds use their own single-instance socket, so they run next to an installed app.
- pi in the VM needs a model: log in with `pi` in the VM, or point `~/.pi/agent/models.json` at any OpenAI-compatible endpoint.

## Recreating the VM

It was set up by hand; this is what it took. Install Lima (`brew install lima`), then start an instance named `pi-linux` from this config (adjust the mount to where the repository is):

```yaml
minimumLimaVersion: 2.0.0
base:
- template:_images/ubuntu-24.04
vmType: vz
cpus: 6
memory: 8GiB
disk: 80GiB
mountType: virtiofs
mounts:
- location: "~/path/to/pi-gpui"
  mountPoint: "/home/pi-gpu-ui"
  writable: true
video:
  display: "vz"
```

In the VM:

```sh
sudo apt-get install -y ubuntu-desktop-minimal xserver-xorg gnome-session gdm3 \
    mesa-vulkan-drivers vulkan-tools xdotool imagemagick x11-utils wmctrl xclip weston
/home/pi-gpu-ui/script/linux-deps.sh
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
# Node 22 (nvm) and pi: https://pi.dev
```

Then, so the scripts can drive the desktop: give your user a password, set `WaylandEnable=false`, `AutomaticLoginEnable=true` and `AutomaticLogin=<user>` under `[daemon]` in `/etc/gdm3/custom.conf`, turn the screen lock and idle delay off (`gsettings set org.gnome.desktop.screensaver lock-enabled false`, `gsettings set org.gnome.desktop.session idle-delay 0`), and add an autostart entry that runs `xrandr --output Virtual-1 --mode 1920x1200`. Reboot the VM.
