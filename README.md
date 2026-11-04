# rustcms

基于 **Rust** 的内容管理系统（CMS）。

服务端使用 Axum + Tokio，数据库访问使用 sqlx（MySQL），模板渲染使用 Tera，
鉴权使用 JWT + bcrypt。整体为「后台管理 API + 前台页面/接口」的经典 CMS 形态，
把内容、分类、模块、会员、交易、互动、设置等常见站点能力收敛成一套统一的接口约定。

- 语言：Rust 2021
- Web 框架：[Axum 0.7](https://github.com/tokio-rs/axum) + Tokio
- 数据库：[sqlx](https://github.com/launchbadge/sqlx) / MySQL
- 模板引擎：[Tera](https://keats.github.io/tera/)
- 鉴权：JWT（jsonwebtoken）+ bcrypt 口令散列
- 许可证：AGPL-3.0（见 [LICENSE](./LICENSE)）

---

## 目录结构

```
src/
├── main.rs              启动入口：命令行选项 → 读 config.json → 连库 → 起服务
├── config.rs            配置文件读写
├── db.rs                数据库连接池
├── state.rs             应用状态（配置 + 连接池）
├── server.rs            HTTP 服务装配与中间件（CORS / 安全头 / UTF-8 / Trace）
├── routes.rs            全部路由与处理器（含集成测试）
├── auth.rs              JWT 签发校验、bcrypt 口令散列
├── login_guard.rs       后台与会员端登录态守卫
├── middleware.rs        通用中间件
├── models/              数据访问层（sqlx + FromRow + ModelError）
├── template/            模板渲染与视图辅助
├── front.rs             前台页面路由
├── api_content.rs       内容 / 分类 / 模块 等公开接口
├── api_member.rs        会员端接口
├── api_trade.rs         交易 / 订单 / 支付接口
├── api_interact.rs      评论 / 互动类接口
└── api_admin_gap.rs     后台补充接口
docs/
└── coverage-plan.md     后台端点覆盖盘点与并行任务划分
```

---

## 快速开始

### 1. 环境要求

- Rust stable（`rustup update stable`）
- MySQL 5.7+ / 8.0

### 2. 准备配置

```bash
cp config.sample.json config.json
```

`config.json` 字段：

```json
{
  "mysql": {
    "database": "cms",
    "user": "root",
    "password": "",
    "host": "127.0.0.1",
    "port": 3306,
    "use_default": false
  },
  "server": {
    "env": "production",
    "port": 8001,
    "log_level": "error"
  },
  "api_token": "",
  "template_dir": "../template/default",
  "public_dir": "../public"
}
```

| 字段 | 说明 |
|---|---|
| `mysql.*` | 数据库连接信息；`use_default: true` 表示复用默认站点（ID=1）的账号密码 |
| `server.port` | 监听端口，默认 `8001` |
| `server.log_level` | `error` / `warn` / `info` / `debug` / `trace`；也可用环境变量 `RUST_LOG` 覆盖 |
| `api_token` | 写入接口令牌；未配置时 `/api/archives` 的 POST/PUT/DELETE 一律拒绝 |
| `template_dir` | 模板目录（相对工作目录或绝对路径） |
| `public_dir` | 静态文件目录，上传落盘在这里 |

配置文件查找顺序：`RUSTCMS_CONFIG` 环境变量 → 可执行文件同目录 → 当前工作目录。

### 3. 编译与运行

```bash
cargo build --release
cargo run --release
```

内存吃紧时降低并发度：

```bash
CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 GOGC=20 cargo build --release
```

### 4. 生成后台管理员口令散列

```bash
cargo run --release -- --hash-password '你的密码'
```
---

## 接口一览

| 前缀 | 说明 |
|---|---|
| `/api/*` | 对外公开接口（内容、分类、模块、会员、交易、互动） |
| `/system/api/*` | 后台管理接口（管理员、用户组、菜单、日志、设置等） |

响应统一信封：

```json
{ "code": 0, "msg": "", "data": {} }
```

`code = 0` 表示成功，`code = 1` 表示业务失败；`msg` 为可直接展示的中文文案。
未在对外契约中出现的字段（如 `password`）不参与序列化。

---

## 测试

```bash
cargo test --offline
```

数据库相关的用例连不上库时会自动跳过（返回 `None`），不会让 `cargo test` 变红。
用例串行执行（`db_lock()`），避免并发写同一个库互相干扰。

---

## 开发文档

[`docs/coverage-plan.md`](./docs/coverage-plan.md) 记录了后台端点的覆盖盘点、分批实现方式，
以及端点覆盖度的机器化核对方法（路径级 + 方法级）。

---

## 许可

AGPL-3.0，完整文本见 [LICENSE](./LICENSE)。