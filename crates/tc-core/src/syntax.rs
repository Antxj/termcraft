//! User syntax definitions ("Customize syntax highlighting"): 8 slots,
//! each with a list of patterns per color attribute. They are listed after
//! the built-in syntaxes, so the 4_36 index of slot `n` is `6 + n`.

use serde::{Deserialize, Serialize};

/// Number of user syntax slots.
pub const SLOTS: usize = 8;

/// 4_36 index of the first user syntax (after None … SQL).
pub const FIRST_CUSTOM_INDEX: usize = 6;

/// The color attributes of a definition, in dialog order.
pub const ATTRIBUTES: [&str; 8] = [
    "Underline",
    "Red",
    "Green",
    "Yellow",
    "Blue",
    "Magenta",
    "Cyan",
    "Blinking",
];

/// Longest pattern list accepted per attribute (keeps compiled patterns
/// small whatever a file contains).
pub const MAX_PATTERNS_LEN: usize = 4096;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomSyntax {
    /// Empty = unused slot.
    pub name: String,
    /// Comma-separated patterns, one list per entry of [`ATTRIBUTES`].
    pub patterns: [String; 8],
    pub case_sensitive: bool,
    /// Patterns are regular expressions ("experts only").
    pub regex: bool,
}

impl CustomSyntax {
    pub fn is_used(&self) -> bool {
        !self.name.trim().is_empty()
    }

    /// "Export syntax to file".
    pub fn to_file(&self) -> String {
        toml::to_string_pretty(self).unwrap_or_default()
    }

    /// "Import syntax from file"; over-long lists are refused.
    pub fn from_file(text: &str) -> Result<Self, String> {
        let s: Self = toml::from_str(text).map_err(|e| e.message().to_string())?;
        if s.patterns.iter().any(|p| p.len() > MAX_PATTERNS_LEN) || s.name.len() > 64 {
            return Err("this syntax definition is too large".into());
        }
        Ok(s)
    }

    fn sample(name: &str, patterns: [&str; 8], regex: bool) -> Self {
        Self {
            name: name.into(),
            patterns: patterns.map(String::from),
            case_sensitive: false,
            regex,
        }
    }
}

/// Sample definitions offered in the dialog, by button label.
pub fn samples() -> Vec<(&'static str, CustomSyntax)> {
    let ok_error = [
        "",
        "error,errors,failed,failure,fatal,critical,denied,refused,invalid,panic",
        "ok,success,succeeded,passed,done,enabled,active,running,ready",
        "warning,warn,deprecated,timeout,retry,unknown",
        "",
        "",
        "",
        "",
    ];
    let shell = [
        "",
        "",
        "",
        "",
        "if,then,else,elif,fi,for,while,until,do,done,case,esac,function,return",
        "export,local,readonly,declare,unset,source,alias,exit",
        "",
        "",
    ];
    vec![
        (
            "Fill-in fields with sample entries",
            CustomSyntax::sample(
                "Sample",
                [
                    "important",
                    "error,failed",
                    "ok,success",
                    "warning",
                    "info",
                    "debug",
                    "trace",
                    "alert",
                ],
                false,
            ),
        ),
        (
            "Import \"ok/error\" syntax definition",
            CustomSyntax::sample("OK / error", ok_error, false),
        ),
        (
            "Import \"shell\" syntax definition",
            CustomSyntax::sample("Shell", shell, false),
        ),
        (
            "Import \"network\" syntax definition",
            CustomSyntax::sample(
                "Network",
                [
                    "",
                    "down,shutdown,deny,err-disabled",
                    "up,permit,connected",
                    "",
                    "interface,vlan,router,hostname",
                    "",
                    r"\b\d{1,3}\.\d{1,3}\.\d{1,3}\.\d{1,3}(/\d{1,2})?\b",
                    "",
                ],
                true,
            ),
        ),
        (
            "Import \"perl\" syntax definition",
            CustomSyntax::sample(
                "Perl",
                [
                    "",
                    "die",
                    "",
                    "",
                    "my,our,sub,use,package,require,return",
                    "if,elsif,else,unless,while,foreach,for",
                    "",
                    "",
                ],
                false,
            ),
        ),
        (
            "Import \"sql\" syntax definition",
            CustomSyntax::sample(
                "SQL",
                [
                    "",
                    "drop,delete,truncate",
                    "",
                    "",
                    "select,from,where,insert,into,values,update,set,join,on",
                    "group,order,by,having,limit,and,or,not,null",
                    "",
                    "",
                ],
                false,
            ),
        ),
        (
            "Import \"keywords+shell\" syntax definition",
            CustomSyntax::sample(
                "Keywords + shell",
                [
                    "",
                    ok_error[1],
                    ok_error[2],
                    ok_error[3],
                    shell[4],
                    shell[5],
                    "",
                    "",
                ],
                false,
            ),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_round_trip_and_limits() {
        for (_, s) in samples() {
            assert_eq!(CustomSyntax::from_file(&s.to_file()).unwrap(), s);
        }
        let mut big = CustomSyntax::default();
        big.patterns[1] = "x,".repeat(MAX_PATTERNS_LEN);
        assert!(CustomSyntax::from_file(&big.to_file()).is_err());
        assert!(CustomSyntax::from_file("not = [toml").is_err());
    }
}
