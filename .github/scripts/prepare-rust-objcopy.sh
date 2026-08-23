#!/usr/bin/env bash

# Dioxus release bundling 会调用宿主 Rust 工具链的 rust-objcopy。
# rustup 将 rust-objcopy 放在 host rustlib 的 bin 目录，而 libLLVM 放在
# sysroot 的 lib 目录（部分版本还提供 host rustlib/lib）。Dioxus 当前只设置
# Linux 使用的加载器变量，因此这里按宿主系统补齐正确的 loader 环境。

configure_rust_objcopy_runtime() {
  command -v rustc >/dev/null 2>&1 || {
    echo "::error::未安装 rustc，无法定位 rust-objcopy" >&2
    return 1
  }

  local rust_sysroot host objcopy target_lib root_lib library_path
  rust_sysroot="$(rustc --print sysroot 2>/dev/null)" || {
    echo "::error::无法读取 Rust sysroot" >&2
    return 1
  }
  [[ -n "$rust_sysroot" && -d "$rust_sysroot" ]] || {
    echo "::error::Rust sysroot 不存在: $rust_sysroot" >&2
    return 1
  }

  host="$(rustc -vV 2>/dev/null | sed -n 's/^host: //p')" || {
    echo "::error::无法读取 Rust host triple" >&2
    return 1
  }
  [[ "$host" =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || {
    echo "::error::Rust host triple 无效: $host" >&2
    return 1
  }

  objcopy="$rust_sysroot/lib/rustlib/$host/bin/rust-objcopy"
  [[ -x "$objcopy" ]] || {
    echo "::error::缺少 $objcopy，请在移动 job 安装 llvm-tools-preview" >&2
    return 1
  }

  target_lib="$rust_sysroot/lib/rustlib/$host/lib"
  root_lib="$rust_sysroot/lib"
  library_path=""
  [[ -d "$target_lib" ]] && library_path="$target_lib"
  if [[ -d "$root_lib" ]]; then
    if [[ -n "$library_path" ]]; then
      library_path="$library_path:$root_lib"
    else
      library_path="$root_lib"
    fi
  fi
  [[ -n "$library_path" ]] || {
    echo "::error::Rust sysroot 中没有可用的 LLVM 动态库目录" >&2
    return 1
  }

  case "$(uname -s)" in
    Darwin)
      export DYLD_LIBRARY_PATH="$library_path${DYLD_LIBRARY_PATH:+:$DYLD_LIBRARY_PATH}"
      ;;
    Linux)
      export LD_LIBRARY_PATH="$library_path${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
      ;;
    *)
      echo "::error::移动打包暂不支持当前宿主系统的动态库加载: $(uname -s)" >&2
      return 1
      ;;
  esac

  if ! "$objcopy" --version >/dev/null 2>&1; then
    echo "::error::rust-objcopy 无法加载 LLVM 动态库，请确认 llvm-tools-preview 已安装" >&2
    return 1
  fi

  echo "Rust objcopy runtime 已准备: host=$host, path=$objcopy"
}
