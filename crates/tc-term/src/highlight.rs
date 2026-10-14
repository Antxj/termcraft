//! Syntax highlighting of terminal output (session 4_36 / Configuration
//! "Syntax highlighting"). Applied when drawing, only to cells the program
//! left in the default color, so it never fights the program's own colors.
//!
//! A small hand-written scanner (no regex): words, IPv4 addresses,
//! quoted strings, comments and variables, depending on the syntax.

/// The 4_36 choices, in reference order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Syntax {
    None,
    /// "Standard keywords (OK/warning/error/...)", the default.
    #[default]
    Standard,
    Shell,
    Cisco,
    Perl,
    Sql,
}

impl Syntax {
    pub fn from_index(index: usize) -> Self {
        match index {
            0 => Self::None,
            2 => Self::Shell,
            3 => Self::Cisco,
            4 => Self::Perl,
            5 => Self::Sql,
            _ => Self::Standard,
        }
    }
}

/// What a span is; the renderer maps it to a palette color.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Error,
    Warning,
    Ok,
    Keyword,
    Address,
    Text,
    Comment,
    Variable,
    Number,
}

impl Kind {
    /// ANSI palette index used for this kind.
    pub fn ansi(self) -> u8 {
        match self {
            Kind::Error => 1,
            Kind::Warning => 3,
            Kind::Ok | Kind::Text => 2,
            Kind::Keyword => 5,
            Kind::Address | Kind::Variable | Kind::Number => 6,
            Kind::Comment => 8,
        }
    }
}

/// A highlighted range of a line, in character positions (`start..end`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

const STD_ERROR: &[&str] = &[
    "error",
    "errors",
    "err",
    "fail",
    "failed",
    "failure",
    "fatal",
    "critical",
    "crit",
    "denied",
    "refused",
    "invalid",
    "panic",
    "exception",
    "abort",
    "aborted",
    "unreachable",
    "ko",
    "alert",
    "emerg",
    "emergency",
];
const STD_WARNING: &[&str] = &[
    "warning",
    "warnings",
    "warn",
    "deprecated",
    "timeout",
    "retry",
    "retrying",
    "unknown",
    "missing",
    "notice",
];
const STD_OK: &[&str] = &[
    "ok",
    "success",
    "successful",
    "successfully",
    "succeeded",
    "passed",
    "done",
    "complete",
    "completed",
    "enabled",
    "active",
    "running",
    "ready",
    "connected",
    "accepted",
    "granted",
    "online",
];
const SHELL_KEYWORDS: &[&str] = &[
    "if", "then", "else", "elif", "fi", "for", "while", "until", "do", "done", "case", "esac",
    "in", "function", "return", "export", "local", "readonly", "declare", "unset", "source",
    "alias", "exit", "break", "continue", "select", "time", "trap", "shift", "set", "eval", "exec",
];
const CISCO_BAD: &[&str] = &[
    "shutdown",
    "down",
    "deny",
    "no",
    "err-disabled",
    "notconnect",
];
const CISCO_GOOD: &[&str] = &["up", "permit", "connected", "enabled"];
const CISCO_KEYWORDS: &[&str] = &[
    "interface",
    "ip",
    "address",
    "router",
    "vlan",
    "hostname",
    "description",
    "switchport",
    "access-list",
    "access-group",
    "line",
    "network",
    "route",
    "mode",
    "trunk",
    "access",
    "spanning-tree",
    "service",
    "enable",
    "username",
    "password",
    "secret",
    "ospf",
    "bgp",
    "eigrp",
    "neighbor",
    "area",
    "channel-group",
    "standby",
    "logging",
    "snmp-server",
    "ntp",
];
const PERL_KEYWORDS: &[&str] = &[
    "my", "our", "local", "sub", "use", "no", "package", "require", "if", "elsif", "else",
    "unless", "while", "until", "for", "foreach", "last", "next", "redo", "return", "do", "eval",
    "die", "warn", "print", "printf", "and", "or", "not", "eq", "ne", "lt", "gt", "le", "ge",
    "cmp", "qw", "bless", "ref", "undef", "defined", "exists", "delete", "shift", "push", "pop",
];
const SQL_KEYWORDS: &[&str] = &[
    "select",
    "from",
    "where",
    "insert",
    "into",
    "values",
    "update",
    "set",
    "delete",
    "create",
    "table",
    "drop",
    "alter",
    "add",
    "column",
    "index",
    "view",
    "join",
    "left",
    "right",
    "inner",
    "outer",
    "full",
    "cross",
    "on",
    "group",
    "by",
    "order",
    "having",
    "and",
    "or",
    "not",
    "null",
    "is",
    "as",
    "distinct",
    "limit",
    "offset",
    "union",
    "all",
    "in",
    "exists",
    "between",
    "like",
    "case",
    "when",
    "then",
    "else",
    "end",
    "primary",
    "key",
    "foreign",
    "references",
    "default",
    "asc",
    "desc",
    "begin",
    "commit",
    "rollback",
    "grant",
    "revoke",
    "count",
    "sum",
    "avg",
    "min",
    "max",
];

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_kind(syntax: Syntax, word: &str, at_line_start: bool) -> Option<Kind> {
    let lower = word.to_ascii_lowercase();
    let w = lower.as_str();
    match syntax {
        Syntax::None => None,
        Syntax::Standard => {
            if STD_ERROR.contains(&w) {
                Some(Kind::Error)
            } else if STD_WARNING.contains(&w) {
                Some(Kind::Warning)
            } else if STD_OK.contains(&w) {
                Some(Kind::Ok)
            } else {
                None
            }
        }
        Syntax::Shell => SHELL_KEYWORDS.contains(&word).then_some(Kind::Keyword),
        Syntax::Cisco => {
            if CISCO_BAD.contains(&w) && (w != "no" || at_line_start) {
                Some(Kind::Error)
            } else if CISCO_GOOD.contains(&w) {
                Some(Kind::Ok)
            } else if CISCO_KEYWORDS.contains(&w) {
                Some(Kind::Keyword)
            } else {
                None
            }
        }
        Syntax::Perl => PERL_KEYWORDS.contains(&word).then_some(Kind::Keyword),
        Syntax::Sql => SQL_KEYWORDS.contains(&w).then_some(Kind::Keyword),
    }
}

