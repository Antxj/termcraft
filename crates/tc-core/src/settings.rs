use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub terminal: TerminalSettings,
    pub update: UpdateSettings,
    pub view: ViewSettings,
    pub passwords: PasswordSettings,
    pub ssh: SshSettings,
}

/// Configuration › SSH.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SshSettings {
    /// "Enable graphical SSH-browser".
    pub browser: bool,
    /// "Automatically switch to SSH-browser tab after login".
    pub switch_to_browser: bool,
    /// "SSH keepalive" (seconds; 0 = off).
    pub keepalive_secs: u64,
}

impl Default for SshSettings {
    fn default() -> Self {
        Self {
            browser: true,
            switch_to_browser: true,
            keepalive_secs: 0,
        }
    }
}

/// "Save sessions passwords" (Passwords settings dialog).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SavePasswords {
    Always,
    Never,
    #[default]
    Ask,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PasswordSettings {
    pub save: SavePasswords,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum ToolbarStyle {
    Small,
    Standard,
    #[default]
    StandardWithCaptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SidebarTab {
    #[default]
    Sessions,
    Tools,
    Macros,
    Sftp,
}

/// Terminal split modes (View / Split menus).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SplitMode {
    #[default]
    Single,
    /// Two terminals side by side.
    Vertical,
    /// Two terminals one above the other.
    Horizontal,
    /// Four terminals in a 2×2 grid.
    Quad,
}

impl SplitMode {
    pub fn panes(self) -> usize {
        match self {
            Self::Single => 1,
            Self::Vertical | Self::Horizontal => 2,
            Self::Quad => 4,
        }
    }
}

/// Window chrome options (View menu).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewSettings {
    pub show_menu_bar: bool,
    pub show_toolbar: bool,
    pub toolbar_style: ToolbarStyle,
    pub sidebar_visible: bool,
    pub sidebar_right: bool,
    pub sidebar_tab: SidebarTab,
    pub show_tab_numbers: bool,
    pub show_close_button: bool,
    pub theme: Theme,
    pub split: SplitMode,
}

impl Default for ViewSettings {
    fn default() -> Self {
        Self {
            show_menu_bar: true,
            show_toolbar: true,
            toolbar_style: ToolbarStyle::default(),
            sidebar_visible: true,
            sidebar_right: false,
            sidebar_tab: SidebarTab::default(),
            show_tab_numbers: false,
            show_close_button: true,
            theme: Theme::default(),
            split: SplitMode::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TerminalSettings {
    pub font_size: f32,
    pub scrollback_lines: usize,
    /// "Close prompt": confirm before exiting with sessions open.
    pub confirm_exit: bool,
    /// "Paste using right-click".
    pub paste_right_click: bool,
    /// "Backspace sends ^H" for local terminals.
    pub backspace_ctrl_h: bool,
    /// "Local terminal environment".
    pub local_shell: LocalShell,
}

/// "Local terminal environment" (Configuration › Terminal) and the Shell
/// session's "Terminal shell".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum LocalShell {
    #[default]
    Bash,
    Zsh,
    Cmd,
    PowerShell,
    Wsl,
}

impl LocalShell {
    pub const ALL: [LocalShell; 5] = [
        Self::Bash,
        Self::Zsh,
        Self::Cmd,
        Self::PowerShell,
        Self::Wsl,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Bash => "Bash",
            Self::Zsh => "Zsh",
            Self::Cmd => "Cmd",
            Self::PowerShell => "PowerShell",
            Self::Wsl => "Windows Subsystem for Linux (WSL)",
        }
    }

    /// Shell session option `3_2` ("Terminal shell"): `0` = the
    /// configured default, then Bash, Zsh, Cmd, PowerShell, WSL.
    pub fn from_option(value: &str) -> Option<Self> {
        match value.trim() {
            "1" => Some(Self::Bash),
            "2" => Some(Self::Zsh),
            "3" => Some(Self::Cmd),
            "4" => Some(Self::PowerShell),
            "5" => Some(Self::Wsl),
            _ => None,
        }
    }

    pub fn option_value(shell: Option<Self>) -> &'static str {
        match shell {
            None => "0",
            Some(Self::Bash) => "1",
            Some(Self::Zsh) => "2",
            Some(Self::Cmd) => "3",
            Some(Self::PowerShell) => "4",
            Some(Self::Wsl) => "5",
        }
    }
}

impl Default for TerminalSettings {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            scrollback_lines: 10_000,
            confirm_exit: true,
            paste_right_click: true,
            backspace_ctrl_h: false,
            local_shell: LocalShell::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateSettings {
    /// Check for updates at startup.
    pub check_on_startup: bool,
    /// Download and apply updates without asking.
    pub auto_install: bool,
    pub channel: UpdateChannel,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            check_on_startup: true,
            auto_install: false,
            channel: UpdateChannel::Stable,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_shell_option_round_trip() {
        assert_eq!(LocalShell::from_option("0"), None);
        assert_eq!(LocalShell::from_option(""), None);
        for shell in LocalShell::ALL {
            let v = LocalShell::option_value(Some(shell));
            assert_eq!(LocalShell::from_option(v), Some(shell));
        }
        let s: Settings = toml::from_str("[terminal]\nlocal_shell = \"power_shell\"\n").unwrap();
        assert_eq!(s.terminal.local_shell, LocalShell::PowerShell);
        assert_eq!(Settings::default().terminal.local_shell, LocalShell::Bash);
    }
}
