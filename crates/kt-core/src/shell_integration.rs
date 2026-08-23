//! Shell 集成：让远端交互 shell 主动上报工作目录，并构造写回 PTY 的目录切换命令。
//!
//! SSH 协议无法读取或修改一个已在运行的交互 shell 的工作目录。要让终端方向的
//! 目录变化实时可见，唯一可靠的做法是让 shell 自己在每次 prompt 前发出 OSC 7；
//! 而 Debian/Ubuntu/RHEL 的默认 bash 与 Linux 上的 zsh 都不会这么做。因此这里
//! 提供一次性注入的 bootstrap 命令，注入后 `cd`、`pushd`、脚本内切目录、子 shell
//! 退出都会自然上报，跟随过程本身不再需要发送任何命令。

use std::time::{Duration, Instant};

/// bootstrap 执行完成时写入 PTY 的不可见 OSC 标记。
pub const BOOTSTRAP_DONE_MARKER: &[u8] = b"\x1b]1337;KitonyTermsShellIntegration=done\x07";
/// 最长只等待这么久；标记缺失时恢复原样输出，不能永久吞掉远端信息。
const FILTER_HARD_LIMIT: Duration = Duration::from_secs(3);
/// 异常 shell 持续输出时的内存上限；超过后立即结束过滤并显示已缓存内容。
const FILTER_MAX_PENDING: usize = 64 * 1024;
/// 去掉旧 prompt 所在行，随后让 shell 正常输出的新 prompt 接管当前行。
const CLEAR_CURRENT_LINE: &[u8] = b"\r\x1b[2K";

/// 一次性注入的 shell 集成命令。
///
/// - `__kt_cwd` 用 `printf '%s'` 传参而不是把 `$PWD` 拼进格式串，路径里的 `%`
///   不会被当成格式说明符。
/// - bash 侧前置追加到 `PROMPT_COMMAND`，并识别 bash 5.1+ 的数组形式，避免把
///   用户的数组配置压成标量。
/// - zsh 侧追加 `precmd_functions`；`+=(...)` 与 `setopt` 放进 `eval`，这样
///   dash 之类不认该语法的 shell 只是运行期跳过，而不是整行解析失败。
/// - 追加 `ignorespace` 让以空格开头的命令不进历史，配合
///   [`change_directory_command`] 的前导空格，使文件管理发出的 `cd` 不污染
///   用户的 shell history。
pub const BOOTSTRAP_COMMAND: &str = concat!(
    r#"__kt_cwd(){ printf '\033]7;file://%s\a' "$PWD"; }; "#,
    r#"if [ -n "$ZSH_VERSION" ]; then "#,
    r#"eval 'precmd_functions+=(__kt_cwd); setopt hist_ignore_space'; "#,
    r#"elif [ -n "$BASH_VERSION" ]; then "#,
    r#"case "$(declare -p PROMPT_COMMAND 2>/dev/null)" in "#,
    r#""declare -a"*) eval 'PROMPT_COMMAND+=(__kt_cwd)';; "#,
    r#"*) PROMPT_COMMAND="__kt_cwd${PROMPT_COMMAND:+;$PROMPT_COMMAND}";; "#,
    r#"esac; "#,
    r#"HISTCONTROL="${HISTCONTROL:+$HISTCONTROL:}ignorespace"; "#,
    r#"fi; __kt_cwd; printf '\033]1337;KitonyTermsShellIntegration=done\a'"#,
    "\n"
);

/// 构造写回 PTY 的目录切换命令；`path` 为空时返回 `None`。
///
/// 组成部分依次是：
/// 1. `\x15`（Ctrl+U）清空用户可能正输入到一半的命令行，避免 `cd` 被拼接到
///    半行输入后面变成 `git commcd -- '/var/log'`。
/// 2. 前导空格，配合 bootstrap 追加的 `ignorespace` 让该命令不进 shell history。
/// 3. `printf '\033[A\r\033[J'` 上移一行并清除到屏幕末尾，擦掉 shell 对本行的
///    回显。它排在 `cd` **之前**，所以 `cd` 失败时的报错落在被清除的位置上，
///    仍然对用户可见。
/// 4. 单引号转义后的 `cd --`，路径中的单引号按 `'\''` 收敛。
pub fn change_directory_command(path: &str) -> Option<String> {
    let path = path.trim();
    if path.is_empty() {
        return None;
    }
    // `~` 与 `.` 交给 shell 自己解释成 home，不做本地展开。
    if path == "~" || path == "." {
        return Some("\x15 printf '\\033[A\\r\\033[J'; cd\n".to_string());
    }
    let escaped = path.replace('\'', r"'\''");
    Some(format!(
        "\x15 printf '\\033[A\\r\\033[J'; cd -- '{escaped}'\n"
    ))
}

