#!/usr/bin/env bash
# 把一个 release 二进制按平台打成**用户可安装**的产物（mailcraft 的三平台口径）。
#
# 由 `.github/workflows/release.yml` 调用，也可以本地跑：同一条命令产出同一种包。
#
# 用法：
#   scripts/package-release.sh <target-triple> <version> <binary> [outdir]
# 例：
#   scripts/package-release.sh aarch64-apple-darwin 0.0.1-beta2 \
#       target/aarch64-apple-darwin/release/term-desk dist
#
# 产物（按平台分支）：
#   macOS   → WindTerm-<ver>-macos-<arch>.dmg        WindTerm.app + /Applications 链接
#   Linux   → WindTerm-<ver>-linux-x86_64.deb        FHS 安装（dpkg -i）
#              WindTerm-<ver>-linux-x86_64.zip       便携目录（解压即用）
#   Windows → WindTerm-<ver>-windows-x86_64.msi      每用户安装器（WiX v5）
#              WindTerm-<ver>-windows-x86_64.zip     便携目录
# 每个产物都配一个 `<产物名>.sha256`。
#
# 依赖：macOS 需 hdiutil（系统自带）；Linux 需 dpkg-deb；Windows 需 WiX v5
# （`dotnet tool install -g wix --version 5.0.2` + WixToolset.UI.wixext）。
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

case "$TARGET" in
  aarch64-apple-darwin)     PLATFORM="macos-aarch64" ;;
  x86_64-apple-darwin)      PLATFORM="macos-x86_64" ;;
  x86_64-unknown-linux-gnu) PLATFORM="linux-x86_64" ;;
  x86_64-pc-windows-msvc)   PLATFORM="windows-x86_64" ;;
  x86_64-pc-windows-gnu)    PLATFORM="windows-x86_64" ;;
  *)
    echo "未知 target：$TARGET（新增平台请在此登记平台标签与打包分支）" >&2
    exit 1
    ;;
esac

STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT
ARTIFACTS=()

# 所有平台都带许可证、说明书与 Rhai 示例。发布产物里没有许可证文本，
# 用户拿到的是一个无法确定授权状态的可执行文件——这不该靠「仓库里有」解决。
common_docs() {
  local dir="$1"
  cp "$ROOT/LICENSE" "$dir/LICENSE"
  cp "$ROOT/README.md" "$dir/README.md"
  if [ -d "$ROOT/apps/desktop/resources/scripts" ]; then
    mkdir -p "$dir/scripts"
    cp -R "$ROOT/apps/desktop/resources/scripts/." "$dir/scripts/"
  fi
}

winpath() {
  # Git Bash 的 /tmp/... 路径 Windows 原生 exe 看不懂；交给 cygpath 转换。
  if command -v cygpath >/dev/null 2>&1; then cygpath -w "$1"; else printf '%s' "$1"; fi
}

zip_dir() {
  # 在 Git Bash 里 zip 不一定在，7-Zip 是 windows runner 预装的。
  local src="$1" dst="$2"
  if command -v zip >/dev/null 2>&1; then
    (cd "$src" && zip -q -r "$dst" .)
  elif command -v 7z >/dev/null 2>&1; then
    (cd "$src" && 7z a -tzip -bso0 -bsp0 "$dst" . >/dev/null)
  else
    echo "既没有 zip 也没有 7z，无法产出 .zip" >&2
    exit 1
  fi
}

sha256_of() {
  local f="$1"
  if command -v sha256sum >/dev/null 2>&1; then
    (cd "$OUT" && sha256sum "$f" > "$f.sha256")
  elif command -v shasum >/dev/null 2>&1; then
    (cd "$OUT" && shasum -a 256 "$f" > "$f.sha256")
  elif command -v openssl >/dev/null 2>&1; then
    (cd "$OUT" && printf '%s  %s\n' "$(openssl dgst -sha256 "$f" | sed 's/.*= //')" "$f" > "$f.sha256")
  else
    echo "找不到 sha256sum / shasum / openssl，跳过校验和" >&2
  fi
}

