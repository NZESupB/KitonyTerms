fn main() {
    println!(
        "cargo:rustc-env=KT_UPDATE_TARGET={}",
        std::env::var("TARGET").unwrap()
    );
    for (key, default) in [
        ("KT_RELEASE_CHANNEL", "development"),
        ("KT_RELEASE_COMMIT", ""),
        ("KT_RELEASE_BUILD", "0"),
        ("KT_RELEASE_ATTEMPT", "0"),
    ] {
        println!("cargo:rerun-if-env-changed={key}");
        let value = std::env::var(key).unwrap_or_else(|_| default.to_string());
        assert!(!value.contains(['\n', '\r']), "无效构建元数据");
        println!("cargo:rustc-env={key}={value}");
    }
}
