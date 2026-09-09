//! qmk-autolayer: turn QMK keyboard layers on and off from the focused
//! window on Linux.
//!
//! Compositor side (`hypr.rs`): focus-change events plus a query for the
//! focused window's class / initial class / pid; the executable name comes
//! from `/proc`. Keyboard side (`hid.rs`): a 32-byte raw-HID report
//! `[command, layer, on]` written to the board's hidraw node, handled by a
//! few lines of firmware (see README).
//!
//! Only transitions are sent, so a layer toggled by hand outside of any rule
//! is left alone. At startup, and whenever a keyboard (re)appears, every layer
//! the rules can set is turned off first so state never sticks.

mod config;
mod hid;
mod hypr;
mod window;

use config::{Config, Keyboard, Watched};
use hid::Hid;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::mpsc::{self, Sender};
use std::thread;
use std::time::Duration;

const RETRY: Duration = Duration::from_secs(1);

pub enum Event {
    /// Focus changed (or startup / reconnect): re-evaluate the active window.
    Focus,
    /// Keyboard `i`'s hidraw node went away (unplug, DFU): rediscover.
    HidGone(usize),
    /// A hidraw node keyboard `i` could use showed up.
    HidBack(usize),
}

fn usage() -> ExitCode {
    eprintln!(
        "usage: qmk-autolayer [-v] [--config PATH]        run the daemon\n       \
         qmk-autolayer list                            show QMK raw-HID devices\n       \
         qmk-autolayer set <layer> on|off [--config PATH] [--keyboard NAME]"
    );
    ExitCode::from(2)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut verbose = false;
    let mut config_path: Option<PathBuf> = None;
    let mut keyboard: Option<String> = None;
    let mut positional = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-v" | "--verbose" => verbose = true,
            "--config" => match it.next() {
                Some(p) => config_path = Some(PathBuf::from(p)),
                None => return usage(),
            },
            "--keyboard" => match it.next() {
                Some(k) => keyboard = Some(k),
                None => return usage(),
            },
            "-h" | "--help" => return usage(),
            _ => positional.push(a),
        }
    }
    let config_path = config_path.unwrap_or_else(config::default_path);

    match positional.first().map(String::as_str) {
        None => run(config_path, verbose),
        Some("list") => {
            let devs = hid::list();
            if devs.is_empty() {
                eprintln!(
                    "no QMK raw-HID devices found (is RAW_ENABLE / VIA on, and is the hidraw node readable?)"
                );
                return ExitCode::FAILURE;
            }
            for d in devs {
                println!("{d}");
            }
            ExitCode::SUCCESS
        }
        Some("set") => {
            let (Some(layer), Some(state)) = (positional.get(1), positional.get(2)) else {
                return usage();
            };
            let Ok(layer) = layer.parse::<u8>() else {
                return usage();
            };
            let on = match state.as_str() {
                "on" => true,
                "off" => false,
                _ => return usage(),
            };
            set(config_path, keyboard, layer, on)
        }
        Some(_) => usage(),
    }
}

/// Load the config, or an empty one (any keyboard, no rules) when the file
/// doesn't exist yet.
fn load_or_default(path: &Path) -> Result<Config, String> {
    if path.exists() {
        config::load(path)
    } else {
        eprintln!("config: {} not found, using defaults (no rules)", path.display());
        Ok(Config::default())
    }
}

