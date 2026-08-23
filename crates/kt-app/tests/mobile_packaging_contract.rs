use std::fs;
use std::path::{Path, PathBuf};

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::process::Command;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use std::os::unix::fs::PermissionsExt;

#[cfg(any(target_os = "linux", target_os = "macos"))]
use tempfile::TempDir;

const MOBILE_BUNDLE_ID: &str = "com.kitonyterms.app";

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("kt-app 应位于 workspace/crates/kt-app")
        .to_path_buf()
}

fn read_workspace_file(path: &str) -> String {
    let path = workspace_root().join(path);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("读取 {} 失败: {error}", path.display()))
}

fn workflow_job<'a>(workflow: &'a str, job_name: &str) -> &'a str {
    let marker = format!("\n  {job_name}:\n");
    let start = workflow
        .find(&marker)
        .unwrap_or_else(|| panic!("workflow 缺少 job: {job_name}"))
        + 1;
    let rest = &workflow[start..];
    for (offset, _) in rest.match_indices('\n').skip(1) {
        let next_line = &rest[offset + 1..];
        if next_line.starts_with("  ") && !next_line.starts_with("   ") {
            return &rest[..offset];
        }
    }
    rest
}

#[test]
fn mobile_bundle_identifier_is_fixed_in_config_and_workflows() {
    let dioxus = read_workspace_file("Dioxus.toml");
    assert!(dioxus.contains(&format!("identifier = \"{MOBILE_BUNDLE_ID}\"")));
    assert!(dioxus.contains("icon = [\"assets/app-icon.png\"]"));
    assert!(
        workspace_root()
            .join("crates/kt-app/assets/app-icon.png")
            .is_file(),
        "Dioxus bundle 图标必须存在于 kt-app crate 的 assets 目录"
    );

    for workflow in [
        ".github/workflows/alpha.yml",
        ".github/workflows/release.yml",
    ] {
        let workflow = read_workspace_file(workflow);
        assert!(workflow.contains(&format!("MOBILE_BUNDLE_ID: {MOBILE_BUNDLE_ID}")));
    }
}

#[test]
fn alpha_and_release_share_mobile_packaging_contract() {
    for workflow in [
        ".github/workflows/alpha.yml",
        ".github/workflows/release.yml",
    ] {
        let workflow = read_workspace_file(workflow);
        for required in [
            "android-aarch64",
            "ios-aarch64",
            ".github/scripts/package-android-apk.sh",
            ".github/scripts/package-ios-ipa.sh",
            ".github/scripts/allocate-mobile-build-number.sh",
            "ANDROID_CERT_SHA256",
            "mobile_android:",
            "mobile_ios:",
            "environment: mobile-signing",
            "needs.allocate_mobile_build_number.outputs.build_number",
        ] {
            assert!(
                workflow.contains(required),
                "移动端 workflow 缺少契约: {required}"
            );
        }
    }
}

