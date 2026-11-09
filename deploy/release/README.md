# 发行脚本

`pack-release.ps1` 是 RustCMS 唯一的一键发版入口，负责编译、组装、归档与生成清单。

```powershell
# 在仓库根目录执行（默认版本取 Cargo.toml 的 version）
powershell -NoProfile -ExecutionPolicy Bypass -File deploy\release\pack-release.ps1

# 指定版本 / 镜像 / 跳过 Linux 交叉编译
powershell -NoProfile -ExecutionPolicy Bypass -File deploy\release\pack-release.ps1 `
  -Version 0.2.0 -Mirror https://mirror.example.com/rustcms -SkipLinux
```

完整参数、产物清单与升级协议见 [`../upgrade.md`](../upgrade.md)。

## 发版检查清单

1. `Cargo.toml` 的 `version` 已递增（脚本会拒绝重打同一版本，除非 `-Force`）。
2. `cargo build --release --offline` 成功；条件允许时再跑
   `cargo check --offline --all-targets` 与 `cargo test --offline --bins`。
   ⚠️ 低内存机器（物理 16GB、系统提交内存接近页面文件上限）编译 test harness 会因
   `os error 1455` / `STATUS_STACK_BUFFER_OVERRUN` 失败，此时用 `-j 1` 单独编译 bin，
   或先释放内存再跑测试；这是环境限制，不是代码错误。
3. 跑完脚本后核对：
   - `SHA256SUMS.txt` 每个条目与 `Get-FileHash` 一致；
   - zip 内路径全是正斜杠（脚本用 .NET `ZipArchive` 逐条写入，天然满足）；
   - `tar -tvzf rustcms-v<ver>-linux-amd64.tar.gz` 里 `rustcms`、`install.sh`、
     `uninstall.sh`、`deploy/*.sh` 是 `-rwxr-xr-x`；
   - `version.json` 的 `packages.*.checksum` 与对应升级包哈希一致；
   - 升级包内带 `version.json`，且不含 `config.json` / `logs/` / `cache/`。
4. 把 `version.json`、`install.sh`、`uninstall.sh` 与全部包上传到静态目录，
   站点后台把 `settings.upgrade.version_url` 指到该地址即可检查更新。

## 版本号数据源

- 程序内版本：`Cargo.toml` → `env!("CARGO_PKG_VERSION")` → `statistic::VERSION`。
- 发行清单版本：`deploy/release/version.json`（由脚本按 `Cargo.toml` 生成）。
- 站点实际版本：升级成功后写到程序目录的 `version.json`，后台 `version/info` 优先读它。

三者正常情况下一致；手工改过站点 `version.json` 时，后台显示的是站点实际版本。
