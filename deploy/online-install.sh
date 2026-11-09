#!/bin/sh
# RustCMS 在线安装（一行命令安装，release 包里生成一份 install.sh）。
#
#   curl -fsSL https://<镜像>/install.sh | sh
#
# 做的事：读同目录的 version.json → 按架构选安装包 → 校验 sha256 →
# 解包 → 调用包内 install.sh（交互式：数据库配置、systemd）。
#
# 可覆盖的环境变量：
#   RUSTCMS_MIRROR   镜像地址（默认编译时写死在 DEFAULT_MIRROR）
#   RUSTCMS_VERSION  指定版本（默认读 version.json）
#   RUSTCMS_ARCH     amd64（默认按 uname -m 推断）
set -eu

DEFAULT_MIRROR="__DEFAULT_MIRROR__"
MIRROR="${RUSTCMS_MIRROR:-$DEFAULT_MIRROR}"
MIRROR="${MIRROR%/}"

die() { printf '%s\n' "错误：$*" >&2; exit 1; }
say() { printf '%s\n' "$*"; }

command -v curl >/dev/null 2>&1 || die "需要 curl，请先安装"
command -v tar  >/dev/null 2>&1 || die "需要 tar"

ARCH="${RUSTCMS_ARCH:-}"
if [ -z "$ARCH" ]; then
	case "$(uname -m)" in
		x86_64|amd64) ARCH=amd64 ;;
		aarch64|arm64) ARCH=arm64 ;;
		*) die "不支持的 CPU 架构：$(uname -m)（可用 RUSTCMS_ARCH 指定）" ;;
	esac
fi

WORK="$(mktemp -d /tmp/rustcms-install.XXXXXX)"
trap 'rm -rf "$WORK"' EXIT INT TERM

say "==> 读取发布信息：$MIRROR/version.json"
curl -fsSL "$MIRROR/version.json" -o "$WORK/version.json" || die "下载 version.json 失败，检查镜像地址：$MIRROR"

# 机器上不一定有 jq：把 JSON 压成一行再用 sed 取扁平字段。
COMPACT="$(tr -d ' \t\r\n' < "$WORK/version.json")"
json_str() {
	printf '%s' "$COMPACT" | sed -n 's/.*"'"$1"'":"\([^"]*\)".*/\1/p' | head -n 1
}

VER="${RUSTCMS_VERSION:-$(json_str version)}"
[ -n "$VER" ] || die "version.json 里没有 version 字段"

PKG="$(json_str "linux_${ARCH}_file")"
[ -n "$PKG" ] || die "该版本清单未提供 ${ARCH} 架构的 Linux 安装包（linux_${ARCH}_file 为空）；请换用 Windows 安装包或在具备交叉编译环境的机器上重新发版"
URL="$PKG"
case "$URL" in http://*|https://*) ;; *) URL="$MIRROR/$PKG" ;; esac

say "==> 版本 $VER，架构 $ARCH"
say "==> 下载安装包：$URL"
curl -fL --progress-bar "$URL" -o "$WORK/$PKG" || die "下载安装包失败：$URL"

SUM="$(json_str "linux_${ARCH}_sha256")"
if [ -n "$SUM" ]; then
	if command -v sha256sum >/dev/null 2>&1; then
		ACT="$(sha256sum "$WORK/$PKG" | awk '{print $1}')"
	elif command -v shasum >/dev/null 2>&1; then
		ACT="$(shasum -a 256 "$WORK/$PKG" | awk '{print $1}')"
	else
		ACT=""
		say "!! 找不到 sha256sum / shasum，跳过校验"
	fi
	if [ -n "$ACT" ]; then
		[ "$ACT" = "$SUM" ] || die "安装包校验失败（期望 $SUM，实际 $ACT）"
		say "==> 校验通过"
	fi
else
	say "!! version.json 没有 sha256，跳过校验"
fi

say "==> 解包"
tar -xzf "$WORK/$PKG" -C "$WORK" || die "解包失败（包可能没下全）"
DIR="$(find "$WORK" -maxdepth 1 -mindepth 1 -type d | head -n 1)"
[ -n "$DIR" ] && [ -f "$DIR/install.sh" ] || die "安装包里没有 install.sh"

say "==> 运行安装程序"
cd "$DIR"
sh ./install.sh