case "$TARGET" in
  *-apple-darwin)
    APP="$STAGE/$NAME.app"
    mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources"
    common_docs "$APP/Contents/Resources"
    cp "$BIN" "$APP/Contents/MacOS/$NAME"
    chmod +x "$APP/Contents/MacOS/$NAME"

    ICON_KEY=""
    if [ -f "$ROOT/assets/icon.icns" ]; then
      cp "$ROOT/assets/icon.icns" "$APP/Contents/Resources/$NAME.icns"
      ICON_KEY="<key>CFBundleIconFile</key><string>$NAME</string>"
    fi
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

    # ad-hoc 签名：不解决 Gatekeeper，但避免「签名已损坏」；公证属 1.0.x。
    if command -v codesign >/dev/null 2>&1; then
      codesign --force --deep --sign - "$APP" 2>/dev/null || true
    fi

    # DMG：一个 .app + /Applications 链接，拖进去即是安装。hdiutil create
    # 不需要 Finder 脚本，CI 上可重复（mailcraft 的同一条路径）。
    DMG_STAGE="$STAGE/dmg"
    mkdir -p "$DMG_STAGE"
    cp -R "$APP" "$DMG_STAGE/$NAME.app"
    ln -s /Applications "$DMG_STAGE/Applications"
    cp "$ROOT/LICENSE" "$DMG_STAGE/LICENSE"
    cp "$ROOT/README.md" "$DMG_STAGE/README.md"
    ARTIFACT="$NAME-$VERSION-$PLATFORM.dmg"
    command -v hdiutil >/dev/null || { echo "macOS 打包需要 hdiutil" >&2; exit 1; }
    hdiutil create -volname "$NAME" -srcfolder "$DMG_STAGE" -ov -format UDZO "$OUT/$ARTIFACT" >/dev/null
    ARTIFACTS+=("$ARTIFACT")
    ;;

  *-linux-gnu)
    command -v dpkg-deb >/dev/null || { echo "Linux .deb 打包需要 dpkg-deb" >&2; exit 1; }

    # 便携包：解压即用，不写 /usr。
    PORT="$STAGE/portable"
    mkdir -p "$PORT"
    cp "$BIN" "$PORT/windterm"
    chmod +x "$PORT/windterm"
    common_docs "$PORT"
    [ -f "$ROOT/assets/icon.png" ] && cp "$ROOT/assets/icon.png" "$PORT/windterm.png"
    cat > "$PORT/windterm.desktop" <<DESKTOP
[Desktop Entry]
Type=Application
Name=WindTerm
Comment=A local-first desktop terminal workbench
Exec=windterm
Icon=windterm
Terminal=false
Categories=System;TerminalEmulator;
Keywords=terminal;shell;ssh;sftp;
DESKTOP
    ZIP_ART="$NAME-$VERSION-$PLATFORM.zip"
    zip_dir "$PORT" "$OUT/$ZIP_ART"
    ARTIFACTS+=("$ZIP_ART")

    # .deb：FHS 安装，dpkg -i 即可。
    DEB_ROOT="$STAGE/deb"
    mkdir -p "$DEB_ROOT/DEBIAN" \
      "$DEB_ROOT/usr/bin" \
      "$DEB_ROOT/usr/share/applications" \
      "$DEB_ROOT/usr/share/doc/windterm" \
      "$DEB_ROOT/usr/share/icons/hicolor/256x256/apps"
    install -m755 "$BIN" "$DEB_ROOT/usr/bin/windterm"
    cp "$PORT/windterm.desktop" "$DEB_ROOT/usr/share/applications/windterm.desktop"
    [ -f "$ROOT/assets/icon.png" ] && cp "$ROOT/assets/icon.png" "$DEB_ROOT/usr/share/icons/hicolor/256x256/apps/windterm.png"
    cp "$ROOT/LICENSE" "$DEB_ROOT/usr/share/doc/windterm/copyright"
    cp "$ROOT/README.md" "$DEB_ROOT/usr/share/doc/windterm/README.md"
    cat > "$DEB_ROOT/DEBIAN/control" <<CONTROL