/// 只过滤 bootstrap 自身回显与执行过程，保留在它之前到达的 MOTD、Last login 等
/// 登录输出。远端 shell 只有进入可读命令的 prompt 后才会回显并执行 bootstrap，
/// 因此完成标记为过滤边界，比按时间吞掉整个 PTY 数据流可靠。
#[derive(Default)]
pub struct BootstrapOutputFilter {
    pending: Vec<u8>,
    hard_deadline: Option<Instant>,
    echo_seen: bool,
}

impl BootstrapOutputFilter {
    /// 开始等待本次 bootstrap 的命令回显与完成标记。
    pub fn start(&mut self, now: Instant) {
        self.pending.clear();
        self.hard_deadline = Some(now + FILTER_HARD_LIMIT);
        self.echo_seen = false;
    }

    /// 立即结束过滤并返回尚未交给终端的普通输出。
    pub fn finish(&mut self) -> Vec<u8> {
        let pending = std::mem::take(&mut self.pending);
        self.hard_deadline = None;
        self.echo_seen = false;
        pending
    }

    pub fn is_active(&self) -> bool {
        self.hard_deadline.is_some()
    }

    /// 过滤一批 stdout 数据，返回应该交给终端引擎的字节。
    pub fn filter(&mut self, data: &[u8], now: Instant) -> Vec<u8> {
        let Some(hard_deadline) = self.hard_deadline else {
            return data.to_vec();
        };
        if now >= hard_deadline {
            let mut visible = self.finish();
            visible.extend_from_slice(data);
            return visible;
        }

        self.pending.extend_from_slice(data);
        if self.echo_seen {
            return self.finish_after_marker_or_limit();
        }

        let echo = BOOTSTRAP_COMMAND.trim_end_matches('\n').as_bytes();
        if let Some((echo_start, echo_end)) = find_echo(&self.pending, echo) {
            let mut visible = self.pending[..echo_start].to_vec();
            visible.extend_from_slice(CLEAR_CURRENT_LINE);
            self.pending.drain(..echo_end);
            self.echo_seen = true;
            visible.extend(self.finish_after_marker_or_limit());
            return visible;
        }

        // ECHO 被远端关闭时不会出现命令文本，但完成标记仍能给出精确边界。
        if let Some(marker_start) = find_subslice(&self.pending, BOOTSTRAP_DONE_MARKER) {
            let mut visible = self.pending[..marker_start].to_vec();
            visible.extend_from_slice(CLEAR_CURRENT_LINE);
            visible.extend_from_slice(&self.pending[marker_start + BOOTSTRAP_DONE_MARKER.len()..]);
            self.finish();
            return visible;
        }

        if self.pending.len() > FILTER_MAX_PENDING {
            return self.finish();
        }

        // 只保留可能与下一数据块拼成“命令回显/完成标记”的末尾，其他登录输出
        // 立即显示，避免为了等待注入完成而让 MOTD 延迟数秒。
        let keep = suffix_prefix_len(&self.pending, echo, true).max(suffix_prefix_len(
            &self.pending,
            BOOTSTRAP_DONE_MARKER,
            false,
        ));
        let visible_len = self.pending.len() - keep;
        self.pending.drain(..visible_len).collect()
    }

    fn finish_after_marker_or_limit(&mut self) -> Vec<u8> {
        if let Some(marker_start) = find_subslice(&self.pending, BOOTSTRAP_DONE_MARKER) {
            let visible = self.pending[marker_start + BOOTSTRAP_DONE_MARKER.len()..].to_vec();
            self.finish();
            return visible;
        }
        if self.pending.len() > FILTER_MAX_PENDING {
            return self.finish();
        }
        Vec::new()
    }
}

fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    (!needle.is_empty())
        .then(|| {
            haystack
                .windows(needle.len())
                .position(|window| window == needle)
        })
        .flatten()
}

