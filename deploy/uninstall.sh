#!/bin/sh
# RustCMS 卸载脚本（Linux）
#
# 用法：
#   sudo sh uninstall.sh               交互式：停服务 → 删单元 → 询问是否删安装目录
#   sudo sh uninstall.sh /opt/rustcms  直接指定安装目录
#   curl -fsSL <镜像>/uninstall.sh | sh
#                                     管道方式：提示从 /dev/tty 读，
#                                     没有终端时只停服务、不删目录（防误删）
#
# 只停服务并移除程序目录，不会动数据库里的数据。
#
# 可覆盖的环境变量：
#   RUSTCMS_SERVICE  systemd 服务名（默认 rustcms）
#   RUSTCMS_DIR      安装目录（默认取服务单元里的 WorkingDirectory）

set -eu

warn() { printf '[!] %s\n' "$*"; }
say()  { printf '%s\n' "$*"; }

if [ "$(id -u)" -ne 0 ]; then
  say "需要 root 权限，请用：sudo sh $0" >&2
  exit 1
fi

SERVICE="${RUSTCMS_SERVICE:-rustcms}"
DIR="${1:-${RUSTCMS_DIR:-}}"

# 交互输入走 fd 3：curl | sh 时 stdin 是脚本本身，直接 read 会吃掉脚本内容。
CAN_ASK=0
if exec 3</dev/tty 2>/dev/null; then CAN_ASK=1; fi

ask() {
  printf '%s' "$1"
  if [ "$CAN_ASK" -eq 1 ]; then
    IFS= read -r ANSWER <&3 || ANSWER=""
  else
    ANSWER=""
  fi
}

if [ -z "$DIR" ] && command -v systemctl >/dev/null 2>&1; then
  DIR=$(systemctl show -p WorkingDirectory --value "$SERVICE" 2>/dev/null | head -n 1 || true)
  [ "$DIR" = "/" ] && DIR=""
fi

# ---- 停服务 ----
if command -v systemctl >/dev/null 2>&1; then
  systemctl stop "$SERVICE" 2>/dev/null || true
  systemctl disable "$SERVICE" 2>/dev/null || true
  if [ -f "/etc/systemd/system/$SERVICE.service" ]; then
    rm -f "/etc/systemd/system/$SERVICE.service"
    systemctl daemon-reload 2>/dev/null || true
    say "已移除 systemd 服务：$SERVICE"
  else
    say "未找到 systemd 单元：$SERVICE（可能不是用 systemd 启动的）"
  fi
else
  pkill -x rustcms 2>/dev/null || true
  say "已尝试结束 rustcms 进程"
fi

# ---- 询问是否删安装目录 ----
if [ -z "$DIR" ]; then
  if [ "$CAN_ASK" -eq 1 ]; then
    ask "是否删除安装目录（含 config.json 与 public/upload 上传文件）？[y/N]: "
    case "${ANSWER:-}" in
      y|Y|yes|YES)
        ask "安装目录 [$(pwd)]: "
        DIR="${ANSWER:-$(pwd)}"
        ;;
      *) DIR="" ;;
    esac
  else
    warn "没有可交互的终端，跳过删除安装目录（要删除请带参数：sh uninstall.sh <目录>）"
  fi
fi

if [ -n "$DIR" ]; then
  [ -d "$DIR" ] || { say "目录不存在：$DIR" >&2; exit 1; }
  say "删除 $DIR ..."
  rm -rf "$DIR"
  say "安装目录已删除。"
fi

warn "数据库中的数据没有删除，如需清理请手动 DROP DATABASE。"
say "卸载完成。"
