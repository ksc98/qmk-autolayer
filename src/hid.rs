//! QMK raw-HID devices as hidraw nodes.
//!
//! A QMK board with `RAW_ENABLE` (or VIA) exposes an interface whose report
//! descriptor starts with usage page 0xFF60 / usage 0x61. That, not the node
//! number, identifies it: numbers move on every replug.

use crate::config::Keyboard;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use std::thread;

/// QMK raw HID report size. The hidraw write is prefixed with a 0 report id.
pub const REPORT_LEN: usize = 32;
const RAW_HID_DESC_PREFIX: [u8; 5] = [0x06, 0x60, 0xFF, 0x09, 0x61];

#[derive(Debug, Clone)]
pub struct Device {
    pub path: PathBuf,
    pub vid: u16,
    pub pid: u16,
    pub name: String,
}

impl std::fmt::Display for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{} {:04x}:{:04x} {}",
            self.path.display(),
            self.vid,
            self.pid,
            self.name
        )
    }
}

/// Every QMK raw-HID node on the system, in node order.
pub fn list() -> Vec<Device> {
    let Ok(dir) = fs::read_dir("/sys/class/hidraw") else {
        return Vec::new();
    };
    let mut names: Vec<_> = dir.flatten().map(|e| e.file_name()).collect();
    names.sort_by_key(|n| {
        // hidraw10 after hidraw9, not after hidraw1.
        n.to_string_lossy()
            .trim_start_matches("hidraw")
            .parse::<u32>()
            .unwrap_or(u32::MAX)
    });
    names.into_iter().filter_map(|n| describe(&n)).collect()
}

fn describe(name: &std::ffi::OsStr) -> Option<Device> {
    let sys = Path::new("/sys/class/hidraw").join(name).join("device");
    let desc = fs::read(sys.join("report_descriptor")).ok()?;
    if !desc.starts_with(&RAW_HID_DESC_PREFIX) {
        return None;
    }
    let uevent = fs::read_to_string(sys.join("uevent")).ok()?;
    let mut vid = 0;
    let mut pid = 0;
    let mut hid_name = String::new();
    for line in uevent.lines() {
        if let Some(id) = line.strip_prefix("HID_ID=") {
            // bus:vendor:product, each 4 or 8 hex digits.
            let mut parts = id.split(':').skip(1);
            vid = parts
                .next()
                .and_then(|s| u32::from_str_radix(s, 16).ok())
                .unwrap_or(0) as u16;
            pid = parts
                .next()
                .and_then(|s| u32::from_str_radix(s, 16).ok())
                .unwrap_or(0) as u16;
        } else if let Some(n) = line.strip_prefix("HID_NAME=") {
            hid_name = n.to_owned();
        }
    }
    Some(Device {
        path: Path::new("/dev").join(name),
        vid,
        pid,
        name: hid_name,
    })
}

/// First raw-HID node matching the keyboard's vid/pid filters that is not
/// already in `claimed`.
pub fn find(kb: &Keyboard, claimed: &[PathBuf]) -> Option<Device> {
    list().into_iter().find(|d| {
        kb.vid.is_none_or(|v| v == d.vid) && kb.pid.is_none_or(|p| p == d.pid) && !claimed.contains(&d.path)
    })
}

pub struct Hid {
    pub dev: Device,
    file: File,
    command: u8,
}

impl Hid {
    pub fn open(dev: Device, command: u8) -> io::Result<Hid> {
        let file = OpenOptions::new().read(true).write(true).open(&dev.path)?;
        Ok(Hid { dev, file, command })
    }

    /// `[command, layer, on]`; the firmware does `layer_on` / `layer_off`.
    pub fn send(&mut self, layer: u8, on: bool) -> io::Result<()> {
        // hidraw: byte 0 is the report id (0 = the device has none), then
        // the report itself.
        let mut buf = [0u8; 1 + REPORT_LEN];
        buf[1] = self.command;
        buf[2] = layer;
        buf[3] = on as u8;
        self.file.write_all(&buf)
    }

    /// Block on reads of the same device so an unplug surfaces promptly
    /// (the read fails) instead of at the next transition. Reads also drain
    /// any input reports the interface produces so they never pile up.
    pub fn spawn_watch<E: Send + 'static>(&self, tx: Sender<E>, gone: impl FnOnce() -> E + Send + 'static) {
        let Ok(mut f) = self.file.try_clone() else { return };
        thread::spawn(move || {
            let mut buf = [0u8; 64];
            while let Ok(n) = f.read(&mut buf) {
                if n == 0 {
                    break;
                }
            }
            tx.send(gone()).ok();
        });
    }
}