/// Length of an IPv4 address (optionally `/prefix`) at `i`, if any.
fn ipv4_len(chars: &[char], i: usize) -> Option<usize> {
    let mut j = i;
    for part in 0..4 {
        let start = j;
        let mut value = 0u32;
        while j < chars.len() && chars[j].is_ascii_digit() && j - start < 3 {
            value = value * 10 + chars[j].to_digit(10)?;
            j += 1;
        }
        if j == start || value > 255 {
            return None;
        }
        if part < 3 {
            if chars.get(j) != Some(&'.') {
                return None;
            }
            j += 1;
        }
    }
    if chars
        .get(j)
        .is_some_and(|c| c.is_ascii_digit() || *c == '.')
    {
        return None; // longer number / version string
    }
    if chars.get(j) == Some(&'/') {
        let mut k = j + 1;
        while k < chars.len() && chars[k].is_ascii_digit() && k - j <= 2 {
            k += 1;
        }
        if k > j + 1 {
            j = k;
        }
    }
    Some(j - i)
}

fn comment_starts(syntax: Syntax, chars: &[char], i: usize, line_start: bool) -> bool {
    match syntax {
        Syntax::Shell | Syntax::Perl => chars[i] == '#' && (i == 0 || chars[i - 1].is_whitespace()),
        Syntax::Cisco => chars[i] == '!' && line_start,
        Syntax::Sql => chars[i] == '-' && chars.get(i + 1) == Some(&'-'),
        Syntax::None | Syntax::Standard => false,
    }
}

