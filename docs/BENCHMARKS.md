# Benchmarks

Pi GPUI next to three Electron desktop clients for pi, measured on 2026-11-08.

| | Pi GPUI 0.1.11 | Pi Desktop 0.2.2-alpha | pi-gui 1.0.1 | PiDeck 0.11.3 |
|---|---|---|---|---|
| Download | 7.3 MB | 144 MB | 137 MB | 120 MB |
| App on disk | 16 MB | 401 MB | 433 MB | 356 MB |
| Time until the window is on screen | 0.40 s | 0.74 s | 1.07 s | 0.52 s |
| Memory at idle | 74 MB (1 process) | 193 MB (4) | 280 MB (4) | 429 MB (4) |

## How

- MacBook Pro, Apple M1 Pro, 16 GB, macOS 27.0.1, pi 1.5.0. Apple Silicon builds from each project's latest GitHub release.
- Download is the size of the release asset (zip or dmg). App on disk is `du -sh` of the `.app`.
- Time: each app is started 6 times from its executable with an empty `HOME`, so every app opens with no projects. The first start is a warm-up; the table shows the median of the other 5. The clock stops when a normal-size window of the app is on screen. An Electron app can show its window before its page has loaded, so this measure favors them.
- Memory: physical footprint (`footprint`, the number Activity Monitor shows as Memory) of all the app's processes, 10 s after the last start. pi and node processes an app starts are left out.
- With projects and running sessions every app uses more. Pi GPUI starts one pi process per open session, like the other clients.

To repeat it:

```sh
script/bench/bench.sh pi-gpui "/Applications/Pi GPUI.app/Contents/MacOS/pi-gpui"
script/bench/bench.sh pi-gui "/Applications/pi-gui.app/Contents/MacOS/pi-gui"
```
