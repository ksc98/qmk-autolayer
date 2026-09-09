//! Hyprland backend: focus-change events from `.socket2.sock`, the focused
//! window's identity from an `activewindow` query on `.socket.sock`.
//!
//! Other compositors slot in beside this module with the same two entry
//! points: `events(tx)` (send `Event::Focus` on every focus change) and
//! `active_window()`.

use crate::window::Window;
use crate::Event;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, SystemTime};

const RETRY: Duration = Duration::from_secs(1);

/// `$XDG_RUNTIME_DIR/hypr/<instance>`: the instance from the environment,
/// else the most recently started one that still has a socket (a compositor
/// restart under a long-lived daemon).
fn instance_dir() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var_os("XDG_RUNTIME_DIR")?).join("hypr");
    if let Some(sig) = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE") {
        let d = root.join(sig);
        if d.join(".socket2.sock").exists() {
            return Some(d);
        }
    }
    let mut best: Option<(SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(&root).ok()?.flatten() {
        let d = entry.path();
        let Ok(meta) = fs::metadata(d.join(".socket2.sock")) else {
            continue;
        };
        let t = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        if best.as_ref().is_none_or(|(bt, _)| t > *bt) {
            best = Some((t, d));
        }
    }
    best.map(|(_, d)| d)
}

/// `None` when Hyprland is down or nothing is focused (it answers `Invalid`).
pub fn active_window() -> Option<Window> {
    let dir = instance_dir()?;
    let mut s = UnixStream::connect(dir.join(".socket.sock")).ok()?;
    s.write_all(b"activewindow").ok()?;
    let mut out = String::new();
    s.read_to_string(&mut out).ok()?;
    parse(&out)
}

/// Text form: one `\tkey: value` per line after the header.
fn parse(out: &str) -> Option<Window> {
    let mut w = Window::default();
    let mut seen = false;
    for line in out.lines() {
        let line = line.trim_start();
        if let Some(v) = line.strip_prefix("class: ") {
            w.class = v.trim().to_owned();
            seen = true;
        } else if let Some(v) = line.strip_prefix("initialClass: ") {
            w.initial_class = v.trim().to_owned();
        } else if let Some(v) = line.strip_prefix("title: ") {
            w.title = v.trim().to_owned();
        } else if let Some(v) = line.strip_prefix("pid: ") {
            w.pid = v.trim().parse().ok();
        }
    }
    seen.then_some(w)
}

/// Forward every `activewindow>>` event as `Event::Focus`, reconnecting
/// (with instance rediscovery) on EOF or error.
pub fn events(tx: Sender<Event>) {
    thread::spawn(move || loop {
        let Some(dir) = instance_dir() else {
            thread::sleep(RETRY);
            continue;
        };
        let stream = match UnixStream::connect(dir.join(".socket2.sock")) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("hyprland: connect {}: {e}", dir.display());
                thread::sleep(RETRY);
                continue;
            }
        };
        // Focus may have changed while we were disconnected.
        if tx.send(Event::Focus).is_err() {
            return;
        }
        for line in BufReader::new(stream).lines() {
            match line {
                Ok(l) if l.starts_with("activewindow>>") => {
                    if tx.send(Event::Focus).is_err() {
                        return;
                    }
                }
                Ok(_) => {}
                Err(_) => break,
            }
        }
        eprintln!("hyprland: event socket closed, reconnecting");
        thread::sleep(RETRY);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_activewindow_text() {
        let out = "Window 55c053241920 -> Terraria:\n\tmapped: 1\n\tclass: steam_app_105600\n\
                   \ttitle: Terraria: class: x\n\tinitialClass: steam_app_105600\n\tpid: 4242\n";
        let w = parse(out).unwrap();
        assert_eq!(w.class, "steam_app_105600");
        assert_eq!(w.initial_class, "steam_app_105600");
        assert_eq!(w.title, "Terraria: class: x");
        assert_eq!(w.pid, Some(4242));
    }

    #[test]
    fn no_window_is_none() {
        assert!(parse("Invalid").is_none());
        assert!(parse("").is_none());
    }
}
