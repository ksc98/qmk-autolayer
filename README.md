# qmk-autolayer

Switch QMK keyboard layers automatically from the focused window on Linux.
Focus a game, the gaming layer turns on; tab out, it turns off. The layer
logic stays in the firmware, so your layer indicators, per-layer RGB, and
everything else the keyboard does with layers keep working.

Small Rust daemon, no runtime dependencies, about 1 MB resident, idle at
0% CPU. Talks to Hyprland over its IPC socket and to the keyboard over the
raw-HID interface QMK already exposes for VIA.

```
[focus change] -> Hyprland IPC -> match class / exe against rules -> [0x42, layer, on] -> hidraw -> layer_on()
```

## How it works

1. Hyprland emits an `activewindow` event on every focus change.
2. The daemon asks Hyprland for the focused window's class, initial class,
   and pid, and reads the executable name from `/proc/<pid>/exe`.
3. Those three names are matched against the rules in your config. First
   match wins and names a layer.
4. When the wanted layer changes, one 32-byte raw-HID report is written to
   the keyboard: `[command, layer, 1]` to turn it on, `[command, layer, 0]`
   to turn it off. Nothing is sent while the answer stays the same, so a
   layer you toggle by hand outside any rule is left alone.

At startup, and whenever a keyboard is plugged in, every layer the rules can
set is turned off first so no state sticks from a previous run.

## Install

Requires a Rust toolchain and Hyprland.

```sh
git clone https://github.com/ksc98/qmk-autolayer
cd qmk-autolayer
just install          # cargo install + systemd user unit, enabled and started
```

Without `just`:

```sh
cargo install --path . --locked
install -Dm644 qmk-autolayer.service ~/.config/systemd/user/qmk-autolayer.service
systemctl --user daemon-reload
systemctl --user enable --now qmk-autolayer.service
```

The unit is bound to `graphical-session.target`, so it starts with your
Wayland session and stops with it.

### hidraw permissions

The raw-HID node is root-only by default. Grant your user access with a udev
rule (substitute your board's ids; `lsusb` shows them):

```
# /etc/udev/rules.d/70-qmk-raw-hid.rules
SUBSYSTEM=="hidraw", ATTRS{idVendor}=="7179", ATTRS{idProduct}=="8475", TAG+="uaccess"
```

Then `sudo udevadm control --reload` and replug. The rule must sort before
`73-seat-late.rules`, which is what applies `uaccess`. Recent versions of
[qmk_udev](https://github.com/qmk/qmk_udev) tag raw HID for all QMK boards,
but some distro packages ship an older helper that only tags the console
interface, so check with `qmk-autolayer list`:

```
$ qmk-autolayer list
/dev/hidraw1 7179:8475 gok TypeK-S Rev. Zeta-RC2
```

## Firmware

QMK has no built-in "set layer from host" command, so the keyboard needs a
few lines. Pick the one that matches your board.

**VIA boards** (`VIA_ENABLE = yes`): VIA owns `raw_hid_receive`, but hands
unknown command ids to `via_command_kb`. The command id `0x42` is outside
VIA's range, and the VIA app never sends it, so both coexist.

```c
// keyboard.c or keymap.c
#include "via.h"

bool via_command_kb(uint8_t *data, uint8_t length) {
    if (length < 3 || data[0] != 0x42) {
        return false;                       // not ours, let VIA handle it
    }
    uint8_t layer = data[1];
    if (layer < DYNAMIC_KEYMAP_LAYER_COUNT) {
        data[2] ? layer_on(layer) : layer_off(layer);
    }
    return true;                            // handled, no reply
}
```

**Plain raw HID boards** (`RAW_ENABLE = yes` in `rules.mk`, no VIA):

```c
// keymap.c
#include "raw_hid.h"

void raw_hid_receive(uint8_t *data, uint8_t length) {
    if (length < 3 || data[0] != 0x42) {
        return;
    }
    uint8_t layer = data[1];
    if (layer < 32) {
        data[2] ? layer_on(layer) : layer_off(layer);
    }
}
```

If `0x42` collides with something your firmware already uses, pick another
id and set `command = 0x..` on the keyboard in the config.

## Config

`$XDG_CONFIG_HOME/qmk-autolayer/config.toml`, which is
`~/.config/qmk-autolayer/config.toml` unless you've set `XDG_CONFIG_HOME`.
Override with `--config PATH`. Edits are picked up on the next focus change;
no restart needed (except for changes to the keyboard list). A broken edit is
reported once and the last good config stays in effect.

```toml
# Layer used by rules that don't name one.
default_layer = 1

# Keyboards to drive. Omit this section entirely to use the first QMK
# raw-HID device found. Ids as hex integers or strings; `lsusb` shows them.
[[keyboard]]
name = "typek"
vid = 0x7179
pid = 0x8475
# command = 0x42            # raw-HID command id, if you changed it in firmware

# Rules, first match wins. `match` patterns are compared against the focused
# window's class, initial class, and executable name; `*` is a wildcard.
[[rule]]
match = ["steam_app_*"]     # every Proton game
[[rule]]
match = ["valheim.x86_64", "Terraria", "tModLoader"]   # native games, by exe / class
[[rule]]
match = ["firefox"]
layer = 2
# keyboard = "typek"        # restrict a rule to one keyboard
```

Finding the right name for a window: run the daemon in the foreground with
`-v` and focus the app, or `hyprctl activewindow`.

```
$ qmk-autolayer -v
qmk-autolayer: 3 rule(s), 1 keyboard(s), config /home/you/.config/qmk-autolayer/config.toml
typek: /dev/hidraw1 7179:8475 gok TypeK-S Rev. Zeta-RC2
focus: class=valheim.x86_64 initial=valheim.x86_64 exe=valheim.x86_64 -> typek layer 1
typek: layer 1 on
focus: class=com.mitchellh.ghostty initial=com.mitchellh.ghostty exe=ghostty -> typek none
typek: layer off
```

## Commands

```
qmk-autolayer [-v] [--config PATH]                       run (this is what the unit does)
qmk-autolayer list                                       show QMK raw-HID devices
qmk-autolayer set <layer> on|off [--keyboard NAME]       one-shot, for checking the wiring
```

Logs: `journalctl --user -u qmk-autolayer -f` (or `just tail`).

## Behaviour notes

- Keyboard unplugged or in DFU: detected immediately, the node is re-found
  when it returns, layers are cleared and the current focus re-applied.
- Hyprland restarted: the daemon reconnects and rediscovers the new
  instance socket.
- Daemon stopped while a layer is on: the layer stays on until the next
  run clears it, or you toggle it yourself.
- Several keyboards: list each under `[[keyboard]]`; rules apply to all of
  them unless they name one.

## Other compositors

Only Hyprland today. A backend is two functions in one module
(`src/hypr.rs`): stream focus-change events, and return the focused window's
class / initial class / pid. niri (`niri msg --json event-stream`), Sway and
i3 (`subscribe window` on the IPC socket), and X11 (`_NET_ACTIVE_WINDOW`)
would all fit. Pull requests welcome.

## Prior art

Other host-side layer switchers exist. They differ in platform and weight:
[active-app-qmk-layer-updater](https://github.com/zigotica/active-app-qmk-layer-updater)
(Node.js, macOS-first), [auto_layers](https://github.com/itsvar8/auto_layers)
(Python + Qt, Windows), and [qmk-hid-host](https://github.com/zzeneg/qmk-hid-host)
(Rust; pushes time, volume and media info to the keyboard rather than
switching layers). None is Wayland-native or dependency-free, and none is
designed to sit beside VIA on the same interface.

## License

MIT
