//! Automatic "Follow terminal folder" for SSH tabs. When the login shell is
//! bash or zsh, the OSC 7 prompt hook is typed into the shell once the first
//! prompt has settled, and its echo is hidden: output is held back until the
//! first OSC 7 report, then the prompt line is redrawn in place.

use std::time::{Duration, Instant};

/// Output must be quiet this long (the prompt is waiting) before typing.
const SETTLE: Duration = Duration::from_millis(400);
/// Give up hiding and show the held output if no report arrives in time.
const HIDE_TIMEOUT: Duration = Duration::from_secs(3);
/// Never hold back more than this.
const MAX_HELD: usize = 64 * 1024;
const OSC7: &[u8] = b"\x1b]7;";

pub enum Action {
    None,
    /// Type the hook into the shell.
    Type,
    /// Show output that was held back.
    Feed(Vec<u8>),
}

enum State {
    Waiting {
        /// `Some(true)` when the login shell is bash or zsh.
        supported: Option<bool>,
        last_output: Option<Instant>,
    },
    Hiding {
        since: Instant,
        held: Vec<u8>,
    },
    Done,
}

pub struct FolderHook {
    state: State,
}

pub fn is_supported_shell(login_shell: &str) -> bool {
    let name = login_shell.rsplit('/').next().unwrap_or("");
    matches!(name, "bash" | "zsh" | "-bash" | "-zsh")
}

impl FolderHook {
    pub fn new() -> Self {
        Self {
            state: State::Waiting {
                supported: None,
                last_output: None,
            },
        }
    }

    pub fn on_login_shell(&mut self, login_shell: &str) {
        if let State::Waiting { supported, .. } = &mut self.state {
            if is_supported_shell(login_shell) {
                *supported = Some(true);
            } else {
                self.state = State::Done;
            }
        }
    }

    /// The user typed something before the hook: leave their line alone.
    pub fn on_user_input(&mut self) {
        if matches!(self.state, State::Waiting { .. }) {
            self.state = State::Done;
        }
    }

    /// Shell output on its way to the terminal: returns what to show now.
    pub fn filter(&mut self, bytes: &[u8], now: Instant) -> Vec<u8> {
        match &mut self.state {
            State::Waiting { last_output, .. } => {
                if contains(bytes, OSC7) {
                    // The shell already reports its folder.
                    self.state = State::Done;
                } else {
                    *last_output = Some(now);
                }
                bytes.to_vec()
            }
            State::Hiding { held, .. } => {
                held.extend_from_slice(bytes);
                if let Some(at) = find(held, OSC7) {
                    // Drop the echoed hook line and redraw the prompt over
                    // the old one (the cursor never left it).
                    let mut out = b"\r\x1b[K".to_vec();
                    out.extend_from_slice(&held[at..]);
                    self.state = State::Done;
                    out
                } else if held.len() > MAX_HELD {
                    let out = std::mem::take(held);
                    self.state = State::Done;
                    out
                } else {
                    Vec::new()
                }
            }
            State::Done => bytes.to_vec(),
        }
    }

    pub fn poll(&mut self, now: Instant) -> Action {
        match &mut self.state {
            State::Waiting {
                supported: Some(true),
                last_output: Some(at),
            } if now.duration_since(*at) >= SETTLE => {
                self.state = State::Hiding {
                    since: now,
                    held: Vec::new(),
                };
                Action::Type
            }
            State::Hiding { since, held } if now.duration_since(*since) >= HIDE_TIMEOUT => {
                let out = std::mem::take(held);
                self.state = State::Done;
                Action::Feed(out)
            }
            _ => Action::None,
        }
    }

    /// When `poll` should run again, if it is waiting on a timer.
    pub fn wakeup(&self, now: Instant) -> Option<Duration> {
        match &self.state {
            State::Waiting {
                supported: Some(true),
                last_output: Some(at),
            } => Some((*at + SETTLE).saturating_duration_since(now)),
            State::Hiding { since, .. } => {
                Some((*since + HIDE_TIMEOUT).saturating_duration_since(now))
            }
            _ => None,
        }
    }

    pub fn is_done(&self) -> bool {
        matches!(self.state, State::Done)
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    find(haystack, needle).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_names() {
        assert!(is_supported_shell("/bin/bash"));
        assert!(is_supported_shell("/usr/bin/zsh"));
        assert!(!is_supported_shell("/usr/bin/fish"));
        assert!(!is_supported_shell("/bin/sh"));
        assert!(!is_supported_shell(""));
    }

    #[test]
    fn types_after_prompt_settles_and_hides_echo() {
        let t0 = Instant::now();
        let mut hook = FolderHook::new();
        assert_eq!(hook.filter(b"motd\r\n$ ", t0), b"motd\r\n$ ");
        assert!(matches!(hook.poll(t0), Action::None), "shell not known yet");
        hook.on_login_shell("/bin/bash");
        assert!(matches!(
            hook.poll(t0 + Duration::from_millis(100)),
            Action::None
        ));
        assert!(matches!(hook.poll(t0 + SETTLE), Action::Type));

        let t1 = t0 + SETTLE;
        assert!(
            hook.filter(b" if [ -n \"$ZSH_VERSION\" ]; then", t1)
                .is_empty()
        );
        assert!(hook.filter(b" ... fi\r\n", t1).is_empty());
        let out = hook.filter(b"\x1b]7;file://h/home/u\x07$ ", t1);
        assert_eq!(out, b"\r\x1b[K\x1b]7;file://h/home/u\x07$ ");
        assert!(hook.is_done());
        assert_eq!(hook.filter(b"ls\r\n", t1), b"ls\r\n");
    }

    #[test]
    fn unsupported_shell_or_typing_cancels() {
        let t0 = Instant::now();
        let mut fish = FolderHook::new();
        fish.on_login_shell("/usr/bin/fish");
        assert!(fish.is_done());

        let mut typed = FolderHook::new();
        typed.filter(b"$ ", t0);
        typed.on_user_input();
        typed.on_login_shell("/bin/bash");
        assert!(matches!(typed.poll(t0 + SETTLE), Action::None));
        assert!(typed.is_done());

        let mut reporting = FolderHook::new();
        reporting.filter(b"\x1b]7;file://h/\x07$ ", t0);
        assert!(reporting.is_done());
    }

    #[test]
    fn held_output_is_shown_on_timeout() {
        let t0 = Instant::now();
        let mut hook = FolderHook::new();
        hook.on_login_shell("zsh");
        hook.filter(b"% ", t0);
        assert!(matches!(hook.poll(t0 + SETTLE), Action::Type));
        assert!(hook.filter(b"echoed", t0 + SETTLE).is_empty());
        match hook.poll(t0 + SETTLE + HIDE_TIMEOUT) {
            Action::Feed(out) => assert_eq!(out, b"echoed"),
            _ => panic!("expected the held output"),
        }
        assert!(hook.is_done());
    }
}
