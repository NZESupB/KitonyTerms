//! 只在 shell 集成确认的提示符后观察一行输入，不采集运行中程序的输入。

pub const PROMPT_MARKER: &[u8] = b"\x1b]1337;KitonyTermsPrompt=ready\x07";
const MAX_COMMAND_BYTES: usize = 4096;

#[derive(Default)]
pub(crate) struct CommandTracker {
    marker_offset: usize,
    ready: bool,
    invalid: bool,
    line: Vec<u8>,
}

impl CommandTracker {
    pub fn output(&mut self, data: &[u8]) {
        for &byte in data {
            if byte == PROMPT_MARKER[self.marker_offset] {
                self.marker_offset += 1;
                if self.marker_offset == PROMPT_MARKER.len() {
                    self.marker_offset = 0;
                    self.line.clear();
                    self.invalid = false;
                    self.ready = true;
                }
            } else {
                self.marker_offset = usize::from(byte == PROMPT_MARKER[0]);
            }
        }
    }

    pub fn input(&mut self, data: &[u8], alt_screen: bool) -> Option<String> {
        if alt_screen {
            self.ready = false;
            self.line.clear();
        }
        if !self.ready {
            return None;
        }
        for &byte in data {
            match byte {
                b'\r' | b'\n' => {
                    self.ready = false;
                    let line = std::mem::take(&mut self.line);
                    // 前导空白遵循 shell 的 ignorespace 习惯，也排除文件管理注入的 cd。
                    return (!self.invalid && !line.first().is_some_and(u8::is_ascii_whitespace))
                        .then(|| String::from_utf8(line).ok())
                        .flatten()
                        .filter(|line| !line.trim().is_empty());
                }
                0x7f | 0x08 if !self.invalid => {
                    self.line.pop();
                    // UTF-8 退格需移除整个码点。
                    while std::str::from_utf8(&self.line).is_err() {
                        self.line.pop();
                    }
                }
                0x15 => {
                    self.line.clear();
                }
                byte if byte >= 0x20
                    && byte != 0x7f
                    && !self.invalid
                    && self.line.len() < MAX_COMMAND_BYTES =>
                {
                    self.line.push(byte);
                }
                _ => {
                    self.line.clear();
                    self.invalid = true;
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> CommandTracker {
        let mut tracker = CommandTracker::default();
        for byte in PROMPT_MARKER {
            tracker.output(&[*byte]);
        }
        tracker
    }

    #[test]
    fn only_one_command_after_each_chunked_prompt() {
        let mut tracker = CommandTracker::default();
        assert_eq!(tracker.input(b"login-secret\r", false), None);
        for byte in PROMPT_MARKER {
            tracker.output(&[*byte]);
        }
        assert_eq!(
            tracker.input(b"sudo whoami\r", false).as_deref(),
            Some("sudo whoami")
        );
        assert_eq!(tracker.input(b"sudo-secret\r", false), None);
        tracker.output(b"Password: ");
        assert_eq!(tracker.input(b"other-secret\r", false), None);
        tracker.output(PROMPT_MARKER);
        assert_eq!(tracker.input(b"pwd\r", false).as_deref(), Some("pwd"));
    }

    #[test]
    fn skips_uncertain_edits_private_lines_and_extra_paste_lines() {
        for input in [
            b" hidden\r".as_slice(),
            b"echo\tvalue\r",
            b"pwd\x1b[A\r",
            b"stop\x03\r",
            b"pwd\x1b[D\x15other\r",
        ] {
            assert_eq!(ready().input(input, false), None);
        }
        let mut tracker = ready();
        assert_eq!(
            tracker
                .input(b"read -s secret\rpassword\r", false)
                .as_deref(),
            Some("read -s secret")
        );
        assert_eq!(tracker.input(b"password\r", false), None);
        assert_eq!(ready().input(b"vim-text\r", true), None);
        let mut tracker = ready();
        tracker.input(b"ls", true);
        assert_eq!(tracker.input(b"secret\r", false), None);
    }

    #[test]
    fn unicode_backspace_clear_and_limits() {
        let mut tracker = ready();
        tracker.input("echo 中文".as_bytes(), false);
        assert_eq!(tracker.input(b"\x7f\r", false).as_deref(), Some("echo 中"));
        let mut tracker = ready();
        tracker.input(b"discard\x15", false);
        assert_eq!(tracker.input(b"pwd\r", false).as_deref(), Some("pwd"));
        let mut tracker = ready();
        tracker.input(&vec![b'x'; MAX_COMMAND_BYTES + 1], false);
        assert_eq!(tracker.input(b"\r", false), None);
    }
}
