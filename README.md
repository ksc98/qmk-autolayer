# qmk-autolayer

Switches QMK keyboard layers from the focused window on Linux. Hyprland
reports focus changes; the daemon matches the window against rules and
writes a raw-HID report to the keyboard, which calls `layer_on` or
`layer_off`. Layer logic, indicators, and per-layer lighting stay in
firmware.

Rust, two dependencies (`serde`, `toml`), no polling. Typically 3 MB resident
and idle at 0% CPU; each window change costs under 0.1 ms.

## Requirements

- Hyprland (other compositors: see below).
- A QMK board with `RAW_ENABLE = yes` or `VIA_ENABLE = yes`.
- A small firmware addition (below).
- Read/write access to the board's hidraw node.

## Install

```sh
git clone https://github.com/ksc98/qmk-autolayer
cd qmk-autolayer
just install
```

`just install` runs `cargo install --path . --locked`, installs
`qmk-autolayer.service` as a systemd user unit, and enables it. The unit is
bound to `graphical-session.target`.

### hidraw access

hidraw nodes are root-only by default. Add a udev rule with your board's
USB ids (`lsusb`):

```
# /etc/udev/rules.d/70-qmk-raw-hid.rules
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="7179", ATTRS{idProduct}=="8475", TAG+="uaccess"
```

`sudo udevadm control --reload`, replug, then confirm:

```
$ qmk-autolayer list
/dev/hidraw1 7179:8475 gok TypeK-S Rev. Zeta-RC2
```

The rule file must sort before `73-seat-late.rules`. Current
[qmk_udev](https://github.com/qmk/qmk_udev) rules cover raw HID; older
distro-packaged versions cover only the console interface.

## Firmware

The daemon sends a 32-byte report `[0x42, layer, state]` with `state` 1 for
on, 0 for off. QMK has no built-in handler for this, so add one.

VIA boards (`VIA_ENABLE = yes`). VIA owns `raw_hid_receive` and forwards
command ids it does not recognise to `via_command_kb`:

```c
#include "via.h"

bool via_command_kb(uint8_t *data, uint8_t length) {
    if (length < 3 || data[0] != 0x42) {
        return false;
    }
    if (data[1] < DYNAMIC_KEYMAP_LAYER_COUNT) {
        data[2] ? layer_on(data[1]) : layer_off(data[1]);
    }
    return true;
}
```

Non-VIA boards (`RAW_ENABLE = yes`):

```c
#include "raw_hid.h"

void raw_hid_receive(uint8_t *data, uint8_t length) {
    if (length < 3 || data[0] != 0x42) {
        return;
    }
    if (data[1] < 32) {
        data[2] ? layer_on(data[1]) : layer_off(data[1]);
    }
}
```

Command id `0x42` is outside the range VIA uses (`0x01`–`0x0F`), so the VIA
app and this daemon share the interface without conflict. The id can only
collide with code you added yourself: if your keyboard or keymap already
defines `raw_hid_receive` or `via_command_kb` and handles `0x42` there,
choose another id in both the firmware and the config (`command` under
`[[keyboard]]`).

## Configuration

`$XDG_CONFIG_HOME/qmk-autolayer/config.toml`, defaulting to
`~/.config/qmk-autolayer/config.toml`. `--config PATH` overrides. The file is
re-read when its mtime changes; a file that fails to parse is reported once
and the previous configuration stays in effect. Changes to `[[keyboard]]`
require a restart.

```toml
# Layer for rules that do not set one.
default_layer = 1

# Omit to use the first QMK raw-HID device found.
[[keyboard]]
name = "typek"
vid = 0x7179
pid = 0x8475
# command = 0x42   # only if changed in firmware

# First matching rule wins. Patterns are compared against the window's
# class, initial class, and executable name. `*` matches any characters.
# A rule without `layer` uses `default_layer`.
[[rule]]
match = ["steam_app_*"]                      # Proton games, layer 1

[[rule]]
match = ["valheim.x86_64", "Terraria"]       # native games, layer 1

[[rule]]
match = ["firefox"]
layer = 2
# keyboard = "typek"                         # or ["typek", "corne"]; default: all
```

To find a window's class or executable name, run `qmk-autolayer -v` and
focus it, or use `hyprctl activewindow`.

## Usage

```
qmk-autolayer [-v] [--config PATH]                   run the daemon
qmk-autolayer list                                   list QMK raw-HID devices
qmk-autolayer set <layer> on|off [--keyboard NAME]   send one report
```

Logs: `journalctl --user -u qmk-autolayer -f`.

## Behaviour

- Reports are sent only when the wanted layer changes. A layer toggled by
  hand while no rule matches is left alone.
- On startup and whenever a keyboard is (re)attached, every layer the rules
  reference is turned off, then the current focus is applied.
- Keyboard unplug is detected immediately; the node is re-found when the
  device returns.
- A Hyprland restart is handled by reconnecting to the new instance socket.
- Stopping the daemon does not turn layers off.

## Other compositors

A backend is one module providing two functions: emit an event on focus
change, and return the focused window's class, initial class, and pid. See
`src/hypr.rs`. niri, Sway/i3, and X11 are all feasible.

## Related projects

[active-app-qmk-layer-updater](https://github.com/zigotica/active-app-qmk-layer-updater)
(Node.js, macOS), [auto_layers](https://github.com/itsvar8/auto_layers)
(Python, Windows), [qmk-hid-host](https://github.com/zzeneg/qmk-hid-host)
(Rust; sends host data to the keyboard, no layer switching).

## License

MIT
