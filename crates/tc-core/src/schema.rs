//! Session parameter schema: which parameters exist per session type, their
//! labels, defaults and value domains. The table itself is generated from the
//! reference inventory (`cargo xtask schema` → `schema_gen.rs`).
//!
//! Parameter ids (`"3_7"`, `"4_2"`) are the reference's bookmark addresses,
//! so imported values can be stored losslessly in `Session::options`.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Widget {
    /// Boolean; stored as `-1` (true) / `0` (false).
    Check,
    Text,
    Number,
    File,
    Folder,
    /// One of the listed labels; stored as the index.
    Choice(&'static [&'static str]),
}

#[derive(Debug, Clone, Copy)]
pub struct Param {
    pub id: &'static str,
    pub label: &'static str,
    pub default: &'static str,
    /// Value domain as written in the reference (`yn`, `0..3`, `<PrivateKeyPath>`, …).
    pub values: &'static str,
    /// Shown in the "Basic settings" block (vs. an "Advanced" tab).
    pub basic: bool,
    pub widget: Widget,
}

#[derive(Debug, Clone, Copy)]
pub struct Section {
    /// `1_N` = session type N, `2` = terminal, `3_N` = network for type N, `4` = bookmark.
    pub key: &'static str,
    pub name: &'static str,
    pub params: &'static [Param],
}

/// Session types in reference order (index = type index, field `3_1`).
pub const SESSION_TYPES: [&str; 15] = [
    "SSH", "Telnet", "Rsh", "Xdmcp", "RDP", "VNC", "FTP", "SFTP", "Serial", "File", "Shell",
    "Browser", "Mosh", "Aws S3", "WSL",
];

pub fn sections() -> &'static [Section] {
    crate::schema_gen::SECTIONS
}

fn section(key: &str) -> Option<&'static Section> {
    sections().iter().find(|s| s.key == key)
}

/// Parameters of a session type (`[1_N]`).
pub fn type_params(type_index: usize) -> &'static [Param] {
    section(&format!("1_{type_index}")).map_or(&[], |s| s.params)
}

/// Gateway / proxy parameters for a session type (`[3_N]`), if any.
pub fn network_params(type_index: usize) -> &'static [Param] {
    section(&format!("3_{type_index}")).map_or(&[], |s| s.params)
}

/// Terminal parameters (`[2]`), shared by terminal-based sessions.
pub fn terminal_params() -> &'static [Param] {
    section("2").map_or(&[], |s| s.params)
}

/// Bookmark parameters (`[4]`), shared by all sessions.
pub fn bookmark_params() -> &'static [Param] {
    section("4").map_or(&[], |s| s.params)
}

pub fn is_true(value: &str) -> bool {
    matches!(value, "-1" | "1")
}

pub fn bool_value(b: bool) -> &'static str {
    if b { "-1" } else { "0" }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_table_is_consistent() {
        assert_eq!(
            type_params(0).iter().find(|p| p.id == "3_2").unwrap().label,
            "Remote host"
        );
        assert!(!terminal_params().is_empty());
        assert!(!bookmark_params().is_empty());
        for s in sections() {
            let mut ids: Vec<_> = s.params.iter().map(|p| p.id).collect();
            ids.sort_unstable();
            let n = ids.len();
            ids.dedup();
            assert_eq!(n, ids.len(), "duplicate ids in section {}", s.key);
            for p in s.params {
                if let Widget::Choice(items) = p.widget
                    && let Ok(i) = p.default.parse::<usize>()
                {
                    assert!(i < items.len(), "{}:{} default out of range", s.key, p.id);
                }
            }
        }
    }
}
