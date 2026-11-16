# Bến

A liquid-glass shell for Windows 11. It replaces the taskbar with a glass dock along the bottom edge and a dynamic island along the top edge of every monitor. Written in Rust, drawn with Windows Composition.

## Download
- https://github.com/GnimDev/ben-dock/releases/tag/test
## Features

- **Dock:** pinned and running apps. Icons magnify under the cursor, show live window previews on hover, and can be dragged to reorder. Drag an app, shortcut or file onto the dock to pin it: the glass strains like the skin of a water drop, then lets the icon slip in. Dividers group the pinned apps. The first icon opens an app launcher with search, made of the same glass as the dock; tapping the Windows key opens it too, in place of the Start menu.
- **Dynamic island:** clock, date, network and battery. It expands for volume, charging, connectivity and now-playing changes. Hover it to open a control panel with media controls, volume, Wi-Fi, Bluetooth, audio output, battery, a calendar and power buttons.
- **Floating windows:** new windows open centered at up to 70% of the screen instead of maximized.
- **Minimize effects:** windows minimize into their own dock icon. A genie-style effect plays when you click the icon.
- **Themes:** four glass materials, light/dark mode, accent tint.
- **Multiple monitors:** each monitor gets its own dock and island.

## Build and run

1. Run `setup.bat` once. It checks for Rust and a linker, and asks before installing anything that's missing.
2. Run `build.bat`. The output is `dist\ben.exe`, and it starts automatically after the build. Use `build.bat norun` to skip starting it.
3. Right-click the dock and choose **Start with Windows** to run Bến at login.

Both Rust toolchains work: MSVC (needs the C++ Build Tools) or GNU (needs MinGW's `dlltool` and `as` on PATH, e.g. `C:\msys64\ucrt64\bin`).

## Usage

- **Windows key:** a tap of the Windows key on its own opens the app tray; start typing to search. Win+D, Win+E and the like work as before, and Ctrl+Esc still opens the Start menu. Turn it off with **Phím Windows mở khay ứng dụng** in the settings, or `win_key = false` in `ben.ini`.
- **Pinning by drag and drop:** drag an app, shortcut or file from Explorer, the desktop or the app launcher onto the dock. Its icon squeezes in and follows the cursor; drop it where it should stay.
- **Dividers:** right-click a pinned icon and choose **Thêm vách ngăn phía sau** to put a divider after it. Drag a divider to move it; right-click it to remove it. Dividers are saved in `pins.txt` as `---` lines.
- **Dock:** right-click an icon for app actions. Right-click empty space on the dock for settings, showing the Windows taskbar, autostart and quit.
- **Settings:** open them from the dock menu, or edit `ben.ini` next to `ben.exe`. Changes apply immediately.
- **Taskbar:** the Windows taskbar is hidden while Bến runs and comes back when it quits. If Bến crashes and the taskbar stays hidden, run `restore-taskbar.bat`.

## Command line

```
ben.exe                     start Bến
ben.exe --quit              stop the running instance
ben.exe --restore-taskbar   bring the Windows taskbar back
ben.exe --autostart on|off  toggle start with Windows
```
