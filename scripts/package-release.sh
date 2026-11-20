#!/usr/bin/env bash
# 把一个 release 二进制打成可分发的产物。
#
# 由 `.github/workflows/release.yml` 调用，**也可以本地跑**：同一条命令产出
# 同一种包，否则「打包」就成了只有 CI 才知道的黑盒，本地没法验证包里的
# 目录结构对不对。
#
# 用法：
#   scripts/package-release.sh <target-triple> <version> <binary> [outdir]
# 例：
#   scripts/package-release.sh aarch64-apple-darwin 1.0.0 \
#       target/aarch64-apple-darwin/release/term-desk dist
#
# 产物（按平台分支）：
#   macOS   → WindTerm-<ver>-macos-<arch>.tar.gz   内含 WindTerm.app/（真 bundle，不是裸二进制）
#   Linux   → WindTerm-<ver>-linux-<arch>.tar.gz   内含 windterm + .desktop + 图标
#   Windows → WindTerm-<ver>-windows-<arch>.zip    内含 WindTerm.exe
#
# 为什么要重命名二进制：包名（`term-desk`）是 crate 名，用户看到的名字是产品名。
# macOS 尤其不能省——`CFBundleExecutable` 必须与 `Contents/MacOS/` 下的文件名
# 逐字一致，否则 Finder 双击只会得到「应用已损坏」。
set -euo pipefail

TARGET="${1:?用法: package-release.sh <target-triple> <version> <binary> [outdir]}"
VERSION="${2:?缺少 version}"
BIN="${3:?缺少二进制路径}"
OUT="${4:-dist}"

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
NAME="WindTerm"

[ -f "$BIN" ] || { echo "二进制不存在：$BIN" >&2; exit 1; }
mkdir -p "$OUT"
OUT="$(cd "$OUT" && pwd)"

# 平台标签：给用户看的，不是 triple。用 triple 会得到
# `WindTerm-1.0.0-x86_64-unknown-linux-gnu.tar.gz` 这种没人想读的文件名。
case "$TARGET" in
  aarch64-apple-darwin)     PLATFORM="macos-aarch64" ;;
  x86_64-apple-darwin)      PLATFORM="macos-x86_64" ;;
  x86_64-unknown-linux-gnu) PLATFORM="linux-x86_64" ;;
  aarch64-unknown-linux-gnu) PLATFORM="linux-aarch64" ;;
  x86_64-pc-windows-msvc)   PLATFORM="windows-x86_64" ;;
  x86_64-pc-windows-gnu)    PLATFORM="windows-x86_64" ;;
  *)
    echo "未知 target：$TARGET（新增平台请在此登记平台标签与打包分支）" >&2
    exit 1
    ;;
esac

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

# 所有平台共用：把 LICENSE 与 README 一起带上。发布产物里没有许可证文本，
# 用户拿到的是一个无法确定授权状态的可执行文件——这不该靠「仓库里有」解决。
cp "$ROOT/LICENSE" "$STAGE/LICENSE"
cp "$ROOT/README.md" "$STAGE/README.md"

case "$TARGET" in
  *-apple-darwin)
    APP="$STAGE/$NAME.app"
    mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"

    cp "$BIN" "$APP/Contents/MacOS/$NAME"
    chmod +x "$APP/Contents/MacOS/$NAME"

    # 图标：`assets/icon.icns` 由 `scripts/gen-icon.py` 生成并提交。这里不现生成，
    # 因为打包机上不一定有 python3 + Pillow，而图标是**构建输入**不是构建产物。
    if [ -f "$ROOT/assets/icon.icns" ]; then
      cp "$ROOT/assets/icon.icns" "$APP/Contents/Resources/$NAME.icns"
      ICON_KEY="<key>CFBundleIconFile</key><string>$NAME</string>"
    else
      ICON_KEY=""
    fi

    # Info.plist 是本脚本唯一手写的「平台元数据」，逐项都有用途：
    #   · CFBundleExecutable 必须等于 Contents/MacOS 下的文件名（否则打不开）
    #   · LSMinimumSystemVersion 11.0：eframe 0.29 的 winit 走 AppKit，
    #     10.x 上没验证过，不宣称支持
    #   · NSHighResolutionCapable：不给 true 的话 Retina 上会是 2× 模糊
    #   · CFBundleIdentifier 用反域名，LaunchServices 靠它区分不同 app
    cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>$NAME</string>
  <key>CFBundleDisplayName</key><string>$NAME</string>
  <key>CFBundleIdentifier</key><string>com.ezfavorites.windterm</string>
  <key>CFBundleVersion</key><string>$VERSION</string>
  <key>CFBundleShortVersionString</key><string>$VERSION</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleExecutable</key><string>$NAME</string>
  $ICON_KEY
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>NSHighResolutionCapable</key><true/>
  <key>NSSupportsAutomaticGraphicsSwitching</key><true/>
