//! `$XDG_CONFIG_HOME/qmk-autolayer/config.toml`: which keyboards to drive,
//! which focused windows map to which layer.

use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Raw HID command id used when a keyboard doesn't set one. Outside VIA's
/// 0x01..=0x0F range so VIA boards can handle it in `via_command_kb`.
pub const DEFAULT_COMMAND: u8 = 0x42;

#[derive(Debug, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Layer for rules that don't name one.
    #[serde(default = "one")]
    pub default_layer: u8,
    /// Keyboards to drive. Empty: the first QMK raw-HID interface found.
    #[serde(default, rename = "keyboard")]
    pub keyboards: Vec<Keyboard>,
    /// Focused-window rules, first match wins.
    #[serde(default, rename = "rule")]
    pub rules: Vec<Rule>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Keyboard {
    /// Label for logs and for `rule.keyboard`. Defaults to `vid:pid` or `any`.
    pub name: Option<String>,
    /// USB ids (`0x7179` or `"7179"`). Omit both to take any QMK raw-HID device.
    #[serde(default, deserialize_with = "hex_u16")]
    pub vid: Option<u16>,
    #[serde(default, deserialize_with = "hex_u16")]
    pub pid: Option<u16>,
    /// Raw HID command id the firmware handles. Default 0x42.
    #[serde(default, deserialize_with = "hex_u8")]
    pub command: Option<u8>,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(deny_unknown_fields)]
pub struct Rule {
    /// Patterns matched against the focused window's class, initial class,
    /// and executable name. `*` matches any run of characters.
    #[serde(rename = "match")]
    pub patterns: Vec<String>,
    /// Layer to turn on while matched. Defaults to `default_layer`.
    pub layer: Option<u8>,
    /// Only drive the keyboard with this name. Default: all keyboards.
    pub keyboard: Option<String>,
}

fn one() -> u8 {
    1
}

impl Keyboard {
    pub fn label(&self) -> String {
        if let Some(n) = &self.name {
            return n.clone();
        }
        match (self.vid, self.pid) {
            (Some(v), Some(p)) => format!("{v:04x}:{p:04x}"),
            _ => "any".into(),
        }
    }

    pub fn command(&self) -> u8 {
        self.command.unwrap_or(DEFAULT_COMMAND)
    }
}

impl Config {
    pub fn keyboards_or_default(&self) -> Vec<Keyboard> {
        if self.keyboards.is_empty() {
            vec![Keyboard {
                name: None,
                vid: None,
                pid: None,
                command: None,
            }]
        } else {
            self.keyboards.clone()
        }
    }

    /// Layer the focused window maps to on the keyboard named `kb`, if any.
    pub fn layer_for(&self, kb: &str, candidates: &[&str]) -> Option<u8> {
        self.rules
            .iter()
            .filter(|r| r.keyboard.as_deref().is_none_or(|k| k == kb))
            .find(|r| r.patterns.iter().any(|p| candidates.iter().any(|c| glob(p, c))))
            .map(|r| r.layer.unwrap_or(self.default_layer))
    }

    /// Every layer any rule can turn on for `kb` — what to clear at startup.
    pub fn layers_for(&self, kb: &str) -> Vec<u8> {
        let mut v: Vec<u8> = self
            .rules
            .iter()
            .filter(|r| r.keyboard.as_deref().is_none_or(|k| k == kb))
            .map(|r| r.layer.unwrap_or(self.default_layer))
            .collect();
        v.sort_unstable();
        v.dedup();
        v
    }
}

/// Default location, honouring `$XDG_CONFIG_HOME`.
pub fn default_path() -> PathBuf {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .unwrap_or_default();
    base.join("qmk-autolayer/config.toml")
}

