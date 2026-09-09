//! The focused window, compositor-independent.

use std::fs;

#[derive(Debug, Default, Clone)]
pub struct Window {
    pub class: String,
    pub initial_class: String,
    pub title: String,
    pub pid: Option<u32>,
}

impl Window {
    /// Basename of `/proc/<pid>/exe`, falling back to `comm` (15-char
    /// truncated) when the link is unreadable.
    pub fn exe(&self) -> Option<String> {
        let pid = self.pid?;
        if let Ok(p) = fs::read_link(format!("/proc/{pid}/exe")) {
            if let Some(n) = p.file_name() {
                return Some(n.to_string_lossy().into_owned());
            }
        }
        fs::read_to_string(format!("/proc/{pid}/comm"))
            .ok()
            .map(|s| s.trim().to_owned())
    }
}
