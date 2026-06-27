use std::env;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=assets/windows/kitonyterms.ico");

    let target_os = env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if target_os != "windows" {
        return;
    }

    embed_windows_icon().unwrap_or_else(|error| panic!("无法嵌入 Windows 应用图标: {error}"));
}

fn embed_windows_icon() -> Result<(), String> {
    let manifest_dir =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").map_err(|err| err.to_string())?);
    let out_dir = PathBuf::from(env::var("OUT_DIR").map_err(|err| err.to_string())?);
    let icon_path = manifest_dir.join("assets/windows/kitonyterms.ico");
    let out_icon_path = out_dir.join("kitonyterms.ico");
    let rc_path = out_dir.join("kitonyterms-icon.rc");
    let res_path = out_dir.join("kitonyterms-icon.res");

    fs::copy(&icon_path, &out_icon_path).map_err(|err| err.to_string())?;
    fs::write(
        &rc_path,
        r#"1 ICON "kitonyterms.ico"
"#,
    )
    .map_err(|err| err.to_string())?;

    let rc = find_windows_tool("rc.exe").unwrap_or_else(|| PathBuf::from("rc.exe"));
    let args = vec![
        OsString::from("/nologo"),
        OsString::from("/fo"),
        res_path.as_os_str().to_os_string(),
        rc_path.as_os_str().to_os_string(),
    ];
    run_tool(&rc, &args, &out_dir)?;

    println!(
        "cargo:rustc-link-arg-bin=kitonyterms={}",
        res_path.display()
    );
    Ok(())
}

fn run_tool(program: &Path, args: &[OsString], current_dir: &Path) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .current_dir(current_dir)
        .status()
        .map_err(|err| format!("执行 {} 失败: {err}", program.display()))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("{} 退出状态 {status}", program.display()))
    }
}

fn find_windows_tool(name: &str) -> Option<PathBuf> {
    find_tool_in_path(name).or_else(|| find_tool_in_windows_kits(name))
}

fn find_tool_in_path(name: &str) -> Option<PathBuf> {
    env::var_os("PATH").and_then(|paths| {
        env::split_paths(&paths)
            .map(|dir| dir.join(name))
            .find(|path| path.is_file())
    })
}

fn find_tool_in_windows_kits(name: &str) -> Option<PathBuf> {
    let mut kit_roots = ["ProgramFiles(x86)", "ProgramFiles"]
        .into_iter()
        .filter_map(env::var_os)
        .map(PathBuf::from)
        .map(|root| root.join("Windows Kits/10/bin"))
        .filter(|path| path.is_dir())
        .flat_map(|path| fs::read_dir(path).into_iter().flatten().flatten())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect::<Vec<_>>();

    kit_roots.sort();
    kit_roots.reverse();

    for root in kit_roots {
        for arch in windows_tool_arch_candidates() {
            let candidate = root.join(arch).join(name);
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }

    None
}

fn windows_tool_arch_candidates() -> &'static [&'static str] {
    let arch = env::var("CARGO_CFG_TARGET_ARCH").unwrap_or_else(|_| env::consts::ARCH.to_owned());
    match arch.as_str() {
        "aarch64" => &["arm64", "x64", "x86"],
        "x86_64" => &["x64", "x86"],
        "x86" => &["x86", "x64"],
        _ => &["x64", "arm64", "x86"],
    }
}