/// 查找 PTY 行回显中的 bootstrap 命令。终端达到右边界时可能在回显中插入
/// `CR/LF`，这些换行不是命令本身的一部分，应在匹配时忽略。
fn find_echo(haystack: &[u8], needle: &[u8]) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    for start in 0..haystack.len() {
        if haystack[start] != needle[0] {
            continue;
        }
        let mut cursor = start;
        let mut matched = 0;
        while cursor < haystack.len() && matched < needle.len() {
            match haystack[cursor] {
                b'\r' | b'\n' => cursor += 1,
                byte if byte == needle[matched] => {
                    cursor += 1;
                    matched += 1;
                }
                _ => break,
            }
        }
        if matched == needle.len() {
            return Some((start, cursor));
        }
    }
    None
}

fn suffix_prefix_len(data: &[u8], needle: &[u8], ignore_line_breaks: bool) -> usize {
    for start in 0..data.len() {
        let mut matched = 0;
        let mut valid = true;
        for &byte in &data[start..] {
            if ignore_line_breaks && matches!(byte, b'\r' | b'\n') {
                continue;
            }
            if needle.get(matched).copied() != Some(byte) {
                valid = false;
                break;
            }
            matched += 1;
        }
        if valid && matched > 0 && matched < needle.len() {
            return data.len() - start;
        }
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_reports_cwd_as_printf_argument() {
        // `$PWD` 必须作为参数传入，否则路径里的 `%` 会被当成格式说明符。
        assert!(BOOTSTRAP_COMMAND.contains(r#"printf '\033]7;file://%s\a' "$PWD""#));
        assert!(
            BOOTSTRAP_COMMAND.contains(r#"printf '\033]1337;KitonyTermsShellIntegration=done\a'"#)
        );
        assert!(BOOTSTRAP_COMMAND.ends_with('\n'));
    }

    #[test]
    fn bootstrap_preserves_existing_shell_hooks() {
        // bash 标量前置追加、bash 数组追加、zsh precmd 追加都不能覆盖用户配置。
        assert!(BOOTSTRAP_COMMAND
            .contains(r#"PROMPT_COMMAND="__kt_cwd${PROMPT_COMMAND:+;$PROMPT_COMMAND}""#));
        assert!(BOOTSTRAP_COMMAND.contains("PROMPT_COMMAND+=(__kt_cwd)"));
        assert!(BOOTSTRAP_COMMAND.contains("precmd_functions+=(__kt_cwd)"));
        assert!(
            BOOTSTRAP_COMMAND.contains(r#"HISTCONTROL="${HISTCONTROL:+$HISTCONTROL:}ignorespace""#)
        );
    }

    #[test]
    fn bootstrap_guards_zsh_only_syntax_behind_eval() {
        // dash 等 shell 会解析整行，`+=(...)` 必须留在 eval 里才不会整行语法失败。
        assert!(BOOTSTRAP_COMMAND
            .contains("eval 'precmd_functions+=(__kt_cwd); setopt hist_ignore_space'"));
        assert!(BOOTSTRAP_COMMAND.contains("eval 'PROMPT_COMMAND+=(__kt_cwd)'"));
    }

    #[test]
    fn change_directory_clears_input_line_and_erases_echo_before_cd() {
        let command = change_directory_command("/var/log").expect("非空路径应生成命令");
        assert_eq!(
            command,
            "\x15 printf '\\033[A\\r\\033[J'; cd -- '/var/log'\n"
        );
        // 擦除必须排在 cd 之前，cd 失败的报错才不会被一起擦掉。
        let erase = command.find("printf").expect("含擦除序列");
        let cd = command.find("cd --").expect("含 cd");
        assert!(erase < cd);
    }

    #[test]
    fn change_directory_escapes_single_quotes() {
        let command = change_directory_command("/tmp/it's here").expect("非空路径应生成命令");
        assert!(command.contains(r"cd -- '/tmp/it'\''s here'"));
    }

    #[test]
    fn change_directory_leaves_home_shorthand_to_the_shell() {
        for path in ["~", "."] {
            let command = change_directory_command(path).expect("home 简写应生成命令");
            assert!(command.ends_with("; cd\n"), "实际: {command:?}");
        }
    }

    #[test]
    fn change_directory_rejects_blank_path() {
        assert!(change_directory_command("").is_none());
        assert!(change_directory_command("   ").is_none());
    }

    #[test]
    fn bootstrap_filter_preserves_login_output_and_hides_chunked_echo() {
        let start = Instant::now();
        let mut filter = BootstrapOutputFilter::default();
        filter.start(start);

        let mut visible = filter.filter(
            b"Welcome to Ubuntu\r\nLast login: today\r\nuser@host:~$ ",
            start + Duration::from_millis(10),
        );
        let echo = BOOTSTRAP_COMMAND.trim_end_matches('\n').as_bytes();
        let echo_split = echo.len() / 2;
        visible.extend(filter.filter(&echo[..echo_split], start + Duration::from_millis(20)));

        let marker_split = BOOTSTRAP_DONE_MARKER.len() / 2;
        let mut execution = echo[echo_split..].to_vec();
        execution.extend_from_slice(b"\r\n\x1b]7;file:///home/demo\x07");
        execution.extend_from_slice(&BOOTSTRAP_DONE_MARKER[..marker_split]);
        visible.extend(filter.filter(&execution, start + Duration::from_millis(30)));

        let mut completion = BOOTSTRAP_DONE_MARKER[marker_split..].to_vec();
        completion.extend_from_slice(b"user@host:~$ ");
        visible.extend(filter.filter(&completion, start + Duration::from_millis(40)));

        let mut term = crate::term::TermEngine::new(120, 8, 20);
        term.advance(&visible);
        let text = term.snapshot().to_plain_text();
        assert!(text.contains("Welcome to Ubuntu"), "实际: {text:?}");
        assert!(text.contains("Last login: today"), "实际: {text:?}");
        assert!(text.contains("user@host:~$"), "实际: {text:?}");
        assert!(!text.contains("__kt_cwd"), "注入命令不得可见: {text:?}");
        assert!(!filter.is_active());
    }

    #[test]
    fn bootstrap_filter_uses_done_marker_when_remote_echo_is_disabled() {
        let start = Instant::now();
        let mut filter = BootstrapOutputFilter::default();
        filter.start(start);

        let mut data = b"System maintenance tonight\r\nquiet$ \x1b]7;file:///srv\x07".to_vec();
        data.extend_from_slice(BOOTSTRAP_DONE_MARKER);
        data.extend_from_slice(b"quiet$ ");
        let visible = filter.filter(&data, start + Duration::from_millis(20));

        let mut term = crate::term::TermEngine::new(100, 6, 20);
        term.advance(&visible);
        let text = term.snapshot().to_plain_text();
        assert!(
            text.contains("System maintenance tonight"),
            "实际: {text:?}"
        );
        assert!(text.contains("quiet$"), "实际: {text:?}");
        assert!(!filter.is_active());
    }

    #[test]
    fn bootstrap_filter_matches_terminal_wrapped_echo() {
        let start = Instant::now();
        let mut filter = BootstrapOutputFilter::default();
        filter.start(start);

        let echo = BOOTSTRAP_COMMAND.trim_end_matches('\n').as_bytes();
        let mut wrapped = b"Wrapped host notice\r\nwrap$ ".to_vec();
        for chunk in echo.chunks(37) {
            wrapped.extend_from_slice(chunk);
            wrapped.extend_from_slice(b"\r\n");
        }
        wrapped.extend_from_slice(b"\x1b]7;file:///tmp\x07");
        wrapped.extend_from_slice(BOOTSTRAP_DONE_MARKER);
        wrapped.extend_from_slice(b"wrap$ ");

        let visible = filter.filter(&wrapped, start + Duration::from_millis(20));
        let mut term = crate::term::TermEngine::new(100, 6, 20);
        term.advance(&visible);
        let text = term.snapshot().to_plain_text();
        assert!(text.contains("Wrapped host notice"), "实际: {text:?}");
        assert!(text.contains("wrap$"), "实际: {text:?}");
        assert!(!text.contains("__kt_cwd"), "换行回显不得可见: {text:?}");
        assert!(!filter.is_active());
    }

    #[test]
    fn bootstrap_filter_timeout_flushes_unconfirmed_output() {
        let start = Instant::now();
        let mut filter = BootstrapOutputFilter::default();
        filter.start(start);

        let mut visible = filter.filter(b"notice __kt", start + Duration::from_millis(10));
        visible.extend(filter.filter(b" still visible", start + FILTER_HARD_LIMIT));

        assert_eq!(visible, b"notice __kt still visible");
        assert!(!filter.is_active());
    }

    #[test]
    fn bootstrap_filter_finish_flushes_partial_normal_output() {
        let start = Instant::now();
        let mut filter = BootstrapOutputFilter::default();
        filter.start(start);

        let echo = BOOTSTRAP_COMMAND.trim_end_matches('\n').as_bytes();
        let split = echo.len() / 2;
        let visible = filter.filter(&echo[..split], start + Duration::from_millis(10));
        assert!(visible.is_empty());
        let mut visible = visible;
        visible.extend(filter.finish());

        assert_eq!(visible, echo[..split]);
        assert!(!filter.is_active());
    }

    /// 注入命令由远端 shell 整行解析，任何一处语法错误都会让整条命令失效，而且
    /// 错误输出可能出现在过滤边界内、运行时不便诊断。所以这里直接用本机 shell
    /// 做语法检查（`-n` 只解析不执行，不会改动本机环境）。
    #[cfg(unix)]
    #[test]
    fn bootstrap_command_parses_in_every_common_login_shell() {
        let mut checked = Vec::new();
        for shell in ["sh", "bash", "zsh", "dash", "ksh"] {
            let Ok(output) = std::process::Command::new(shell)
                .args(["-n", "-c", BOOTSTRAP_COMMAND])
                .output()
            else {
                continue; // 本机没装这个 shell
            };
            assert!(
                output.status.success(),
                "{shell} 无法解析注入命令: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            checked.push(shell);
        }
        assert!(!checked.is_empty(), "至少要有一个可用 shell 参与语法检查");
    }

    /// 覆盖用户的 `PROMPT_COMMAND` 会连带废掉他的 history 追加、窗口标题等配置，
    /// 因此前置追加行为必须真的在 bash 里跑通，而不是只比对字符串。
    #[cfg(unix)]
    #[test]
    fn bootstrap_appends_to_bash_hooks_without_dropping_user_config() {
        let script = format!(
            "PROMPT_COMMAND='history -a'; HISTCONTROL=ignoredups\n\
             {BOOTSTRAP_COMMAND}\
             printf 'PC=%s|HC=%s|' \"$PROMPT_COMMAND\" \"$HISTCONTROL\""
        );
        let Ok(output) = std::process::Command::new("bash")
            .args(["-c", &script])
            .output()
        else {
            return; // 本机没有 bash
        };
        assert!(
            output.status.success(),
            "bash 执行注入命令失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("PC=__kt_cwd;history -a|"),
            "用户原有的 PROMPT_COMMAND 必须保留在后面，实际: {stdout:?}"
        );
        assert!(
            stdout.contains("HC=ignoredups:ignorespace|"),
            "HISTCONTROL 必须是追加而不是覆盖，实际: {stdout:?}"
        );
        assert!(
            stdout.contains("\x1b]7;file:///"),
            "注入后应立即上报一次 OSC 7，实际: {stdout:?}"
        );
    }

    /// zsh 没有 `PROMPT_COMMAND`，走的是 `precmd_functions`；同样不能丢掉用户已有的 hook。
    #[cfg(unix)]
    #[test]
    fn bootstrap_appends_to_zsh_precmd_without_dropping_user_hooks() {
        let script = format!(
            "precmd_functions=(user_hook)\n\
             {BOOTSTRAP_COMMAND}\
             printf 'PF=%s|PC=%s|' \"$precmd_functions\" \"${{PROMPT_COMMAND:-unset}}\""
        );
        let Ok(output) = std::process::Command::new("zsh")
            .args(["-c", &script])
            .output()
        else {
            return; // 本机没有 zsh
        };
        assert!(
            output.status.success(),
            "zsh 执行注入命令失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("PF=user_hook __kt_cwd|"),
            "用户原有的 precmd hook 必须保留，实际: {stdout:?}"
        );
        assert!(
            stdout.contains("PC=unset|"),
            "zsh 下不得误设 bash 专用的 PROMPT_COMMAND，实际: {stdout:?}"
        );
    }

    /// `cd` 命令的擦除序列依赖 shell 内建 `printf` 解释八进制转义；若某个 shell
    /// 原样输出 `\033`，终端里会留下可见垃圾字符而不是擦掉回显。
    #[cfg(unix)]
    #[test]
    fn change_directory_erase_sequence_is_interpreted_by_shell_printf() {
        let command = change_directory_command("/var/log").expect("非空路径应生成命令");
        // 取出 `printf '...'` 这一段单独执行，验证它产出真正的 ANSI 序列。
        let printf_part = command
            .trim_start_matches('\x15')
            .trim_start()
            .split("; cd")
            .next()
            .expect("命令含 printf 段")
            .to_string();

        for shell in ["sh", "bash", "zsh", "dash"] {
            let Ok(output) = std::process::Command::new(shell)
                .args(["-c", &printf_part])
                .output()
            else {
                continue;
            };
            assert_eq!(
                output.stdout, b"\x1b[A\r\x1b[J",
                "{shell} 的 printf 未解释擦除序列"
            );
        }
    }
}