pub fn load(path: &Path) -> Result<Config, String> {
    let text = fs::read_to_string(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let cfg: Config = toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
    for r in &cfg.rules {
        if r.patterns.is_empty() {
            return Err(format!("{}: a [[rule]] has an empty `match`", path.display()));
        }
        if let Some(k) = &r.keyboard {
            if !cfg.keyboards.iter().any(|kb| kb.label() == *k) {
                return Err(format!(
                    "{}: rule refers to unknown keyboard {k:?}",
                    path.display()
                ));
            }
        }
    }
    Ok(cfg)
}

/// Re-reads the file only when its mtime changes; a broken edit keeps the
/// last good config and is reported once.
pub struct Watched {
    pub path: PathBuf,
    pub config: Config,
    mtime: Option<SystemTime>,
    failing: bool,
}

impl Watched {
    pub fn new(path: PathBuf, config: Config) -> Watched {
        let mtime = mtime_of(&path);
        Watched {
            path,
            config,
            mtime,
            failing: false,
        }
    }

    /// Returns true when a new config was loaded.
    pub fn refresh(&mut self) -> bool {
        let now = mtime_of(&self.path);
        if now == self.mtime {
            return false;
        }
        self.mtime = now;
        match load(&self.path) {
            Ok(c) => {
                self.config = c;
                self.failing = false;
                eprintln!("config: reloaded {}", self.path.display());
                true
            }
            Err(e) => {
                if !self.failing {
                    eprintln!("config: {e} (keeping previous)");
                }
                self.failing = true;
                false
            }
        }
    }
}

fn mtime_of(p: &Path) -> Option<SystemTime> {
    fs::metadata(p).and_then(|m| m.modified()).ok()
}

/// `*`-only glob, anchored at both ends.
pub fn glob(pattern: &str, s: &str) -> bool {
    fn rec(p: &[u8], s: &[u8]) -> bool {
        match p.split_first() {
            None => s.is_empty(),
            Some((b'*', rest)) => (0..=s.len()).any(|i| rec(rest, &s[i..])),
            Some((c, rest)) => s.first() == Some(c) && rec(rest, &s[1..]),
        }
    }
    rec(pattern.as_bytes(), s.as_bytes())
}

// TOML has no hex integer literals without the 0x prefix being an integer
// already, but people write ids as "7179" (a string) as often as 0x7179.
// Accept both.
fn hex_u16<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u16>, D::Error> {
    hex::<D, u16>(d, "u16")
}

fn hex_u8<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u8>, D::Error> {
    hex::<D, u8>(d, "u8")
}

fn hex<'de, D: serde::Deserializer<'de>, T: TryFrom<u32>>(d: D, what: &str) -> Result<Option<T>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Int(u32),
        Str(String),
    }
    let v = match Option::<Raw>::deserialize(d)? {
        None => return Ok(None),
        Some(Raw::Int(i)) => i,
        Some(Raw::Str(s)) => {
            let s = s.trim().trim_start_matches("0x").trim_start_matches("0X");
            u32::from_str_radix(s, 16)
                .map_err(|_| serde::de::Error::custom(format!("bad hex {what}: {s:?}")))?
        }
    };
    T::try_from(v)
        .map(Some)
        .map_err(|_| serde::de::Error::custom(format!("{what} out of range: {v:#x}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn glob_matches() {
        assert!(glob("steam_app_*", "steam_app_105600"));
        assert!(glob("*", ""));
        assert!(glob("valheim.x86_64", "valheim.x86_64"));
        assert!(!glob("valheim", "valheim.x86_64"));
        assert!(glob("*.exe", "Game.exe"));
        assert!(!glob("a*b", "ab c"));
    }

    #[test]
    fn parses_and_resolves() {
        let cfg: Config = toml::from_str(
            r#"
            default_layer = 1
            [[keyboard]]
            name = "typek"
            vid = 0x7179
            pid = "8475"
            [[rule]]
            match = ["steam_app_*", "valheim.x86_64"]
            [[rule]]
            match = ["firefox"]
            layer = 2
            keyboard = "typek"
            "#,
        )
        .unwrap();
        assert_eq!(cfg.keyboards[0].vid, Some(0x7179));
        assert_eq!(cfg.keyboards[0].pid, Some(0x8475));
        assert_eq!(cfg.keyboards[0].command(), 0x42);
        assert_eq!(cfg.layer_for("typek", &["steam_app_1"]), Some(1));
        assert_eq!(cfg.layer_for("typek", &["firefox"]), Some(2));
        assert_eq!(cfg.layer_for("other", &["firefox"]), None);
        assert_eq!(cfg.layer_for("typek", &["kitty"]), None);
        assert_eq!(cfg.layers_for("typek"), vec![1, 2]);
        assert_eq!(cfg.layers_for("other"), vec![1]);
    }

    #[test]
    fn empty_config_means_any_keyboard() {
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.default_layer, 1);
        assert_eq!(cfg.keyboards_or_default()[0].label(), "any");
    }

    #[test]
    fn rejects_unknown_keys_and_bad_refs() {
        assert!(toml::from_str::<Config>("layer = 1").is_err());
        let dir = std::env::temp_dir().join(format!("qmk-autolayer-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        fs::write(&p, "[[rule]]\nmatch = [\"x\"]\nkeyboard = \"nope\"\n").unwrap();
        assert!(load(&p).is_err());
        fs::remove_dir_all(&dir).unwrap();
    }
}
