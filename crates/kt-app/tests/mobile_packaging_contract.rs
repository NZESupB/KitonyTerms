use std::fs;
use std::path::{Path, PathBuf};

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

#[test]
fn mobile_build_number_allocator_is_global_monotonic_and_bounded() {
    let allocator = read_workspace_file(".github/scripts/allocate-mobile-build-number.sh");
    for required in [
        "refs/${COUNTER_REF}",
        "mobile-build-number:",
        "last_build_number + 1",
        "2100000000",
        "force=false",
        "MAX_ATTEMPTS",
        "allocator: %s.%s.%s",
        "GITHUB_RUN_ATTEMPT",
        "commits/${GITHUB_SHA}",
        ".commit.tree.sha",
    ] {
        assert!(
            allocator.contains(required),
            "移动构建号分配器缺少: {required}"
        );
    }

    for workflow in [
        ".github/workflows/alpha.yml",
        ".github/workflows/release.yml",
    ] {
        let workflow = read_workspace_file(workflow);
        assert!(!workflow.contains("MOBILE_BUILD_NUMBER=$(date -u +%s)"));
    }
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
    ] {
        assert!(
            android.contains(required),
            "Android 签名脚本缺少: {required}"
        );
    }
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