#[test]
fn rust_and_dioxus_follow_latest_stable_channels() {
    let toolchain = read_workspace_file("rust-toolchain.toml");
    assert!(toolchain.contains("channel = \"stable\""));
    assert!(toolchain.contains("profile = \"minimal\""));

    let workspace = read_workspace_file("Cargo.toml");
    assert!(!workspace.contains("rust-version"));
    assert!(workspace.contains("dioxus = { version = \">=0\""));

    for crate_manifest in [
        "crates/kt-app/Cargo.toml",
        "crates/kt-config/Cargo.toml",
        "crates/kt-core/Cargo.toml",
        "crates/kt-secrets/Cargo.toml",
        "crates/kt-sync/Cargo.toml",
        "crates/kt-ui/Cargo.toml",
    ] {
        assert!(!read_workspace_file(crate_manifest).contains("rust-version"));
    }

    for workflow_path in [
        ".github/workflows/alpha.yml",
        ".github/workflows/release.yml",
    ] {
        let workflow = read_workspace_file(workflow_path);
        assert!(workflow.contains("dtolnay/rust-toolchain@stable"));
        assert!(!workflow.contains("DIOXUS_CLI_VERSION"));
        assert!(workflow.contains("cargo install dioxus-cli --locked"));
        assert!(!workflow.contains("cargo install dioxus-cli --locked --version"));
        for line in workflow.lines() {
            if line.contains("dtolnay/rust-toolchain@") {
                assert!(
                    line.contains("dtolnay/rust-toolchain@stable"),
                    "Rust toolchain action 必须跟随 stable: {line}"
                );
            }
        }
    }

    let android = read_workspace_file(".github/scripts/package-android-apk.sh");
    assert!(android.contains("dx --version"));
    assert!(android.contains("prepare-rust-objcopy.sh"));
    assert!(android.contains("configure_rust_objcopy_runtime"));
    assert!(
        android.find("configure_rust_objcopy_runtime") < android.find("dx --version"),
        "Android 必须先准备 rust-objcopy runtime 再运行 dx"
    );
    assert!(!android.contains("DIOXUS_CLI_VERSION"));

    let ios = read_workspace_file(".github/scripts/package-ios-ipa.sh");
    assert!(ios.contains("dx --version"));
    assert!(ios.contains("prepare-rust-objcopy.sh"));
    assert!(ios.contains("configure_rust_objcopy_runtime"));
    assert!(
        ios.find("configure_rust_objcopy_runtime") < ios.find("dx --version"),
        "iOS 必须先准备 rust-objcopy runtime 再运行 dx"
    );
    assert!(!ios.contains("REQUIRED_DX_VERSION"));
}