/// Highlighted spans of one line of characters.
pub fn spans(syntax: Syntax, chars: &[char]) -> Vec<Span> {
    let mut out = Vec::new();
    if syntax == Syntax::None {
        return out;
    }
    let quotes = matches!(syntax, Syntax::Shell | Syntax::Perl | Syntax::Sql);
    let variables = matches!(syntax, Syntax::Shell | Syntax::Perl);
    let addresses = matches!(syntax, Syntax::Standard | Syntax::Cisco);
    let mut i = 0;
    let mut line_start = true;
    while i < chars.len() {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }
        let span = |start, end, kind| Span { start, end, kind };
        if comment_starts(syntax, chars, i, line_start) {
            out.push(span(i, chars.len(), Kind::Comment));
            break;
        }
        line_start = false;
        if quotes && (c == '"' || c == '\'') {
            let end = chars[i + 1..]
                .iter()
                .position(|&q| q == c)
                .map_or(chars.len(), |p| i + 1 + p + 1);
            out.push(span(i, end, Kind::Text));
            i = end;
            continue;
        }
        let sigil = c == '$' || (syntax == Syntax::Perl && (c == '@' || c == '%'));
        if variables && sigil && chars.get(i + 1).is_some_and(|&n| is_word(n) || n == '{') {
            let mut end = i + 1;
            if chars[end] == '{' {
                end = chars[end..]
                    .iter()
                    .position(|&b| b == '}')
                    .map_or(chars.len(), |p| end + p + 1);
            } else {
                while end < chars.len() && is_word(chars[end]) {
                    end += 1;
                }
            }
            out.push(span(i, end, Kind::Variable));
            i = end;
            continue;
        }
        if c.is_ascii_digit() && (i == 0 || !is_word(chars[i - 1])) {
            if addresses {
                if let Some(len) = ipv4_len(chars, i) {
                    out.push(span(i, i + len, Kind::Address));
                    i += len;
                } else {
                    // Not an address (version string, long number): skip
                    // it whole so no address is found inside it.
                    while i < chars.len() && (chars[i].is_ascii_alphanumeric() || chars[i] == '.') {
                        i += 1;
                    }
                }
                continue;
            }
            if syntax == Syntax::Sql {
                let mut end = i;
                while end < chars.len() && (chars[end].is_ascii_digit() || chars[end] == '.') {
                    end += 1;
                }
                if !chars.get(end).is_some_and(|&n| is_word(n)) {
                    out.push(span(i, end, Kind::Number));
                    i = end;
                    continue;
                }
            }
        }
        if is_word(c) {
            let mut end = i;
            // Cisco keywords contain dashes (access-list, err-disabled).
            while end < chars.len()
                && (is_word(chars[end]) || (syntax == Syntax::Cisco && chars[end] == '-'))
            {
                end += 1;
            }
            let word: String = chars[i..end].iter().collect();
            let first = out.is_empty() && chars[..i].iter().all(|c| c.is_whitespace());
            if let Some(kind) = word_kind(syntax, &word, first) {
                out.push(span(i, end, kind));
            }
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(syntax: Syntax, line: &str) -> Vec<(String, Kind)> {
        let chars: Vec<char> = line.chars().collect();
        spans(syntax, &chars)
            .into_iter()
            .map(|s| (chars[s.start..s.end].iter().collect(), s.kind))
            .collect()
    }

    #[test]
    fn standard_keywords_and_addresses() {
        let k = kinds(
            Syntax::Standard,
            "[ OK ] eth0 up; ERROR: link failed, warning at 10.0.0.1/24",
        );
        assert_eq!(
            k,
            vec![
                ("OK".into(), Kind::Ok),
                ("ERROR".into(), Kind::Error),
                ("failed".into(), Kind::Error),
                ("warning".into(), Kind::Warning),
                ("10.0.0.1/24".into(), Kind::Address),
            ]
        );
        assert!(
            kinds(
                Syntax::Standard,
                "version 1.2.3.4.5 and 300.1.1.1 and errorless"
            )
            .is_empty()
        );
        assert!(kinds(Syntax::None, "error").is_empty());
    }

    #[test]
    fn shell_script() {
        let k = kinds(
            Syntax::Shell,
            r#"if [ "$HOME" ]; then echo ${USER} 'x' # done"#,
        );
        assert_eq!(
            k,
            vec![
                ("if".into(), Kind::Keyword),
                ("\"$HOME\"".into(), Kind::Text),
                ("then".into(), Kind::Keyword),
                ("${USER}".into(), Kind::Variable),
                ("'x'".into(), Kind::Text),
                ("# done".into(), Kind::Comment),
            ]
        );
        assert!(kinds(Syntax::Shell, "a#b").is_empty(), "# inside a word");
    }

    #[test]
    fn cisco_perl_sql() {
        let k = kinds(Syntax::Cisco, " no shutdown");
        assert_eq!(
            k,
            vec![("no".into(), Kind::Error), ("shutdown".into(), Kind::Error)]
        );
        let k = kinds(
            Syntax::Cisco,
            "interface Gi0/1 is up, ip address 192.168.1.1",
        );
        assert_eq!(k[0], ("interface".into(), Kind::Keyword));
        assert!(k.contains(&("up".into(), Kind::Ok)));
        assert!(k.contains(&("192.168.1.1".into(), Kind::Address)));
        assert_eq!(
            kinds(Syntax::Cisco, "! comment"),
            vec![("! comment".into(), Kind::Comment)]
        );

        let k = kinds(Syntax::Perl, "my @list = (%h); # c");
        assert_eq!(k[0], ("my".into(), Kind::Keyword));
        assert_eq!(k[1], ("@list".into(), Kind::Variable));
        assert_eq!(k[2], ("%h".into(), Kind::Variable));

        let k = kinds(
            Syntax::Sql,
            "SELECT name FROM t WHERE id = 42 AND s = 'a' -- note",
        );
        assert_eq!(
            k.iter().map(|(_, kind)| *kind).collect::<Vec<_>>(),
            vec![
                Kind::Keyword,
                Kind::Keyword,
                Kind::Keyword,
                Kind::Number,
                Kind::Keyword,
                Kind::Text,
                Kind::Comment
            ]
        );
    }
}