Package: windterm
Version: $VERSION
Section: utils
Priority: optional
Architecture: amd64
Maintainer: EzFavorites <windterm@local>
Homepage: https://github.com/EzFavorites/WindTerm
Description: Local-first desktop terminal workbench
 WindTerm is a native terminal workbench for local shell, SSH, Telnet, TCP,
 serial, SFTP, tunnels and Rhai scripting, built with Rust and egui.
CONTROL
    DEB_ART="$NAME-$VERSION-$PLATFORM.deb"
    dpkg-deb --build --root-owner-group "$DEB_ROOT" "$OUT/$DEB_ART" >/dev/null
    ARTIFACTS+=("$DEB_ART")
    ;;

  *-windows-*)
    PORT="$STAGE/portable"
    mkdir -p "$PORT"
    cp "$BIN" "$PORT/$NAME.exe"
    common_docs "$PORT"
    [ -f "$ROOT/assets/icon.ico" ] && cp "$ROOT/assets/icon.ico" "$PORT/$NAME.ico"
    ZIP_ART="$NAME-$VERSION-$PLATFORM.zip"
    zip_dir "$PORT" "$OUT/$ZIP_ART"
    ARTIFACTS+=("$ZIP_ART")

    # MSI：WiX v5，版本号只取数字部分（MSI 不接受 beta2 这类后缀）。
    command -v wix >/dev/null || { echo "Windows .msi 打包需要 WiX v5（wix 不在 PATH）" >&2; exit 1; }
    MSI_STAGE="$STAGE/msi"
    mkdir -p "$MSI_STAGE"
    cp "$BIN" "$MSI_STAGE/$NAME.exe"
    cp "$ROOT/LICENSE" "$MSI_STAGE/LICENSE"
    cp "$ROOT/README.md" "$MSI_STAGE/README.md"
    if [ -f "$ROOT/LICENSE" ]; then
      # WiX 的欢迎页需要一个 RTF 许可证；把 LICENSE 做最基本的 RTF 转义。
      {
        printf '{\\rtf1\\ansi\\deff0 {\\fonttbl{\\f0 Arial;}}\\f0\\fs18 '
        sed -e 's/\\/\\\\/g' -e 's/{/\\{/g' -e 's/}/\\}/g' "$ROOT/LICENSE" | tr '\n' ' ' | sed 's/$/\\par/'
        printf '}'
      } > "$MSI_STAGE/license.rtf"
    else
      : > "$MSI_STAGE/license.rtf"
    fi
    MSI_ART="$NAME-$VERSION-$PLATFORM.msi"
    NUM_VERSION="${VERSION%%-*}"
    wix build "$(winpath "$ROOT/packaging/windows/windterm.wxs")" -arch x64 \
      -ext WixToolset.UI.wixext -ext WixToolset.Util.wixext \
      -d "Version=$NUM_VERSION" \
      -d "BinDir=$(winpath "$MSI_STAGE")" \
      -d "IconPath=$(winpath "$ROOT/assets/icon.ico")" \
      -d "LicenseRtf=$(winpath "$MSI_STAGE/license.rtf")" \
      -o "$(winpath "$OUT/$MSI_ART")"
    rm -f "$OUT/${MSI_ART%.msi}.wixpdb"
    ARTIFACTS+=("$MSI_ART")
    ;;
esac

for a in "${ARTIFACTS[@]}"; do
  sha256_of "$a"
done

echo "打包完成，产物："
for a in "${ARTIFACTS[@]}"; do
  ls -lh "$OUT/$a"
  [ -f "$OUT/$a.sha256" ] && echo "  $(cat "$OUT/$a.sha256")"
done