</dict>
</plist>
PLIST

    # 未签名的 .app 在别的机器上会被 Gatekeeper 拦（"无法验证开发者"）。
    # CI 里没有开发者证书，所以这里**如实标注**而不是假装签过：ad-hoc 签名
    # 不解决 Gatekeeper，只是让 macOS 不去抱怨「签名已损坏」。
    # 真正分发需要 codesign + notarytool，那是 1.0.x 的独立工作。
    if command -v codesign >/dev/null 2>&1; then
      codesign --force --deep --sign - "$APP" 2>/dev/null || true
    fi

    tar -czf "$OUT/$NAME-$VERSION-$PLATFORM.tar.gz" -C "$STAGE" "$NAME.app"
    ARTIFACT="$NAME-$VERSION-$PLATFORM.tar.gz"
    ;;

  *-linux-gnu)
    cp "$BIN" "$STAGE/windterm"
    chmod +x "$STAGE/windterm"

    if [ -f "$ROOT/assets/icon.png" ]; then
      cp "$ROOT/assets/icon.png" "$STAGE/windterm.png"
    fi

    # .desktop：Linux 上「装进应用菜单」靠它。Exec 用包内相对名，
    # 因为包是免安装的 tar.gz——用户解压到哪都能直接把 .desktop 里的
    # Exec 换成绝对路径，比写死 /usr/bin 更实际。
    cat > "$STAGE/windterm.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=WindTerm
Comment=A terminal built on egui
Exec=windterm
Icon=windterm
Terminal=false
Categories=System;TerminalEmulator;
Keywords=terminal;shell;ssh;sftp;
DESKTOP

    tar -czf "$OUT/$NAME-$VERSION-$PLATFORM.tar.gz" -C "$STAGE" .
    ARTIFACT="$NAME-$VERSION-$PLATFORM.tar.gz"
    ;;

  *-windows-*)
    cp "$BIN" "$STAGE/$NAME.exe"
    # 图标一并带走：本 exe **没有**把 .ico 编进 PE 资源（那要加 winres/embed-resource
    # 构建依赖，属 1.0.x），所以资源管理器里显示的是默认图标。把 .ico 放在旁边，
    # 用户做快捷方式时至少能手工指定——比让人以为「忘了带图标」诚实。
    if [ -f "$ROOT/assets/icon.ico" ]; then
      cp "$ROOT/assets/icon.ico" "$STAGE/$NAME.ico"
    fi
    # Windows 没有 bundle 概念：一个 exe + 说明书就是全部。
    # zip 而不是 tar.gz——Windows 资源管理器原生认 zip，双击即可解压。
    #
    # 为什么不是直接 `zip` 一条路：GitHub 的 windows runner 上本脚本跑在 Git Bash
    # 里，而 Git for Windows **不带** zip（`tar` 倒是有）。7-Zip 是 runner 预装的，
    # 因此退到 `7z a -tzip`。两条都不在才报错，并说清补哪个，而不是让 make 语法
    # 错误糊过去。
    ARTIFACT="$NAME-$VERSION-$PLATFORM.zip"
    if command -v zip >/dev/null 2>&1; then
      (cd "$STAGE" && zip -q -r "$OUT/$ARTIFACT" .)
    elif command -v 7z >/dev/null 2>&1; then
      (cd "$STAGE" && 7z a -tzip -bso0 -bsp0 "$OUT/$ARTIFACT" . >/dev/null)
    else
      echo "既没有 zip 也没有 7z，无法产出 .zip（runner 上装 7-Zip 或 p7zip）" >&2
      exit 1
    fi
    ;;
esac

# 校验和：发布产物必须能被下游验证完整性。`sha256sum` 在 GNU coreutils（Linux）
# 上叫这个名字，macOS 只有 `shasum`；Windows Git Bash 两个都没有时退到 `openssl`。
# 输出格式用 `<hash>  <文件名>`（两个空格），`sha256sum -c` 能直接吃。
if command -v sha256sum >/dev/null 2>&1; then
  (cd "$OUT" && sha256sum "$ARTIFACT" > "$ARTIFACT.sha256")
elif command -v shasum >/dev/null 2>&1; then
  (cd "$OUT" && shasum -a 256 "$ARTIFACT" > "$ARTIFACT.sha256")
elif command -v openssl >/dev/null 2>&1; then
  (cd "$OUT" && printf '%s  %s\n' "$(openssl dgst -sha256 "$ARTIFACT" | sed 's/.*= //')" "$ARTIFACT" > "$ARTIFACT.sha256")
else
  echo "警告：找不到 sha256sum / shasum / openssl，跳过校验和" >&2
fi

echo "打包完成：$OUT/$ARTIFACT"
ls -lh "$OUT/$ARTIFACT" 2>/dev/null || true
if [ -f "$OUT/$ARTIFACT.sha256" ]; then
  echo "校验和：$(cat "$OUT/$ARTIFACT.sha256")"
fi
