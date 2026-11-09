# RustCMS 编译、发行与在线升级

后台「系统升级」页支持两种升级方式：**上传升级包**（服务器不出网也能升）与
**远程在线升级**（把 `version.json` 与升级包放到任意静态目录）。
升级内容是「主程序 + 模板 + 静态资源」，由服务端解压覆盖，并自动写出站点 `version.json`。

## 1. 一键编译发行

在仓库根目录执行：

```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File deploy\release\pack-release.ps1
```

| 参数 | 说明 |
|---|---|
| `-Version 0.2.0` | 指定版本；缺省读 `Cargo.toml` 的 `version` |
| `-Mirror https://...` | 写进在线安装脚本的镜像地址 |
| `-SkipLinux` | 跳过 Linux 交叉编译与 Linux 产物（没有 `x86_64-unknown-linux-gnu` linker 时用） |
| `-SkipBuild` | 复用 `target\` 下已有二进制，不重新编译 |
| `-OutDir D:\publish` | 换输出目录（默认 `E:\CodeBuddy\rustcms发布`） |
| `-Force` | 允许覆盖同版本产物 |

脚本依次完成：`cargo build --release --offline`（Windows）与
`cargo build --release --offline --target x86_64-unknown-linux-gnu`（Linux）
→ 组装安装包目录与升级包目录 → 归档 → 生成 `version.json` 与 `SHA256SUMS.txt`
→ 输出 `install.sh` / `uninstall.sh`。

归档不依赖 `Compress-Archive`（PS 5.1 会写反斜杠路径）与系统 `tar.exe`
（Windows 打出来全是 0644）：zip 用 .NET `ZipArchive` 逐条写入，
tar.gz 手写 ustar 头 + `GZipStream`，主程序与 `*.sh` 写 0755。

### 产物（`E:\CodeBuddy\rustcms发布`）

| 文件 | 用途 |
|---|---|
| `rustcms-v<ver>-windows-upgrade.zip` | Windows 升级包 |
| `rustcms-v<ver>-linux-upgrade.zip` | Linux 升级包 |
| `rustcms-v<ver>.zip` | 兼容旧面板的单包（内容 = Windows 升级包） |
| `rustcms-v<ver>-windows.zip` | Windows 完整安装包（解压即用） |
| `rustcms-v<ver>-linux-amd64.tar.gz` | Linux x86_64 完整安装包（install.sh + systemd） |
| `rustcms-install.sql` | 全新安装用数据库脚本（结构 + 演示数据，默认库名 `rustcms`） |
| `version.json` | 远程升级 + 在线安装共用清单 |
| `SHA256SUMS.txt` | 校验清单，格式 `sha256  文件名  字节数` |
| `install.sh` / `uninstall.sh` | 在线一键安装 / 卸载 |

`deploy/release/` 下同时留一份升级包与 `version.json`，方便内网直接取用。

### 升级包结构

```
rustcms.exe / rustcms     # 主程序
template/                 # 模板（整体替换）
public/                   # 静态资源（整体替换，public/upload 除外）
version.json              # 版本号 + SQL 清单
```

不要放 `config.json`、`logs/`、`cache/`、`data/`；这些是站点自有数据，升级不会覆盖。

## 2. version.json 格式

```json
{
  "name": "rustcms",
  "version": "0.1.0",
  "date": "2026-11-09",
  "note": "本次更新说明",
  "sql": [],
  "url": "rustcms-v0.1.0.zip",
  "checksum": "sha256:...",
  "packages": {
    "windows": { "url": "rustcms-v0.1.0-windows-upgrade.zip", "checksum": "sha256:..." },
    "linux":   { "url": "rustcms-v0.1.0-linux-upgrade.zip",   "checksum": "sha256:..." }
  },
  "install": {
    "windows_file": "rustcms-v0.1.0-windows.zip",
    "windows_sha256": "...",
    "linux_amd64_file": "rustcms-v0.1.0-linux-amd64.tar.gz",
    "linux_amd64_sha256": "..."
  }
}
```

| 字段 | 说明 |
|---|---|
| `version` | 目标版本号，升级后写进站点目录的 `version.json` |
| `sql` | 需要执行的 SQL（相对包根）；失败只记日志不阻断升级 |
| `packages` | 按平台给包；后台按 `windows` / `linux` 取，取不到退回顶层 `url` |
| `checksum` | `sha256:<hex>`；有值时服务端下载/上传后强制校验 |
| `install.*` | 在线安装脚本读取的扁平键（机器上不一定有 jq） |

`url` 支持相对路径，相对 `version.json` 所在地址解析。

## 3. 后台配置

| 配置 | 说明 | 未配置时 |
|---|---|---|
| `settings.upgrade.version_url` / `RUSTCMS_VERSION_URL` | 远程 `version.json` 完整地址 | 「检查更新」恒回「当前已是最新版本」，不外呼 |
| `settings.upgrade.package_base` / `RUSTCMS_UPGRADE_BASE` | 升级包下载基址（无清单时的旧格式回退） |「立即升级」明确回「未配置在线升级源」 |
| `settings.upgrade.auto_restart` | 升级后是否自动执行自升级脚本重启服务 | 只生成脚本，提示手工重启 |

独立部署默认零外呼：不填任何地址时，云端/版本检查/在线下载都不会发请求，
**上传本地升级包**始终可用。

## 4. 升级方式一：上传升级包

1. 后台「系统升级」→「上传升级包」卡片。
2. 选择 `rustcms-v<ver>-{windows|linux}-upgrade.zip`（也支持 `.tar.gz`），
   包内 `version.json` 会自动给出版本号；包内没有清单时可手填版本号或勾选强制升级。
3. 可选填 `checksum=sha256:...` 做完整性校验。
4. 提交后服务端解压覆盖；返回写入/跳过文件数与 SQL 清单。

## 5. 升级方式二：远程在线升级

1. 把 `version.json` 与升级包放到同一静态目录。
2. 后台配置 `settings.upgrade.version_url` 指向该 `version.json`。
3. 「检查更新」返回版本、说明、按当前平台挑好的包地址与校验和；
   「立即升级」下载 → 校验 sha256 → 覆盖 → 重启。

## 6. 覆盖规则与回滚

会被替换：主程序、`template/`、`public/` 下的程序资源、`version.json`。
不会覆盖：`config.json`、`logs/`、`cache/`、`data/`、`uploads/`、
`public/upload/`、`public/uploads/`、`*.sh` / `*.bat` / `*.ps1` 脚本。

主程序先写 `<exe>.new`，全包解压成功后再备份旧程序（`<exe>.old`）并替换。
替换失败时 `.new` 会留在目录里，自升级脚本会在服务停止后再次尝试。

回滚：停服务 → 把 `<exe>.old` 改回 `<exe>` → 启动。数据库变更需另用备份回滚。

## 7. 常见问题

**Q：点了升级没反应？**
A：先看响应里的「写入 N 个文件」，再看服务日志。版本不高于当前版本时会被拒绝，
需要勾选「强制升级」。

**Q：提示「校验失败」？**
A：`version.json` 里的 checksum 与包不一致。重新下载或用 `Get-FileHash` 核对后更新清单。

**Q：升级后页面还是旧样式？**
A：升级会写站点 `version.json` 但不会自动清模板缓存，重启服务并强刷浏览器（Ctrl+F5）。

**Q：Linux 交叉编译失败？**
A：需要 `x86_64-unknown-linux-gnu` target 与 `x86_64-linux-gnu-gcc` linker（ring 等
依赖的 C 代码由 `cc-rs` 编译，只有 `rust-lld` 不够）。缺 linker 时脚本会打印
warning 并自动跳过 Linux 产物，`version.json` 里不会出现 `packages.linux` 与
`install.linux_*`，`SHA256SUMS.txt` 也会注明未构建；也可显式加 `-SkipLinux`。
Linux 包需在 Linux 机器上 `cargo build --release` 后重新发版。

---

## 8. 本机实跑记录（2026-11-09，v0.1.0）

| 项 | 结果 |
|---|---|
| Windows `cargo build --release --offline -j 1` | 成功，`rustcms.exe` 35,651,932 B；19m13s |
| Linux 交叉编译 | 跳过（缺 `x86_64-linux-gnu-gcc`），脚本按设计降级 |
| 产物 | 3 个 zip（升级包 / 兼容单包 / Windows 完整安装包）、`version.json`、`SHA256SUMS.txt`、`install.sh`、`uninstall.sh` |
| SHA256SUMS | 3/3 与实测哈希一致 |
| `version.json` | UTF-8 合法，`packages.windows.checksum` / `install.windows_sha256` / 顶层 `checksum` 三处交叉核对一致 |
| zip 结构 | 576 / 576 / 583 条，反斜杠路径 0，`testzip()` 无损坏，不含 `config.json` / `logs/` / `cache/` |
| 包内主程序 | 含 `deployed_version`、`upgrade_finish_message`、`version/upload-upgrade`、`RUSTCMS_VERSION_URL` |
| 文件编码 | `install.sh` / `uninstall.sh` 无 BOM、CR=0；`使用说明.txt` 带 BOM；两个 `.bat` 纯 ASCII |
| tar.gz 生成器 | 用 Python `tarfile` 验证 ustar 头：typeflag `'0'`、二进制/`*.sh` 0755、其余 0644、内容完整 |

⚠️ 本次未跑成 `cargo test`：机器物理内存 16GB、系统提交内存已达 22.4/23.8GB，
编译 test harness 时 rustc 连续报 `os error 1455` 与 `STATUS_STACK_BUFFER_OVERRUN`
（`E0786: failed to mmap` / `memory allocation failed`）。这是页面文件耗尽的环境限制，
不是代码错误 —— 同一份源码的 release bin 已完整编译、链接成功。需要跑测试时先
释放内存或扩大页面文件，再用 `-j 1` 执行。
