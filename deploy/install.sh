#!/bin/sh
# RustCMS 安装脚本（Linux）
# 用法：在解压后的安装包目录中执行  sudo sh install.sh
#
# 做的事：
#   1. 交互收集数据库连接信息，写出 config.json（权限 600；server.port 默认 9001）
#   2. 给主程序加执行权限，创建 logs/ cache/ 运行时目录
#   3. （可选）注册 systemd 服务并启动
#
# 前置条件：目标数据库已存在并已导入站点表结构（RustCMS 会自动补缺失列，
# 但不会凭空建全部业务表；全新站点请先按部署文档初始化数据库）。
set -eu

cd "$(dirname "$0")" || exit 1
DIR="$(pwd)"

say()  { printf '%s\n' "$*"; }
warn() { printf '[!] %s\n' "$*"; }
ask()  { printf '%s' "$1"; IFS= read -r ANSWER || ANSWER=""; }
json_escape() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }

say "=============================================="
say "        RustCMS 安装 (Linux)"
say "=============================================="
say "安装目录：$DIR"

if [ ! -f rustcms ]; then
  say "当前目录找不到 rustcms 主程序，请在解压后的安装包目录执行本脚本" >&2
  exit 1
fi

ask "数据库主机 [默认 127.0.0.1]: "
DBHOST="${ANSWER:-127.0.0.1}"
ask "数据库端口 [默认 3306]: "
DBPORT="${ANSWER:-3306}"
case "$DBPORT" in ''|*[!0-9]*) say "端口必须是数字" >&2; exit 1 ;; esac
ask "数据库名 [默认 rustcms]: "
DBNAME="${ANSWER:-rustcms}"
ask "数据库用户名 [默认 root]: "
DBUSER="${ANSWER:-root}"
ask "数据库密码（可留空）: "
DBPASS="$ANSWER"

# config.json 不随升级覆盖；已存在时先备份再写
if [ -f config.json ]; then
  cp config.json "config.json.bak.$(date +%Y%m%d%H%M%S)"
  warn "已备份原有 config.json"
fi

cat > config.json <<EOF
{
  "mysql": {
    "database": "$(json_escape "$DBNAME")",
    "user": "$(json_escape "$DBUSER")",
    "password": "$(json_escape "$DBPASS")",
    "host": "$(json_escape "$DBHOST")",
    "port": $DBPORT,
    "use_default": false
  },
  "server": {
    "env": "production",
    "port": 9001,
    "log_level": "error"
  },
  "api_token": "",
  "template_dir": "./template/default",
  "public_dir": "./public"
}
EOF
chmod 600 config.json
chmod 755 rustcms
mkdir -p logs cache
say "==> config.json 已生成（server.port 默认 9001，可在文件中修改），rustcms 已加执行权限"

USE_SYSTEMD=0
if [ "$(id -u)" -eq 0 ] && command -v systemctl >/dev/null 2>&1; then
  ask "是否注册为 systemd 服务（rustcms）并立即启动？[Y/n]: "
  case "${ANSWER:-y}" in
    y|Y|yes|YES) USE_SYSTEMD=1 ;;
  esac
else
  warn "非 root 或没有 systemctl，跳过服务注册"
fi

if [ "$USE_SYSTEMD" -eq 1 ]; then
  UNIT="/etc/systemd/system/rustcms.service"
  if [ -f "$DIR/deploy/rustcms.service" ]; then
    sed -e "s|WorkingDirectory=.*|WorkingDirectory=$DIR|" \
        -e "s|ExecStart=.*|ExecStart=$DIR/rustcms|" \
        "$DIR/deploy/rustcms.service" > "$UNIT"
  else
    cat > "$UNIT" <<EOF
[Unit]
Description=RustCMS
After=network-online.target

[Service]
Type=simple
WorkingDirectory=$DIR
ExecStart=$DIR/rustcms
Restart=always
RestartSec=3

[Install]
WantedBy=multi-user.target
EOF
  fi
  systemctl daemon-reload
  systemctl enable rustcms >/dev/null 2>&1 || true
  systemctl restart rustcms
  say "==> 已启动 systemd 服务 rustcms（systemctl status rustcms 查看状态）"
else
  if command -v pgrep >/dev/null 2>&1 && pgrep -x rustcms >/dev/null 2>&1; then
    warn "检测到已有 rustcms 进程在运行，请先停止后再手动启动"
  else
    nohup "$DIR/rustcms" > "$DIR/logs/server.log" 2>&1 &
    say "==> 已后台启动，日志：$DIR/logs/server.log"
  fi
fi

say ""
say "安装完成。访问 http://127.0.0.1:9001/system/ 进入后台（端口可在 config.json 的 server.port 修改）。"
say "卸载：sudo sh $DIR/uninstall.sh"
