use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub terminal: TerminalSettings,
    pub update: UpdateSettings,
    pub view: ViewSettings,
    pub passwords: PasswordSettings,
    pub ssh: SshSettings,
    pub general: GeneralSettings,
    /// Keyboard shortcuts changed from their defaults: action id →
    /// hotkey (`<none>` removes one). See `crate::hotkeys`.
    pub hotkeys: std::collections::BTreeMap<String, String>,
}

/// Configuration › General.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    /// "Automatically backup TermCraft configuration file".
    pub auto_backup: bool,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        Self { auto_backup: true }
    }
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
    /// "Remote-monitoring": resource bar below SSH terminals.
    pub remote_monitoring: bool,
    /// "Use Windows SSH agent" (the OpenSSH agent; `SSH_AUTH_SOCK` on Unix).
    pub use_system_agent: bool,
    /// "Use PuTTY agent" (Pageant, Windows).
    pub use_pageant: bool,
    /// "Use internal SSH agent": these key files are offered to every
    /// SSH server (passphrases asked, or taken from the saved ones).
    pub internal_agent: bool,
    pub agent_keys: Vec<String>,
}

impl Default for SshSettings {
    fn default() -> Self {
        Self {
            browser: true,
            switch_to_browser: true,
            keepalive_secs: 0,
            remote_monitoring: false,
            use_system_agent: true,
            use_pageant: true,
            internal_agent: false,
            agent_keys: Vec::new(),
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

/// "Prompt me for my master password" (configuration file storage).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum MasterPrompt {
    /// The key is cached in the OS keychain: ask on a new account or computer.
    #[default]
    NewComputer,
    /// Ask at every TermCraft startup.
    EveryStartup,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PasswordSettings {
    pub save: SavePasswords,
    /// "Save SSH keys passphrases as well".
    pub save_key_passphrases: bool,
    pub master_prompt: MasterPrompt,
}

/// "Skin selection" (Configuration › Display).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    /// Follow the OS light / dark mode.
    System,
    /// Light grey and blue, like the reference (default).
    #[default]
    Light,
    /// Between light and dark.
    Dim,
    Dark,
}

impl Theme {
    pub const ALL: [Theme; 4] = [Self::Light, Self::Dim, Self::Dark, Self::System];

    pub fn label(self) -> &'static str {
        match self {
            Self::System => "Follow system",
            Self::Light => "Light",
            Self::Dim => "Dim",
            Self::Dark => "Dark",
        }
    }
}

/// "Graphical interface elements size".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UiSize {
    #[default]
    Autodetect,
    Small,
    Normal,
    Big,
    Huge,
    HighDpi4k,
    HighDpiBig,
}

impl UiSize {
    pub const ALL: [UiSize; 7] = [
        Self::Autodetect,
        Self::Small,
        Self::Normal,
        Self::Big,
        Self::Huge,
        Self::HighDpi4k,
        Self::HighDpiBig,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Autodetect => "Autodetect",
            Self::Small => "Small",
            Self::Normal => "Normal",
            Self::Big => "Big",
            Self::Huge => "Huge",
            Self::HighDpi4k => "HighDPI 4K",
            Self::HighDpiBig => "HighDPI big",
        }
    }

    /// Zoom on top of the OS scale factor (`Autodetect` = the OS factor).
    pub fn zoom(self) -> f32 {
        match self {
            Self::Autodetect | Self::Normal => 1.0,
            Self::Small => 0.85,
            Self::Big => 1.15,
            Self::Huge => 1.3,
            Self::HighDpi4k => 1.5,
            Self::HighDpiBig => 1.75,
        }
    }
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
    /// "Rounded tabs".
    pub rounded_tabs: bool,
    /// "Graphical interface elements size".
    pub ui_size: UiSize,
    /// "Show green/orange buttons next to each open session in the sessions tree".
    pub session_dots: bool,
    /// "Compact mode": only the tabs and terminals.
    pub compact: bool,
    /// "Color saturation" of the skin, in percent (36 = as designed).
    pub saturation: u8,
    /// "Transparency" of the main window, in percent (0 = opaque).
    pub transparency: u8,
    /// "Detached/fullscreen transparency", in percent.
    pub detached_transparency: u8,
}

/// Highest window transparency offered (beyond it the text is unreadable).
pub const MAX_TRANSPARENCY: u8 = 80;

