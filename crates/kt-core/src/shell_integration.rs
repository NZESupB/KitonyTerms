//! Shell 集成：让远端交互 shell 主动上报工作目录，并构造写回 PTY 的目录切换命令。
//!
//! SSH 协议无法读取或修改一个已在运行的交互 shell 的工作目录。要让终端方向的
//! 目录变化实时可见，唯一可靠的做法是让 shell 自己在每次 prompt 前发出 OSC 7；
//! 而 Debian/Ubuntu/RHEL 的默认 bash 与 Linux 上的 zsh 都不会这么做。因此这里
//! 提供一次性注入的 bootstrap 命令，注入后 `cd`、`pushd`、脚本内切目录、子 shell
//! 退出都会自然上报，跟随过程本身不再需要发送任何命令。

use std::time::{Duration, Instant};

/// 注入后等待远端首次回包的时间。要覆盖一次网络往返与 shell 执行。
const QUIET_INITIAL: Duration = Duration::from_millis(1200);
/// 每收到一批被吞掉的数据后延长的空闲时间；连续这么久没有新数据即认为回显结束。
const QUIET_IDLE: Duration = Duration::from_millis(300);
/// 静默窗口的硬上限，防止 shell 持续输出时终端长时间空白。
const QUIET_HARD_LIMIT: Duration = Duration::from_secs(3);

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
    r#"fi; __kt_cwd"#,
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

/// 注入期间吞掉远端回包的窗口，使命令回显与随之重绘的 prompt 不进入终端快照。
///
/// 收敛条件是「连续 [`QUIET_IDLE`] 没有新数据」或触达 [`QUIET_HARD_LIMIT`]，
/// 判定发生在数据到达时，因此不需要额外的定时器分支；没有数据也就没有显示。
#[derive(Default)]
pub struct QuietWindow {
    idle_until: Option<Instant>,
    hard_deadline: Option<Instant>,
}

impl QuietWindow {
    /// 开启静默窗口。
    pub fn start(&mut self, now: Instant) {
        self.idle_until = Some(now + QUIET_INITIAL);
        self.hard_deadline = Some(now + QUIET_HARD_LIMIT);
    }

    /// 立即结束静默窗口，用于用户按键等必须马上恢复回显的场合。
    pub fn end(&mut self) {
        self.idle_until = None;
        self.hard_deadline = None;
    }

    pub fn is_active(&self) -> bool {
        self.idle_until.is_some()
    }

    /// 判断这批远端数据是否应被吞掉；返回 `true` 表示不要喂给终端引擎。
    pub fn absorb(&mut self, now: Instant) -> bool {
        let (Some(idle_until), Some(hard_deadline)) = (self.idle_until, self.hard_deadline) else {
            return false;
        };
        if now >= idle_until || now >= hard_deadline {
            self.end();
            return false;
        }
        self.idle_until = Some((now + QUIET_IDLE).min(hard_deadline));
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bootstrap_reports_cwd_as_printf_argument() {
        // `$PWD` 必须作为参数传入，否则路径里的 `%` 会被当成格式说明符。
        assert!(BOOTSTRAP_COMMAND.contains(r#"printf '\033]7;file://%s\a' "$PWD""#));
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
    fn quiet_window_absorbs_until_output_goes_idle() {
        let start = Instant::now();
        let mut window = QuietWindow::default();
        assert!(!window.absorb(start), "未开启时不得吞数据");

        window.start(start);
        // 初始窗口内的回显被吞掉，并把空闲期限向后推。
        assert!(window.absorb(start + Duration::from_millis(400)));
        assert!(window.absorb(start + Duration::from_millis(600)));
        // 连续 QUIET_IDLE 无数据后，下一批数据正常显示。
        assert!(!window.absorb(start + Duration::from_millis(1000)));
        assert!(!window.is_active());
    }

    #[test]
    fn quiet_window_stops_at_hard_limit_even_with_continuous_output() {
        let start = Instant::now();
        let mut window = QuietWindow::default();
        window.start(start);

        let mut now = start;
        // 持续输出时空闲期限被不断延长，但不能越过硬上限。
        for _ in 0..40 {
            now += Duration::from_millis(100);
            if !window.absorb(now) {
                break;
            }
        }
        assert!(!window.is_active(), "硬上限必须结束静默窗口");
        assert!(now <= start + QUIET_HARD_LIMIT + Duration::from_millis(100));
    }

    #[test]
    fn quiet_window_ends_immediately_on_demand() {
        let start = Instant::now();
        let mut window = QuietWindow::default();
        window.start(start);
        window.end();
        assert!(!window.is_active());
        assert!(!window.absorb(start + Duration::from_millis(10)));
    }

    /// 注入命令由远端 shell 整行解析，任何一处语法错误都会让整条命令失效，而且
    /// 错误输出会被静默窗口吞掉、运行时几乎无从发现。所以这里直接用本机 shell
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