fn set(config_path: PathBuf, keyboard: Option<String>, layer: u8, on: bool) -> ExitCode {
    let cfg = match load_or_default(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let mut claimed = Vec::new();
    let mut ok = true;
    for kb in cfg.keyboards_or_default() {
        if keyboard.as_deref().is_some_and(|k| k != kb.label()) {
            continue;
        }
        let Some(dev) = hid::find(&kb, &claimed) else {
            eprintln!("{}: no matching device", kb.label());
            ok = false;
            continue;
        };
        claimed.push(dev.path.clone());
        match Hid::open(dev, kb.command()).and_then(|mut h| h.send(layer, on).map(|()| h)) {
            Ok(h) => eprintln!(
                "{}: layer {layer} {} ({})",
                kb.label(),
                if on { "on" } else { "off" },
                h.dev
            ),
            Err(e) => {
                eprintln!("{}: {e}", kb.label());
                ok = false;
            }
        }
    }
    if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// One configured keyboard and what we last did to it.
struct Slot {
    kb: Keyboard,
    hid: Option<Hid>,
    /// Layer we turned on and have not yet turned off, on the current device.
    active: Option<u8>,
    /// Layer the current focus asks for.
    target: Option<u8>,
}

fn run(config_path: PathBuf, verbose: bool) -> ExitCode {
    let cfg = match load_or_default(&config_path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };
    let mut slots: Vec<Slot> = cfg
        .keyboards_or_default()
        .into_iter()
        .map(|kb| Slot {
            kb,
            hid: None,
            active: None,
            target: None,
        })
        .collect();
    let mut cfg = Watched::new(config_path.clone(), cfg);
    eprintln!(
        "qmk-autolayer: {} rule(s), {} keyboard(s), config {}",
        cfg.config.rules.len(),
        slots.len(),
        config_path.display()
    );

    let (tx, rx) = mpsc::channel();
    hypr::events(tx.clone());
    for i in 0..slots.len() {
        attach(&mut slots, i, &cfg.config, &tx);
    }

    tx.send(Event::Focus).ok();
    for ev in rx {
        match ev {
            Event::Focus => {
                if cfg.refresh() {
                    let labels: Vec<String> = cfg
                        .config
                        .keyboards_or_default()
                        .iter()
                        .map(Keyboard::label)
                        .collect();
                    if labels != slots.iter().map(|s| s.kb.label()).collect::<Vec<_>>() {
                        eprintln!("config: keyboard list changed; restart to apply");
                    }
                }
                let win = hypr::active_window();
                let exe = win.as_ref().and_then(window::Window::exe).unwrap_or_default();
                let candidates: Vec<&str> = match &win {
                    Some(w) => vec![w.class.as_str(), w.initial_class.as_str(), exe.as_str()],
                    None => Vec::new(),
                };
                for slot in &mut slots {
                    slot.target = cfg.config.layer_for(&slot.kb.label(), &candidates);
                }
                if verbose {
                    match &win {
                        Some(w) => eprintln!(
                            "focus: class={} initial={} exe={} -> {}",
                            w.class,
                            w.initial_class,
                            exe,
                            describe_targets(&slots)
                        ),
                        None => eprintln!("focus: no window -> {}", describe_targets(&slots)),
                    }
                }
            }
            Event::HidGone(i) => {
                let slot = &mut slots[i];
                if slot.hid.take().is_some() {
                    eprintln!("{}: keyboard gone", slot.kb.label());
                    slot.active = None;
                    wait_for_device(i, slot.kb.clone(), &tx);
                }
            }
            Event::HidBack(i) => {
                if slots[i].hid.is_none() {
                    attach(&mut slots, i, &cfg.config, &tx);
                }
            }
        }
        for (i, slot) in slots.iter_mut().enumerate() {
            if let Err(e) = reconcile(slot) {
                eprintln!("{}: write: {e}", slot.kb.label());
                tx.send(Event::HidGone(i)).ok();
            }
        }
    }
    ExitCode::SUCCESS
}

fn describe_targets(slots: &[Slot]) -> String {
    slots
        .iter()
        .map(|s| match s.target {
            Some(l) => format!("{} layer {l}", s.kb.label()),
            None => format!("{} none", s.kb.label()),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Open the device for slot `i` (if present), clear every layer the rules
/// can set on it, and arm the unplug watcher. Otherwise poll for it.
fn attach(slots: &mut [Slot], i: usize, cfg: &Config, tx: &Sender<Event>) {
    let claimed: Vec<PathBuf> = slots
        .iter()
        .filter_map(|s| s.hid.as_ref().map(|h| h.dev.path.clone()))
        .collect();
    let slot = &mut slots[i];
    let label = slot.kb.label();
    let Some(dev) = hid::find(&slot.kb, &claimed) else {
        eprintln!("{label}: keyboard not found, waiting");
        wait_for_device(i, slot.kb.clone(), tx);
        return;
    };
    let mut h = match Hid::open(dev, slot.kb.command()) {
        Ok(h) => h,
        Err(e) => {
            eprintln!("{label}: open: {e}");
            wait_for_device(i, slot.kb.clone(), tx);
            return;
        }
    };
    eprintln!("{label}: {}", h.dev);
    for layer in cfg.layers_for(&label) {
        if let Err(e) = h.send(layer, false) {
            eprintln!("{label}: write: {e}");
            wait_for_device(i, slot.kb.clone(), tx);
            return;
        }
    }
    h.spawn_watch(tx.clone(), move || Event::HidGone(i));
    slot.hid = Some(h);
    slot.active = None;
}

/// Poll sysfs until a node the keyboard could use exists, then `HidBack`.
fn wait_for_device(i: usize, kb: Keyboard, tx: &Sender<Event>) {
    let tx = tx.clone();
    thread::spawn(move || loop {
        thread::sleep(RETRY);
        if hid::find(&kb, &[]).is_some() {
            tx.send(Event::HidBack(i)).ok();
            return;
        }
    });
}

/// Bring the keyboard's layer state to `target`, sending only what changed.
fn reconcile(slot: &mut Slot) -> std::io::Result<()> {
    if slot.active == slot.target {
        return Ok(());
    }
    let Some(h) = slot.hid.as_mut() else { return Ok(()) };
    if let Some(a) = slot.active {
        h.send(a, false)?;
    }
    if let Some(t) = slot.target {
        h.send(t, true)?;
    }
    slot.active = slot.target;
    match slot.target {
        Some(t) => eprintln!("{}: layer {t} on", slot.kb.label()),
        None => eprintln!("{}: layer off", slot.kb.label()),
    }
    Ok(())
}