/// The skin saturation that leaves its colors unchanged.
pub const DEFAULT_SATURATION: u8 = 36;

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
            rounded_tabs: true,
            ui_size: UiSize::default(),
            session_dots: true,
            compact: false,
            saturation: DEFAULT_SATURATION,
            transparency: 0,
            detached_transparency: 0,
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
    /// "Copy on select": selected text goes to the clipboard.
    pub copy_on_select: bool,
    /// Ask before pasting text with several lines.
    pub warn_multiline_paste: bool,
    /// Characters that end a word on double click (empty = default set).
    pub word_delimiters: String,
    /// "Log terminal output to the following directory" (empty = off).
    pub log_dir: String,
    /// "Use Windows PATH" in the local Bash / Zsh (Git for Windows, MSYS2).
    pub use_windows_path: bool,
    /// "Syntax highlighting", as the session 4_36 index (1 = standard
    /// keywords); sessions may choose their own.
    pub syntax_highlighting: usize,
    /// "Customize": the 8 user syntax definitions.
    pub custom_syntaxes: [crate::syntax::CustomSyntax; crate::syntax::SLOTS],
    /// "Expert settings".
    pub expert: ExpertTerminal,
}

/// Configuration › Terminal › "Expert settings" (reference
/// `TFORMADVANCEDTERMSETTINGS`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExpertTerminal {
    /// "Display scrollbar".
    pub scrollbar: bool,
    /// "Force local echo on / off" (`None` = the remote side echoes).
    pub local_echo: Option<bool>,
    /// "Implicit CR in every LF".
    pub implicit_cr_in_lf: bool,
    /// "Implicit LF in every CR".
    pub implicit_lf_in_cr: bool,
    /// "Flush log files frequently": every write reaches the disk.
    pub flush_logs: bool,
    /// "Enable xterm-style mouse reporting".
    pub mouse_reporting: bool,
    /// "Enable bracketed-paste".
    pub bracketed_paste: bool,
    /// "Force application cursor keys mode on / off" (`None` = the
    /// program decides).
    pub app_cursor: Option<bool>,
    /// "Answerback to ^E".
    pub answerback: String,
    /// "Bell Type".
    pub bell: BellType,
    /// "Reconnection keys" of the "Session stopped" message, by key name
    /// (`Any other key`, `RETURN`, `F1`, `R` …).
    pub exit_key: String,
    pub reconnect_key: String,
    pub save_key: String,
    /// Double-click word delimiters added to / removed from the default
    /// set, on each side of the word.
    pub add_left_delimiters: String,
    pub add_right_delimiters: String,
    pub remove_left_delimiters: String,
    pub remove_right_delimiters: String,
}

impl Default for ExpertTerminal {
    fn default() -> Self {
        Self {
            scrollbar: true,
            local_echo: None,
            implicit_cr_in_lf: false,
            implicit_lf_in_cr: false,
            flush_logs: false,
            mouse_reporting: true,
            bracketed_paste: true,
            app_cursor: None,
            answerback: "xterm-256color".into(),
            bell: BellType::default(),
            exit_key: ANY_OTHER_KEY.into(),
            reconnect_key: "R".into(),
            save_key: "S".into(),
            add_left_delimiters: String::new(),
            add_right_delimiters: String::new(),
            remove_left_delimiters: String::new(),
            remove_right_delimiters: String::new(),
        }
    }
}

/// The "Exit" reconnection key that accepts any key but the other two.
pub const ANY_OTHER_KEY: &str = "Any other key";

/// Key names of the reconnection key lists, in reference order.
pub const RECONNECTION_KEYS: &[&str] = &[
    "F1",
    "F2",
    "F3",
    "F4",
    "F5",
    "F6",
    "F7",
    "F8",
    "F9",
    "F10",
    "F12",
    "SPACE",
    "HOME",
    "END",
    "PAGE_UP",
    "PAGE_DOWN",
    "DEL",
    "ESC",
    "TAB",
    "RETURN",
    "SUBTRACT",
    "DIVIDE",
    "ADD",
    "MULTIPLY",
    "0",
    "1",
    "2",
    "3",
    "4",
    "5",
    "6",
    "7",
    "8",
    "9",
    "A",
    "B",
    "C",
    "D",
    "E",
    "F",
    "G",
    "H",
    "I",
    "J",
    "K",
    "L",
    "M",
    "N",
    "O",
    "P",
    "Q",
    "R",
    "S",
    "T",
    "U",
    "V",
    "W",
    "X",
    "Y",
    "Z",
];

/// "Bell Type".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum BellType {
    #[default]
    None,
    Sound,
    Visual,
    Speaker,
}

impl BellType {
    pub const ALL: [BellType; 4] = [Self::None, Self::Sound, Self::Visual, Self::Speaker];

    pub fn label(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Sound => "Default sound",
            Self::Visual => "Visual",
            Self::Speaker => "PC speakers",
        }
    }
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
            copy_on_select: true,
            warn_multiline_paste: true,
            word_delimiters: String::new(),
            log_dir: String::new(),
            use_windows_path: true,
            syntax_highlighting: 1,
            custom_syntaxes: Default::default(),
            expert: ExpertTerminal::default(),
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