#[test]
fn mobile_objcopy_runtime_contract_is_cross_platform_and_fail_closed() {
    let helper = read_workspace_file(".github/scripts/prepare-rust-objcopy.sh");
    for required in [
        "rustc --print sysroot",
        "rustc -vV",
        "rust-objcopy",
        "llvm-tools-preview",
        "DYLD_LIBRARY_PATH",
        "LD_LIBRARY_PATH",
        "rust-objcopy 无法加载 LLVM 动态库",
        "uname -s",
    ] {
        assert!(
            helper.contains(required),
            "objcopy helper 缺少契约: {required}"
        );
    }
    assert!(helper.contains("lib/rustlib/$host/bin/rust-objcopy"));
    assert!(helper.contains("lib/rustlib/$host/lib"));
    assert!(helper.contains("return 1"));

    for workflow_path in [
        ".github/workflows/alpha.yml",
        ".github/workflows/release.yml",
    ] {
        let workflow = read_workspace_file(workflow_path);
        for job in ["mobile_android", "mobile_ios"] {
            let job_text = workflow_job(&workflow, job);
            assert!(
                job_text.contains("components: llvm-tools-preview"),
                "{workflow_path} 的 {job} 未安装 llvm-tools-preview"
            );
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn write_executable(path: &Path, contents: &str) {
    fs::write(path, contents)
        .unwrap_or_else(|error| panic!("写入 {} 失败: {error}", path.display()));
    let mut permissions = fs::metadata(path)
        .unwrap_or_else(|error| panic!("读取 {} 权限失败: {error}", path.display()))
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .unwrap_or_else(|error| panic!("设置 {} 权限失败: {error}", path.display()));
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn objcopy_smoke_fixture() -> (TempDir, PathBuf, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().expect("创建 objcopy smoke 临时目录失败");
    let fake_bin = temp.path().join("fake-bin");
    let sysroot = temp.path().join("sysroot");
    let host = "aarch64-test-unknown";
    let target_lib = sysroot.join("lib/rustlib").join(host).join("lib");
    let root_lib = sysroot.join("lib");
    let objcopy = sysroot
        .join("lib/rustlib")
        .join(host)
        .join("bin/rust-objcopy");
    fs::create_dir_all(&fake_bin).expect("创建 fake bin 目录失败");
    fs::create_dir_all(&target_lib).expect("创建 fake target lib 目录失败");
    fs::create_dir_all(&root_lib).expect("创建 fake root lib 目录失败");
    fs::create_dir_all(objcopy.parent().expect("objcopy 应有父目录"))
        .expect("创建 fake objcopy 目录失败");

    write_executable(
        &fake_bin.join("rustc"),
        "#!/bin/sh\ncase \"$1\" in\n  --print) printf '%s\\n' \"$FAKE_RUST_SYSROOT\" ;;\n  -vV) printf 'host: %s\\n' \"$FAKE_RUST_HOST\" ;;\n  *) exit 2 ;;\nesac\n",
    );
    write_executable(
        &fake_bin.join("uname"),
        "#!/bin/sh\nprintf '%s\\n' \"$FAKE_UNAME\"\n",
    );
    write_executable(
        &objcopy,
        "#!/bin/sh\n[ \"$1\" = --version ] || exit 2\n[ \"${FAKE_OBJCOPY_FAIL:-0}\" = 1 ] && exit 42\nprintf 'fake rust-objcopy\\n'\n",
    );
    (temp, fake_bin, sysroot, objcopy)
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_objcopy_smoke(
    helper: &Path,
    fake_bin: &Path,
    sysroot: &Path,
    host: &str,
    uname: &str,
    fail_objcopy: bool,
) -> std::process::Output {
    let parent_path = std::env::var_os("PATH").unwrap_or_default();
    let path = format!(
        "{}{}{}",
        fake_bin.display(),
        ":",
        parent_path.to_string_lossy()
    );
    let harness = r#"
source "$1"
set -e
LD_LIBRARY_PATH=prior-ld
DYLD_LIBRARY_PATH=prior-dyld
export FAKE_RUST_SYSROOT FAKE_RUST_HOST FAKE_UNAME FAKE_OBJCOPY_FAIL
configure_rust_objcopy_runtime
printf 'LD=%s\nDYLD=%s\n' "${LD_LIBRARY_PATH-}" "${DYLD_LIBRARY_PATH-}"
export -p | grep -E 'declare -x (LD_LIBRARY_PATH|DYLD_LIBRARY_PATH)=' || true
"#;
    Command::new("bash")
        .args([
            "-c",
            harness,
            "bash",
            helper.to_str().expect("helper 路径应为 UTF-8"),
        ])
        .env_clear()
        .env("PATH", path)
        .env("FAKE_RUST_SYSROOT", sysroot)
        .env("FAKE_RUST_HOST", host)
        .env("FAKE_UNAME", uname)
        .env("FAKE_OBJCOPY_FAIL", if fail_objcopy { "1" } else { "0" })
        .output()
        .expect("执行 objcopy smoke harness 失败")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn rust_objcopy_runtime_smoke_exports_the_platform_loader() {
    let (temp, fake_bin, sysroot, _objcopy) = objcopy_smoke_fixture();
    let helper = workspace_root().join(".github/scripts/prepare-rust-objcopy.sh");
    let host = "aarch64-test-unknown";
    let target_lib = sysroot.join("lib/rustlib").join(host).join("lib");
    let root_lib = sysroot.join("lib");
    for (uname, expected, untouched, exported) in [
        (
            "Linux",
            format!(
                "LD={}:{}:prior-ld",
                target_lib.display(),
                root_lib.display()
            ),
            "DYLD=prior-dyld",
            "declare -x LD_LIBRARY_PATH=",
        ),
        (
            "Darwin",
            format!(
                "DYLD={}:{}:prior-dyld",
                target_lib.display(),
                root_lib.display()
            ),
            "LD=prior-ld",
            "declare -x DYLD_LIBRARY_PATH=",
        ),
    ] {
        let output = run_objcopy_smoke(&helper, &fake_bin, &sysroot, host, uname, false);
        assert!(
            output.status.success(),
            "{uname} helper smoke 失败: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains(&expected),
            "{uname} loader 路径不正确: {stdout}"
        );
        assert!(
            stdout.contains(untouched),
            "{uname} 未保留另一平台变量: {stdout}"
        );
        assert!(
            stdout.contains(exported),
            "{uname} loader 变量没有导出给 Dioxus 子进程: {stdout}"
        );
    }
    drop(temp);
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
#[test]
fn rust_objcopy_runtime_smoke_fails_closed_when_objcopy_cannot_start() {
    let (temp, fake_bin, sysroot, _objcopy) = objcopy_smoke_fixture();
    let helper = workspace_root().join(".github/scripts/prepare-rust-objcopy.sh");
    let output = run_objcopy_smoke(
        &helper,
        &fake_bin,
        &sysroot,
        "aarch64-test-unknown",
        "Linux",
        true,
    );
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("rust-objcopy 无法加载 LLVM 动态库"));
    let unsupported = run_objcopy_smoke(
        &helper,
        &fake_bin,
        &sysroot,
        "aarch64-test-unknown",
        "FreeBSD",
        false,
    );
    assert!(!unsupported.status.success());
    assert!(String::from_utf8_lossy(&unsupported.stderr)
        .contains("移动打包暂不支持当前宿主系统的动态库加载"));
    drop(temp);
}

#[test]
fn workflow_isolates_android_signing_environment_from_unsigned_ios() {
    for path in [
        ".github/workflows/alpha.yml",
        ".github/workflows/release.yml",
    ] {
        let workflow = read_workspace_file(path);
        let android = workflow_job(&workflow, "mobile_android");
        let ios = workflow_job(&workflow, "mobile_ios");

        assert!(android.contains("environment: mobile-signing"));
        for secret in [
            "ANDROID_KEYSTORE_BASE64",
            "ANDROID_KEYSTORE_PASSWORD",
            "ANDROID_KEY_ALIAS",
            "ANDROID_KEY_PASSWORD",
            "ANDROID_CERT_SHA256",
        ] {
            assert!(
                android.contains(secret),
                "Android job 缺少 Secret: {secret}"
            );
        }

        assert!(
            !ios.contains("environment:"),
            "iOS job 不得绑定 Environment"
        );
        assert!(!ios.contains("secrets."), "iOS job 不得读取 Secrets");
        assert!(!ios.contains("IOS_"), "iOS job 不得读取历史签名变量");
        assert!(
            !ios.contains("ANDROID_"),
            "iOS job 不得读取 Android 签名变量"
        );
        assert!(ios.contains("Build unsigned iOS IPA"));
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn calculate_mobile_build_number(now: u64) -> u64 {
    let script = workspace_root().join(".github/scripts/allocate-mobile-build-number.sh");
    let output = Command::new("bash")
        .args(["-c", "source \"$1\"; calculate_candidate \"$2\"", "bash"])
        .arg(script)
        .arg(now.to_string())
        .output()
        .expect("应能调用移动构建号脚本");
    assert!(
        output.status.success(),
        "移动构建号计算失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout)
        .expect("移动构建号应为 UTF-8")
        .trim()
        .parse()
        .expect("移动构建号应为整数")
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_mobile_build_number_allocator() -> (String, String) {
    let script = workspace_root().join(".github/scripts/allocate-mobile-build-number.sh");
    let temp = tempfile::tempdir().expect("创建 allocator 主流程测试目录失败");
    let output_path = temp.path().join("github-output");
    let output = Command::new("bash")
        .args([
            "-c",
            "exec 3<<<$'1787238033\\n1787238034\\n'; source \"$1\"; \
             clock_now(){ IFS= read -r value <&3; printf '%s\\n' \"$value\"; }; \
             sleep_seconds(){ :; }; main alpha",
            "bash",
        ])
        .arg(script)
        .env("GITHUB_OUTPUT", &output_path)
        .output()
        .expect("应能执行移动构建号脚本主流程");
    let github_output = fs::read_to_string(&output_path).unwrap_or_default();
    assert!(
        output.status.success(),
        "移动构建号主流程失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8(output.stdout).expect("allocator stdout 应为 UTF-8"),
        github_output,
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_mobile_build_number_allocator_at_limit() -> (String, String) {
    let script = workspace_root().join(".github/scripts/allocate-mobile-build-number.sh");
    let temp = tempfile::tempdir().expect("创建 allocator 上限测试目录失败");
    let output_path = temp.path().join("github-output");
    let output = Command::new("bash")
        .args([
            "-c",
            "exec 3<<<$'2100000000\n2100000001\n'; source \"$1\"; \
             clock_now(){ IFS= read -r value <&3; printf '%s\n' \"$value\"; }; \
             sleep_seconds(){ :; }; main release",
            "bash",
        ])
        .arg(script)
        .env("GITHUB_OUTPUT", &output_path)
        .output()
        .expect("应能执行移动构建号上限主流程");
    let github_output = fs::read_to_string(&output_path).unwrap_or_default();
    assert!(
        output.status.success(),
        "移动构建号上限主流程失败: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    (
        String::from_utf8(output.stdout).expect("allocator 上限 stdout 应为 UTF-8"),
        github_output,
    )
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn run_mobile_build_number_failure(
    harness: &str,
    github_output: Option<&Path>,
) -> std::process::Output {
    let script = workspace_root().join(".github/scripts/allocate-mobile-build-number.sh");
    let mut command = Command::new("bash");
    command.args(["-c", harness, "bash"]).arg(script);
    if let Some(path) = github_output {
        command.env("GITHUB_OUTPUT", path);
    } else {
        command.env_remove("GITHUB_OUTPUT");
    }
    command.output().expect("应能执行移动构建号失败路径")
}

#[test]
fn mobile_build_number_allocator_is_read_only_serialized_and_bounded() {
    let allocator = read_workspace_file(".github/scripts/allocate-mobile-build-number.sh");
    for required in [
        "CUTOVER_FLOOR=1787238032",
        "MAX_BUILD_NUMBER=2100000000",
        "MAX_WAIT_SECONDS=30",
        "MAX_CLOCK_VALUE=9223372036854775807",
        "date -u +%s",
        "calculate_candidate",
        "wait_for_slot",
        "now > candidate",
        "系统 UTC 时钟发生回拨",
        "GITHUB_OUTPUT",
    ] {
        assert!(
            allocator.contains(required),
            "移动构建号分配器缺少: {required}"
        );
    }
    for forbidden in ["gh api", "GH_TOKEN", "git/refs", "jq", "force=false"] {
        assert!(
            !allocator.contains(forbidden),
            "移动构建号分配器不得依赖 GitHub 写状态: {forbidden}"
        );
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    {
        assert_eq!(calculate_mobile_build_number(1_787_238_031), 1_787_238_033);
        assert_eq!(calculate_mobile_build_number(1_787_238_032), 1_787_238_033);
        assert_eq!(calculate_mobile_build_number(1_787_238_033), 1_787_238_033);
        assert_eq!(calculate_mobile_build_number(2_100_000_000), 2_100_000_000);
        let (stdout, github_output) = run_mobile_build_number_allocator();
        assert!(stdout.contains("已分配移动构建号: 1787238033（alpha）"));
        assert_eq!(github_output, "build_number=1787238033\n");
        let (limit_stdout, limit_output) = run_mobile_build_number_allocator_at_limit();
        assert!(limit_stdout.contains("已分配移动构建号: 2100000000（release）"));
        assert_eq!(limit_output, "build_number=2100000000\n");
    }

    for (path, channel) in [
        (".github/workflows/alpha.yml", "alpha"),
        (".github/workflows/release.yml", "release"),
    ] {
        let workflow = read_workspace_file(path);
        let allocator_job = workflow_job(&workflow, "allocate_mobile_build_number");
        assert!(allocator_job.contains("group: mobile-build-number"));
        assert!(allocator_job.contains("cancel-in-progress: false"));
        assert!(allocator_job.contains(&format!(
            ".github/scripts/allocate-mobile-build-number.sh {channel}"
        )));
        assert!(allocator_job.contains("shell: bash"));
        assert!(!allocator_job.contains("contents: write"));
        assert!(!allocator_job.contains("GH_TOKEN"));
        assert!(!workflow.contains("MOBILE_BUILD_NUMBER=$(date -u +%s)"));
    }
}

#[test]
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn mobile_build_number_allocator_fails_closed_on_invalid_runtime_state() {
    let temp = tempfile::tempdir().expect("创建 allocator 测试目录失败");
    let output_path = temp.path().join("github-output");
    let invalid_channel =
        run_mobile_build_number_failure("source \"$1\"; main beta", Some(&output_path));
    assert!(!invalid_channel.status.success());
    assert!(String::from_utf8_lossy(&invalid_channel.stderr).contains("未知移动构建通道"));

    let missing_output = run_mobile_build_number_failure("source \"$1\"; main alpha", None);
    assert!(!missing_output.status.success());
    assert!(String::from_utf8_lossy(&missing_output.stderr).contains("缺少环境变量: GITHUB_OUTPUT"));

    let rollback = run_mobile_build_number_failure(
        "exec 3<<<$'1787238034\\n1787238033\\n'; source \"$1\"; \
         clock_now(){ IFS= read -r value <&3; printf '%s\\n' \"$value\"; }; \
         sleep_seconds(){ :; }; main alpha",
        Some(&output_path),
    );
    assert!(!rollback.status.success());
    assert!(String::from_utf8_lossy(&rollback.stderr).contains("系统 UTC 时钟发生回拨"));

    let timeout = run_mobile_build_number_failure(
        "source \"$1\"; clock_now(){ printf '1787238034\\n'; }; \
         sleep_seconds(){ :; }; main release",
        Some(&output_path),
    );
    assert!(!timeout.status.success());
    assert!(String::from_utf8_lossy(&timeout.stderr).contains("等待移动构建号时间槽超时"));

    let over_limit = run_mobile_build_number_failure(
        "source \"$1\"; clock_now(){ printf '2100000001\\n'; }; main alpha",
        Some(&output_path),
    );
    assert!(!over_limit.status.success());
    assert!(String::from_utf8_lossy(&over_limit.stderr).contains("超出 Android versionCode 上限"));

    for (clock_value, expected_error) in [
        ("", "无法生成有效的 UTC Unix 秒构建号"),
        ("0", "无法生成有效的 UTC Unix 秒构建号"),
        ("not-a-number", "无法生成有效的 UTC Unix 秒构建号"),
        (
            "9223372036854775808",
            "UTC Unix 秒时钟值超出 Bash 可计算范围",
        ),
    ] {
        let harness =
            format!("source \"$1\"; clock_now(){{ printf '%s\\n' '{clock_value}'; }}; main alpha");
        let invalid_clock = run_mobile_build_number_failure(&harness, Some(&output_path));
        assert!(!invalid_clock.status.success());
        assert!(String::from_utf8_lossy(&invalid_clock.stderr).contains(expected_error));
    }

    assert!(
        !output_path.exists()
            || fs::read_to_string(&output_path)
                .expect("读取失败路径 GITHUB_OUTPUT 失败")
                .is_empty(),
        "allocator 失败路径不得写入 build_number"
    );

    let unwritable_output = run_mobile_build_number_failure(
        "exec 3<<<$'1787238034\\n1787238035\\n'; source \"$1\"; \
         clock_now(){ IFS= read -r value <&3; printf '%s\\n' \"$value\"; }; \
         sleep_seconds(){ :; }; main alpha",
        Some(temp.path()),
    );
    assert!(!unwritable_output.status.success());
    assert!(String::from_utf8_lossy(&unwritable_output.stderr)
        .contains("无法写入 GitHub Actions 输出文件"));
}

#[test]
fn release_mobile_version_matches_the_workspace_version() {
    let release = read_workspace_file(".github/workflows/release.yml");
    assert!(release.contains("workspace_version="));
    assert!(release.contains("marketing_version\" != \"$workspace_version"));
    assert!(release.contains("正式 tag 版本必须与 Cargo.toml 一致"));
}

#[test]
fn android_packager_fails_closed_on_signing_identity_mismatch() {
    let android = read_workspace_file(".github/scripts/package-android-apk.sh");
    for required in [
        "ANDROID_CERT_SHA256",
        "apksigner",
        "--ks-type PKCS12",
        "MOBILE_BUNDLE_ID",
        "MOBILE_BUILD_NUMBER",
        "arm64-v8a",
        "application-icon-",
        "WryActivity.kt",
        "AndroidManifest.xml",
        "uses-permission android:name=\"android.permission.INTERNET\"",
        "setDecorFitsSystemWindows(false)",
        "statusBarColor = Color.TRANSPARENT",
        "navigationBarColor = Color.TRANSPARENT",
        "isNavigationBarContrastEnforced = false",
        "AAPT\" dump permissions",
        "android.permission.INTERNET",
        "Android Gradle Plugin 可能缩短 APK 内资源路径",
        "dx --version",
    ] {
        assert!(
            android.contains(required),
            "Android 签名脚本缺少: {required}"
        );
    }
    assert!(
        !android.contains("^res/mipmap-[^/]+/ic_launcher\\.png$"),
        "APK 内资源路径可能被 AGP 缩短，不得恢复硬编码图标路径校验"
    );
}

#[test]
fn ios_packager_outputs_a_verified_unsigned_arm64_ipa() {
    let ios = read_workspace_file(".github/scripts/package-ios-ipa.sh");
    for required in [
        "CODE_SIGNING_ALLOWED=NO",
        "CODE_SIGNING_REQUIRED=NO",
        "-path '*/release/ios/*'",
        "MOBILE_BUNDLE_ID",
        "MOBILE_MARKETING_VERSION",
        "MOBILE_BUILD_NUMBER",
        "IPA 顶层只能包含 Payload 目录",
        "Payload",
        "ios-aarch64-unsigned.ipa",
        "arm64",
        "x86_64",
        "embedded.mobileprovision",
        "_CodeSignature",
        "CodeResources",
        "codesign --remove-signature",
        "codesign -d",
        "未签名 iOS IPA 已生成并通过结构校验",
        "NSLocalNetworkUsageDescription",
        "局域网设备以同步配置",
        "dx --version",
    ] {
        assert!(ios.contains(required), "iOS 未签名脚本缺少: {required}");
    }
    for forbidden in [
        "IOS_",
        "IOS_TEAM_ID",
        "IOS_CERTIFICATE",
        "security import",
        "security create-keychain",
        "codesign --force",
        "DeveloperCertificates",
    ] {
        assert!(
            !ios.contains(forbidden),
            "iOS 脚本仍包含签名逻辑: {forbidden}"
        );
    }
}

#[test]
fn alpha_publish_stages_assets_then_switches_the_rolling_tag() {
    let alpha = read_workspace_file(".github/workflows/alpha.yml");
    assert!(alpha.contains("needs: [audit, build, mobile_android, mobile_ios]"));
    assert!(alpha.contains("branches:\n      - main"));
    assert!(alpha.contains("cancel-in-progress: false"));
    assert!(alpha.contains("alpha-stage-${{ github.run_id }}-${{ github.run_attempt }}"));
    assert!(alpha.contains("draft: true"));
    assert!(alpha.contains(".github/scripts/publish-alpha.sh"));
    assert!(alpha.contains("make_latest: false"));

    let publisher = read_workspace_file(".github/scripts/publish-alpha.sh");
    for required in [
        "alpha-backup-",
        "rollback",
        "trap on_exit EXIT",
        "transaction_started=true",
        "committed=true",
        "local-assets.txt",
        "remote-assets.txt",
        "git/refs/${ALPHA_REF}",
        "-F force=true",
        "Alpha tag 与新 Release 切换后复验失败",
    ] {
        assert!(
            publisher.contains(required),
            "Alpha 发布切换脚本缺少: {required}"
        );
    }
}

#[test]
fn release_notes_explain_that_ios_requires_user_resigning() {
    for workflow in [
        ".github/workflows/alpha.yml",
        ".github/workflows/release.yml",
    ] {
        let workflow = read_workspace_file(workflow);
        assert!(workflow.contains("IPA 未签名，不能直接安装"));
        assert!(workflow.contains("provisioning profile 重签"));
    }

    let release = read_workspace_file(".github/workflows/release.yml");
    assert!(release.contains("needs: [build, mobile_android, mobile_ios]"));
}
