# rustcms 后台端点覆盖盘点与并行任务划分

> 本文是 rustcms 的内部工程记录：记录后台端点的覆盖盘点、分批实现方式与端点覆盖度的核对方法。

## 一、现状盘点（还有多少没完成）

### Go 源工作量（`controller/manageController/`）
- 文件：68 个 `.go`（其中 5 个是 `*_test.go`），**实际需移植 63 个非测试文件**。
- 函数：452 个 `func`（含 13 个测试函数），**非测试函数约 439 个**。
- 行数：约 19,874 行。

### rustcms 已完成部分（`rustcms/src/`）
| 领域 | Go 源 | 状态 |
|---|---|---|
| 管理员 / 用户组 / 登录·操作日志 | `admin.go` | ✅ 已完整（`models/admin.rs` + routes 的 `admin_*` 处理器） |
| 后台菜单 | `menu` | ✅ 已完整（`models/menu.rs`，含多站点分组测试） |
| 分类 | `category.go` | ⚠️ 只做了 `list/detail`，其余未做 |
| 模块 | `module.go` | ⚠️ 只做了 `list`，其余未做 |
| 内容/文档 | `archive.go` | ⚠️ 只做了基础 CRUD，批量/回收站/参数等未做 |
| 其余 59 个文件 | — | ❌ 全部未移植 |

### 结论
- **已完成：约 17 个函数（admin）+ 菜单 + 分类/模块/文档的基础 CRUD。**
- **未完成：约 422 个函数、62 个文件、约 1.9 万行。**

---

## 二、Rust 移植通用约定（5 份任务都必须遵守）

1. **分层**：数据访问放 `rustcms/src/models/<领域>.rs`（sqlx + `FromRow` + `ModelError`），
   HTTP 处理器放 `rustcms/src/routes.rs`（axum handler，返回 `{"code":0,"msg":"","data":...}`）。
2. **错误**：业务校验用 `ModelError::invalid("中文文案")`，DB 错误用 `?` 自动转 `ModelError::Db`。
3. **响应信封**：成功 `json!({"code":0,"msg":"","data":...})`；失败 `fail(msg)`（`code=1`）。
   复用 `routes.rs` 里已有的 `ok/ok_msg/fail/failed` 辅助函数。
4. **鉴权**：后台接口用 `require_admin(...)`（已在 routes.rs），先鉴权再取 `state.db()`。
5. **序列化对齐**：字段名与 Go 版 API 完全一致（下划线），`json:"-"` 的字段（如 `password`、
   `backend`）用 `#[serde(skip_serializing)]`。
6. **测试**：每个模型文件带 `#[cfg(test)]` 用例，串行用 `db_lock()`，连不上库 `test_pool()` 返回
   `None` 即跳过；`cargo test` 不能红。
7. **编译**：`cargo build` 与 `cargo test` 必须全绿；低内存用 `$env:CARGO_BUILD_JOBS=1`。

---

## 三、5 份任务划分（按主题 + 大致函数数均衡）

| 分区 | 主题 | Go 文件 | 函数数 | 文件数 |
|---|---|---|---|---|
| **Part 1** | 内容体系（内容/分类/模块/附件/导航） | archive, category, module, attachment, nav, diy, collector, skill, common | ~69 | 9 |
| **Part 2** | 设置 / 设计 / 站点 / 升级 | design, setting, website, anqi, statistics, userAuth, upgrade | ~95 | 7 |
| **Part 3** | SEO / 收录 / 链接 插件 | Anchor, Sitemap, Robots, Link, Keyword, Tag, JsonLd, Google, Transfer, Timefactor, TitleImage, MultiLang, Redirect, Replace, Rewrite | ~70 | 15 |
| **Part 4** | 内容安全 / 互动 / 增强 插件 | Comment, Guestbook, Subscriber, Interference, sensitiveWords, Limiter, Akismet, User, Watermark, FileUpload, Material, HtmlCache, Fulltext, Importapi, AiGenerate | ~87 | 15 |
| **Part 5** | 商业 / 交易 / 集成 | Order, Pay, Commission, Withdraw, Finance, Retailer, Wechat, Weapp, Push, Storage, Backup, Sendmail, aiChat, LLMs, Translate, Place | ~105 | 16 |

---

## 四、各分区详细文件清单

### Part 1 — 内容体系（9 文件）
`archive.go`(19) `category.go`(5) `module.go`(5) `attachment.go`(9) `nav.go`(6)
`diy.go`(4) `collector.go`(7) `skill.go`(5) `common.go`(9)

### Part 2 — 设置 / 设计 / 站点 / 升级（7 文件）
`design.go`(24) `setting.go`(23) `website.go`(6) `anqi.go`(17) `statistics.go`(6)
`userAuth.go`(3) `upgrade.go`(16)

### Part 3 — SEO / 收录 / 链接 插件（15 文件）
`pluginAnchor.go`(10) `pluginSitemap.go`(3) `pluginRobots.go`(2) `pluginLink.go`(4)
`pluginKeyword.go`(7) `pluginTag.go`(6) `pluginJsonLd.go`(2) `pluginGoogle.go`(2)
`pluginTransfer.go`(5) `pluginTimefactor.go`(2) `pluginTitleImage.go`(5)
`pluginMultiLang.go`(11) `pluginRedirect.go`(4) `pluginReplace.go`(1) `pluginRewrite.go`(2)

### Part 4 — 内容安全 / 互动 / 增强 插件（15 文件）
`pluginComment.go`(5) `pluginGuestbook.go`(7) `pluginSubscriber.go`(9)
`pluginInterference.go`(2) `sensitiveWords.go`(4) `pluginLimiter.go`(4) `pluginAkismet.go`(2)
`pluginUser.go`(12) `pluginWatermark.go`(5) `pluginFileUpload.go`(3) `pluginMaterial.go`(9)
`pluginHtmlCache.go`(13) `pluginFulltext.go`(4) `pluginImportapi.go`(2) `pluginAiGenerate.go`(6)

### Part 5 — 商业 / 交易 / 集成（16 文件）
`pluginOrder.go`(11) `pluginPay.go`(6) `pluginCommission.go`(2) `pluginWithdraw.go`(5)
`pluginFinance.go`(2) `pluginRetailer.go`(5) `pluginWechat.go`(12) `pluginWeapp.go`(2)
`pluginPush.go`(4) `pluginStorage.go`(3) `pluginBackup.go`(9) `pluginSendmail.go`(8)
`aiChat.go`(20) `pluginLLMs.go`(4) `pluginTranslate.go`(6) `pluginPlace.go`(6)

---

## 五、验收标准（每个分区）

1. `cargo build` 通过；`cargo test` 全绿（连库的用例通过 `test_pool()` 自动跳过）。
2. 每个移植的动作都有对应的模型函数 + 路由处理器 + 测试。
3. 响应 JSON 字段名与 Go 版逐字段一致；业务错误中文文案一致。
4. 无 `unimplemented!()` / `todo!()` 残留；无死代码告警（模型层可用 `#![allow(dead_code)]` 过渡）。

## 六、依赖顺序建议

- Part 1 是基础（`common.go` 的公共辅助、`archive/category/module` 的模型被很多插件复用），**优先落地**。
- Part 2 的 `setting.go`（全站设置）与 `design.go`（模板设计）体量最大、联动最多。
- Part 3/4/5 的插件多为独立配置页 + 保存逻辑，可真正并行，但都依赖 Part 1 的公共辅助与内容模型。

---

## 七、关键架构结论（5 个并行探查 agent 产出，务必遵守）

1. **路由前缀**：Go 后台接口真实挂载在 **`/system/api/...`** 下（`route/manage.go`：`system.Party("/api")`），
   **不是** `/api/...`。Rust 现有前缀是 `/api/...`，移植时统一映射。插件类接口形如 `/system/api/plugin/<plugin>/...`。
2. **鉴权中间件**：绝大多数业务路由挂 `ParseAdminToken + AdminPermission`（Rust 对应 `require_admin`，权限暂按 `group_id==1` 超管兜底）；
   `captcha` 只挂 `ParseAdminUrl`；`version/*`、`anqi/*` 只挂 `ParseAdminToken`。
3. **配置存储**：几乎全部插件配置存 **`settings` 表**（`key VARCHAR(190) PK` + `value LONGTEXT` 存 JSON 字符串），
   **没有独立 config 表**。Rust 需新增 `src/models/setting.rs`（`get_setting_value(key)` / `save_setting_value(key, json)`）。
   - 例外：`robots.txt` 存文件 `public/robots.txt`；sitemap 更新时间存文件 `cache/sitemap-time.log`；多语言 HTML 缓存存文件 `cache/multiLang/`。
4. **业务逻辑在 `provider/*.go`**：Go 控制器只是「参数绑定 + 信封」，真正的实体逻辑在 `provider.Website` 层。
   移植到 Rust 时对应落到 `models/*.rs`，`routes.rs` 里只做路由 + 校验 + 信封。
5. **部分更新语义**：大量 `*ConfigForm` 保存用「读现配置为基底 + `library.MergeJSONPayload` 只覆盖请求里出现的字段」。
   Rust 侧需在 `serde_json::Value` 层做同样的字段级 merge，不能用整块覆盖。
6. **列表响应**：分页接口顶层额外带 `"total":N`（**不是**放在 data 里）。
7. **AI 接口特例**（`aiChat.go` 等）：成功/失败用 `code:0`/`code:-1`，与普通插件 `0/1` 不同。
8. **金额单位**：一律 `int64`（分）。
9. **每个写动作记操作日志**：复用已有的 `models/admin.rs::record_action`。

## 八、已探查出的依赖（横切）

- 全部分区都依赖：`require_admin`、`ok/ok_msg/fail/failed`（已在 routes.rs）、`models/setting.rs`（待建）、`admin::record_action`。
- Part 1 的 `common.go`（验证码/版本/统计摘要/静态后台壳）与 `archive/category/module` 模型是其余 4 个分区的公共底座。

---

## 九、进度（2026-10-31）

### Part 1 内容体系

| Go 文件 | 状态 | 说明 |
|---|---|---|
| `common.go` | ✅ 大部分 | `version/info`、`statistic/summary`、`statistic/dashboard`、`captcha` 已实现；`version/check`、`version/upgrade` 未做（需外呼 + 覆盖二进制） |
| `category.go` | ✅ | `list` / `detail` / `save` / `delete` / `count` 全实现（模板默认值推导未做） |
| `module.go` | ✅ | `list` / `detail` / `save` / `delete` / `field-delete` 全实现 |
| `nav.go` | ✅ | 导航 + 导航分组 的列表 / 保存 / 删除 |
| `archive.go` | ✅ 大部分 | `/system/api/archive/*`：list / detail / save / delete（回收站）/ recover / status / recommend / sort / parent / category / time。未做：快速导入（Excel/分片）、删除单图、标签批量、定时发布计划 |
| `attachment.go` | ✅ | list / detail / upload（multipart + MD5 去重 + 图片宽高）/ delete / edit / category / scan。未做：远程 URL 导入（需 HTTP 客户端）、WebP 批量转换、缩略图生成 |
| `diy.go` | ✅ | types / page / save / reset（`diy_pages` 表，8 种区块类型，页面 key 白名单） |
| `collector.go` | ⚠️ 部分 | 配置读/存（`settings.collector`）、内容替换（SQL `REPLACE`）、关键词落库已实现；**真正抓取 / 问答组合 / 关键词拓展未实现**（需 HTTP 抓取与 AI 外呼），接口明确返回未实现提示 |
| `skill.go` | ✅ | list / detail / edit / delete / reload（文件系统 `data/skills/<name>/SKILL.md`，frontmatter 解析） |

新增基础设施：
- `src/models/setting.rs`：`settings` 表读写 + 字段级 merge（**所有插件配置的公共底座**）。
- `src/models/statistic.rs`：统计摘要 / 仪表盘 / 版本信息。
- `src/models/nav.rs`：导航与导航分组。
- `src/models/attachment.rs`：附件（MD5 去重、PNG/GIF/JPEG/WebP 宽高解析、显式列过滤）。
- `src/models/archive_admin.rs`：内容批量操作（状态 / 标记 / 排序 / 分类 / 父级 / 时间 / 回收站）。
- `src/models/diy.rs`：可视化装修（`diy_pages`，8 种区块类型）。
- `src/models/collector.rs`：采集配置（`settings.collector`）+ 关键词 + 内容替换。
- `src/models/skill.rs`：AI 技能（文件系统 + frontmatter 解析）。
- `src/models/category.rs` 扩展：`save` / `delete` / `update_archive_counts`。
- `src/models/module.rs` 扩展：`save` / `delete` / `delete_field`。
- `src/routes.rs` 新增 `manage_router()`，暴露 `/system/api/...`（与 Go 版路径对齐）。

**重要 schema 事实**：`archives` 只有 30 列、**没有 `status`**；`archive_drafts` 有 31 列（含 `status`）。
内容在两张表之间用 `REPLACE ... SELECT` 搬运（`archive_admin.rs` 的 `ARCHIVE_COLUMNS` / `DRAFT_COLUMNS`）。

验证：`cargo build` 无告警；`cargo test` **106 项全绿**。

### 下一步（Part 2 收尾 → Part 3）
Part 2 只剩 `anqi.go`（云端服务外呼，需网络，最后做）。其余 6 个文件已完成。
接下来进入 **Part 3（SEO / 收录 / 链接插件）**，建议顺序（先纯配置、后生成文件）：
1. `pluginRewrite.go`（伪静态规则，纯配置）
2. `pluginRobots.go` / `pluginSitemap.go`（写 `public/robots.txt`、`cache/sitemap-time.log`）
3. `pluginLink.go` / `pluginKeyword.go` / `pluginTag.go`（独立表 CRUD）
4. `pluginAnchor.go` / `pluginReplace.go` / `pluginRedirect.go` / `pluginTimefactor.go`
5. `pluginJsonLd.go` / `pluginGoogle.go` / `pluginMultiLang.go` / `pluginTransfer.go`
6. `pluginTitleImage.go`（需图片库，最后）

Part 1 仍遗留的边角：内容快速导入（Excel/分片）、标签批量、定时发布计划、附件远程 URL 导入、附件缩略图生成。

---

## 十、Part 2 进度（2026-11-01）

| Go 文件 | 状态 | 说明 |
|---|---|---|
| `statistics.go` | ✅ | `statistic/spider`、`traffic`（`statistic_logs` 的 JSON 列）、`dates`（`data/statistic/*.log` 文件名）、`detail`（按行解析日志、按 `type` 过滤、倒序分页）、`include` / `include/detail`（`spider_includes`） |
| `setting.go` | ✅ | 23 个动作：`system` / `content` / `index` / `contact` / `safe` / `ai` 读写（字段级 merge）、`cache`（读清理时间 + 切换 `cache_type`）、`diyfield`（数组整份覆盖）、`favicon` 上传/删除、`banner` 增改删。未做：缩略图重建、`AutoMigrateDB` |
| `userAuth.go` | ✅ | `plugin/userauth/{list,detail,audit}`（`user_auths` 表，`0/1/-1` 状态，驳回必填原因，记 `audit_time`/`audit_admin_id`） |
| `website.go` | ⚠️ 部分 | `website/{list,info,save,delete}` + `siteinfo` 已实现（`websites` 表 CRUD，站点 1 的 mysql 取 config.json 且抹掉密码）。**未实现**：`website/login`（子站切换登录）与真正的多站点数据隔离——Go 每站点一个独立 DB 连接池，Rust 当前是单池 |
| `upgrade.go` | ⚠️ 部分 | `version/env` 已实现（版本 / 平台 / 主程序名 / 路径 / 上传上限 / 跳过规则）。**未实现**：`version/check`、`version/upgrade`（需外呼）、`version/upload-upgrade`（需 zip/tar 解包 + 覆盖二进制） |
| `design.go` | ✅ | `design/list`、`info`、`save`、`use`、`delete`、`upload/check`；模板文件 `file/{info,save,copy,delete,upload,restore,histories,history/info,history/delete,templates}`；`docs`/`helpers` 返回空数组。未做：zip 打包下载 / 上传解压（无 zip 库）、演示数据备份/初始化（依赖备份模块） |
| `anqi.go` | ❌ | 待做（云端服务外呼，最后做） |

验证：`cargo build` 无告警；`cargo test` **133 项全绿**。

> **构建环境备忘**：本机只有约 1.4GB 空闲内存，`cargo test` 会因 rustc 提交内存不足而
> `memory allocation ... failed / STATUS_STACK_BUFFER_OVERRUN`。
> 解决：`CARGO_INCREMENTAL=0` + `--jobs 1`（关闭增量缓存可显著降低 rustc 峰值内存）。
> 已固化在 `rustcms/tmp/check.ps1` / `test.ps1`。

> **踩坑记录 1**：`replace_in_file` 曾两次报告「成功」但内容没写进去。根因是**同一个文件在同一个并行批次里被编辑了多次**——
> 第二次编辑基于旧快照回写，把第一次的改动覆盖掉了。
> 结论：**同一文件的多处修改必须串行**（一次一个 replace_in_file，或先合并成一次大替换），改完 `manage_router` 一定要 grep 复核。

---

## 十一、Part 3 进度（2026-11-02）

| Go 文件 | 状态 | 说明 |
|---|---|---|
| `pluginRewrite.go` | ✅ | `plugin/rewrite` 读 / 写（`settings.rewrite`，`{mode, patten}` 字段级 merge） |
| `pluginRobots.go` | ✅ | `plugin/robots` 读 / 写；内容存**文件** `public/robots.txt`，文件缺失时回默认内容 |
| `pluginSitemap.go` | ✅ | `plugin/sitemap` 读 / 写（`type` 归一为 `xml`/`txt`，切格式时自动清理旧文件）+ `plugin/sitemap/build` 真生成 `public/sitemap.{xml,txt}` 并写 `cache/sitemap-time.log`。简化：不分片（忽略 `page_size` 多文件）、不生成标签页 |
| `pluginLink.go` | ✅ | `plugin/link/{list,detail,delete}`（`links` 表，保存时 `status` 强制归零等回链检查）。`plugin/link/check` 明确未实现（需 HTTP 客户端） |
| `pluginKeyword.go` | ✅ | `plugin/keyword/{setting,list,detail,delete,export,import}`（`keywords` 表 + `settings.keyword`）：批量新增自动去重、单条更新查重、单/批量/全量删除、导出表头+数据、导入按行解析 `title[, category_id]` |
| `pluginTag.go` | ✅ | `plugin/tag/{list,detail,delete,fields}`（`tags` + `tag_contents` + `tag_data`）：标题模糊搜索 + 分类筛选、`partial` PATCH 语义、别名自动去重加后缀、首字母推导、正文/自定义字段 upsert、字段定义存 `settings.tag_fields` |
| `pluginRedirect.go` | ✅ | `plugin/redirect/{list,detail,delete,import}`（`redirects` 表，`from_url` 唯一）：地址自动补前导 `/`、来源≠目标校验、同来源自动复用记录、导入按行 `from, to` |
| `pluginAnchor.go` | ✅ | `plugin/anchor/{list,detail,delete,replace,export,import,setting,add-from-title}`（`anchors` 表，`title` 唯一）：关键词搜索、链接剥前台域名前缀、密度 <10 归一 100、批量替换把正文里**未被链接**的词包成 `<a>`、可从分类/内容/标签标题批量生成锚文本 |
| `pluginReplace.go` | ✅ | `plugin/replace/values`：按 14 个「位置」批量替换；正文字段用 Rust 扫描器**尊重 `replace_tag`**（false 时跳过 `<...>` 内内容），其余字段走 SQL `REPLACE()` |
| `pluginTimefactor.go` | ✅ | `plugin/timefactor/setting` 读 / 写（`settings.time_factor`）：默认值补齐；保存时丢弃 `update_running`、**保留 `today_count`/`last_sent`/`today_update`/`last_update` 等运行期计数** |
| `pluginJsonLd.go` | ✅ | `plugin/jsonld/config` 读 / 写（`settings.json_ld`）：返回前把 38 个字段的默认值补齐，保存走字段级 merge |
| `pluginGoogle.go` | ✅ | `plugin/google/setting` 读 / 写（`settings.google_auth`）：`redirect_url` / `client_id` / `client_secret` |
| `pluginTransfer.go` | ⚠️ 部分 | `plugin/transfer/task` 恒空（Go 也是内存态）；`plugin/transfer/download` **真实可用**（校验 provider 名、防穿越、按 `train→.wpm` 映射、作为附件返回）。`create` / `modules` / `start` 需 HTTP 拉远端数据，明确未实现 |
| `pluginMultiLang.go` | ⚠️ 部分 | 10 个动作可用：`config` 读写（`default_language` 回写 `system.language`、`site_type` 空值归一、`single` 模式自动剔除默认语言子站）、`sites`（补 `websites` 名称/根目录）、`validsites`、`site/save`（按 id 或 language 去重）、`site/remove`（只删配置）、`sync/status`（恒无任务）、`site/html/{logs,caches,cache/remove}`（扫 `cache/multiLang`）。**未实现**：`site/sync`（需跨站点复制 + 翻译外呼） |
| `pluginTitleImage.go` | ⚠️ 部分 | `config` 读写（尺寸/字号归零回落 800/600/32）、`upload` **真实可用**（字体只收 `.ttf` 且固定落 `uploads/titleimage/title_font.ttf` 并回写 `font_path`；背景图按 `md5(原文件名)` 命名且限 6 种图片后缀）。**未实现**：`preview`、`generate`（需图片渲染库） |

新增：`src/models/plugin.rs`（rewrite / robots / sitemap / timefactor / jsonld / google）、
`src/models/link.rs`（友情链接）、`src/models/tag.rs`（标签）、`src/models/redirect.rs`（301 跳转）、
`src/models/anchor.rs`（锚文本 + 批量加链接）、`src/models/replace.rs`（全站替换 + HTML 感知扫描器）、
`src/models/collector.rs` 扩展（关键词 CRUD + `settings.keyword`）。

验证：`cargo build` 无告警；`cargo test` **166 项全绿**（新增 39 项：plugin 6 + link 1 + tag 3 + redirect 3 + anchor 4 + replace 6 + transfer 3 + multilang 3 + titleimage 2 + 路由端到端 8）。

**Part 3 已完成 15 / 15**（`titleimage` 的渲染部分明确标注未实现）。

> **schema 事实**：`archive_data.id` 是 `bigint unsigned`、`tag_contents.id` 是 `int unsigned`。
> sqlx 按无符号解码，**直接取成 `i64` 会整条查询失败**（且容易被 `Err(_) => 跳过` 静默吞掉）。
> 统一用 `CAST(id AS CHAR)` 再解析。

---

## 十二、Part 4 进度（2026-11-02）

| Go 文件 | 状态 | 说明 |
|---|---|---|
| `sensitiveWords.go` | ✅ | `setting/sensitive/{words,check,sync}`：数组配置 + 三种规则（`词` / `词\|替换词` / `{正则}[|替换词]`，用已有 `regex` 依赖真跑正则）、命中检测、等长 `*` 掩码。`sync` 需外呼，明确未实现 |
| `pluginComment.go` | ✅ | `plugin/comment/{list,detail,delete,check}`（`comments` 表）：列表补 `item_title`、改内容**强制转义并置为正常**、单条切换/批量置状态、删除时扣减 `comment_count` |
| `pluginGuestbook.go` | ✅ | `plugin/guestbook/{list,detail,status,delete,export,setting}`（`guestbooks` 表）：关键词（用户名/联系方式/内容）+ `default/ok/spam` 三态筛选、单/批量改状态与删除、导出按配置字段顺序拼表头（末 5 列固定 Created Time/IP/Refer/Site Id/Spam）、配置 `fields` 按 `field_name` 去重 |
| `pluginSubscriber.go` | ✅ | `plugin/subscriber/{list,detail,save,delete,category/list,category/save,category/delete}`（`subscribers` + `subscriber_categories`）：邮箱必填、分类名唯一。`send` / `send/status` 需 SMTP，明确未实现（`send/status` 缺 `job_id` 时给出可执行的下一步提示） |
| `pluginUser.go` | ✅ | 12 个动作：`user/fields` 读写（字段名 `^[a-z][0-9a-z_]+$` 校验、去重、`required` 强制 false）、`user/field/delete`（同步 `ALTER TABLE users DROP COLUMN`）、`user/list`（id / 分组 / 用户名 / 真实姓名 / 手机号 / 综合 `q` / 三态 status / `subscribed·ordered·repurchase` 筛选）、`detail` 读写（PATCH 语义 + bcrypt + 用户名查重）、`balance`（±金额、备注必填）、`delete`（连带清 `user_auths`）、用户组 list/detail/save/delete（名称唯一、组内有会员拒绝删除） |
| `pluginFileUpload.go` | ✅ | `fileupload/{list,upload,delete}`：只收 `txt/htm/html/xml`、拒绝路径分隔符与点开头、落在 `public` 根、清单按 `md5(文件名)` 登记（同名不重复），列表补 `link` |
| `pluginMaterial.go` | ✅ | `material/{list,detail,import,delete,category/{list,detail,delete},convert/file}`（`materials` + `material_categories`）：分类名唯一、分类下还有素材拒绝删除、保存/删除后自动重算 `material_count`；`convert/file` 支持剥 BOM + 去标签压空白，**非 UTF-8 明确报错**（需转码库） |
| `pluginWatermark.go` | ⚠️ 部分 | `watermark/config` 读写（尺寸/位置/透明度/最小边归零回落，默认 20/9/100/400/`#ffffff`）、`watermark/upload` **真实可用**（字体只收 ttf 固定名，图片 6 种后缀固定名）。**未实现**：`preview`、`generate`（需图片处理库） |
| `pluginInterference.go` | ✅ | `interference/config` 读写（`open` / `mode` / 禁选择·禁复制·禁右键） |
| `pluginLimiter.go` | ⚠️ 部分 | `limiter/setting` 读写（白/黑名单、最大请求数、封禁时长、UA 黑名单、内存限制等 12 项）。**未实现**：真正的限流器——`blockedips` 恒空、`blockedip/remove` 明确报错 |
| `pluginHtmlCache.go` | ⚠️ 部分 | 13 个动作里可用 5 个：`config` 读写（`storage_url` 自动补尾部 `/`、`keep_local` 恒定 false）、`build/status` 与 `push/status`（恒 `null`）、`push/logs`（恒空）、`clean`（**真删** `<cache>/html` 并回条数）、`upload/file`（SSH 私钥落 `<data>/cert/htmlcache_ssh_private_key.key`）。**未实现**：`build` 与 `build/{index,category,archive,tag}`（需渲染管线）、`push`（需存储客户端） |
| `pluginAkismet.go` | ✅ | `akismet/setting` 读写（`open` / `api_key` / `check_type[]` / reCAPTCHA 三字段） |
| `pluginFulltext.go` | ⚠️ 部分 | `fulltext/config` 读写（引擎 / 地址 / 账号 / 评分 / 包含长度等 12 项）、`status`（恒 `running:false`）。**未实现**：`rebuild`（需索引引擎） |
| `pluginImportapi.go` | ✅ | `import/api` 读 + `import/token` **真生成**两个 32 位随机 token 并落库 |
| `pluginAiGenerate.go` | ⚠️ 部分 | `aigenerate/setting` 读写（`settings.ai_generate`）、`plans`（恒空）。**未实现**：`checkapi`、`article/{collect,start}`（需外呼） |
| — | — | **Part 4 已全部 15 / 15 覆盖**（其中 7 个为「配置可用、动作明确未实现」） |

新增：`src/models/sensitive.rs`（敏感词规则 + 掩码）、`src/models/comment.rs`（评论 CRUD + HTML 转义 + `&nbsp;` 折叠）、
`src/models/guestbook.rs`（留言 + 导出）、`src/models/subscriber.rs`（订阅用户 + 分类）、
`src/models/user_admin.rs`（会员 / 用户组 / 扩展字段）、`src/models/upload_file.rs`（验证文件）、
`src/models/material.rs`（素材 + 分类 + 文本转换）、`src/models/watermark.rs`（水印配置 + 资源上传）。

验证：`cargo build` 无告警；`cargo test` **195 项全绿**（新增 29 项：sensitive 4 + comment 2 + guestbook 2 + subscriber 2 + user_admin 5 + upload_file 2 + material 4 + watermark 2 + plugin 1 + 路由端到端 5）。

> **踩坑记录 3**：`site_setting::get_section()` 只认**对象**，读数组型配置（如 `upload_file`）必须用
> `get_array()`。用错时表现为「写入成功、读回永远是空数组」，很容易误判成保存逻辑有问题。

> **schema 事实**：`subscribers` / `subscriber_categories` 的 `id` 与时间列都是**有符号** `int(11)`，
> 必须映射成 `i32`；取成 `u32`/`i64` 会解码失败（与前面 `archive_data.id` 的无符号坑正好相反）。

> **踩坑记录 2**：正则敏感词用 `{\d{6,12}}` 这种写法，解析时**必须用 `rfind('}')`** 而不是 `find('}')`——
> 量词 `{6,12}` 自带 `}`，取第一个会把正则截断成 `\d{6,12`，编译失败后被静默跳过。
> 教训：**「解析器里的静默跳过 + 没有覆盖该写法的用例」= 功能悄悄消失**。

---

## 十三、Part 5 进度（2026-11-02）

| Go 文件 | 状态 | 说明 |
|---|---|---|
| `pluginPlace.go` | ✅ | `plugin/place/{setting,list,detail,delete}`（`places` 表 + `settings.place`）：树形/平铺双模式、`CAST(latitude AS CHAR)` 规避 `decimal(10,6)` 解码、别名自动去重加后缀、上级循环引用校验、删除连带整棵子树、`url_type`/`content_type` 非法值归一 |
| `pluginPush.go` | ⚠️ 部分 | `plugin/push` 读写（`settings.push`，`logs` 不被前端覆盖）、`plugin/push/logs`（最近 20 条）、`plugin/push/push` 先记一条失败日志再明确报错。**未实现**：真正调用百度/Bing/Google 推送接口（需 HTTP 客户端） |
| `pluginStorage.go` | ✅ | `plugin/storage` 读写（`storage_url` 补尾部 `/`、FTP/SSH webroot 去尾斜杠、`keep_local` 由用户决定）+ `plugin/storage/upload`（SSH 私钥落 `<data>/cert/ssh_private_key.key` 并回写配置） |
| `pluginWeapp.go` | ✅ | `plugin/weapp/config` 读写（`settings.weapp`），`server_url` 派生为 `base_url/api/wechat` 不落库 |
| `pluginWechat.go` | ⚠️ 部分 | `plugin/wechat/config` 读写（`settings.wechat`）+ `message/{list,delete,reply}` + `reply/rule/{list,save,delete}`（默认规则唯一）+ `menu/{list,save,delete}`（名称 ≤20 字符预校验、删除连带子级）。**未实现**：`message/reply` 真正推送到微信、`menu/sync`（需 access_token 与 HTTP 客户端） |
| `pluginOrder.go` / `pluginFinance.go` / `pluginRetailer.go` / `pluginCommission.go` / `pluginWithdraw.go` / `pluginPay.go` | ✅ | 见「Part 5 收尾」小节（Batch A） |
| `pluginSendmail.go` / `pluginBackup.go` | ✅ | 见「Part 5 收尾」小节（Batch B） |
| `pluginTranslate.go` / `pluginLLMs.go` | ✅ | 见「Part 5 收尾」小节（Batch C） |
| `anqi.go` / `aiChat.go` | ✅ | 见「Part 5 收尾」小节（Batch D，Part 5 收官） |

新增：`src/models/place.rs`（城市站 + 树组装）、`src/models/wechat.rs`（微信/小程序配置 + 消息/规则/菜单）、
`src/models/plugin.rs` 扩展（push / storage 配置与推送日志）。

验证：`cargo check --all-targets` 无告警；`cargo test` **202 项全绿**（新增 7 项：place 3 + wechat 2 + 路由端到端 2）。

> **构建环境备忘 2**：本机空闲内存波动大（几百 MB ~ 2GB），`-C debuginfo=2` 编最终 crate 会
> `rustc-LLVM ERROR: out of memory` / `STATUS_STACK_BUFFER_OVERRUN`，但**代码本身没问题**（`cargo check` 全过）。
> 解法：只对最终 crate 关调试信息，依赖产物复用——
> - bin：`cargo rustc --bin rustcms -- -C debuginfo=0`（已固化进 `tmp/build.ps1`）；
> - 测试：`cargo rustc --bin rustcms -- --test -C debuginfo=0` 后直接跑 `target/debug/deps/rustcms-*.exe --test-threads=1`。
>
> **schema 备忘**：`wechat_menus.name` 是 `varchar(20)`、`type` 是 MySQL 关键字——
> Rust 侧字段改名 `kind` 且要同时 `#[serde(rename = "type")] #[sqlx(rename = "type")]`，否则
> `FromRow` 按字段名找列 `kind` 会查空；名称超长要在入库前预校验，别让用户看到 MySQL 1406。

### Part 5 下一步
1. `pluginOrder.go` / `pluginFinance.go`（交易读 + 状态流转）
2. `pluginRetailer.go` / `pluginCommission.go` / `pluginWithdraw.go` / `pluginPay.go`
3. `pluginTranslate.go` / `pluginLLMs.go` / `pluginSendmail.go` / `pluginBackup.go`
4. `anqi.go`（云端服务外呼，最后）

Part 3 已落地经验（Part 4 / 5 继续复用）：
- 配置统一走 `models/site_setting.rs`（字段级 merge）；数组型配置必须用 `get_array`，`get_section` 只认对象；
- 上传类统一「临时 `public_dir` + 扩展名白名单 + md5 命名」；
- 表格 `id` 若是无符号，取 `i64` 前先 `CAST(id AS CHAR)`；`decimal` 列同理先 `CAST(... AS CHAR)`。

---

## 十四、Part 5 收尾进度（2026-11-03，全项目收官）

| 批次 | Go 文件 | 状态 | 说明 |
|---|---|---|---|
| Batch A | `pluginOrder.go` / `pluginFinance.go` / `pluginRetailer.go` / `pluginCommission.go` / `pluginWithdraw.go` / `pluginPay.go` | ✅ | `/plugin/order/{list,detail,pay,deliver,finished,canceled,refund,refund/apply,export}`、`/plugin/finance/{list,detail}`、`/plugin/retailer/{list,detail,config,realname,apply}`、`/plugin/commission/{list,detail}`、`/plugin/withdraw/{list,detail,approval,finished,apply}`、`/plugin/pay/{accounts,detail,delete,upload,statistic}`。真实支付扣款 / 提现打款的外呼未实现 |
| Batch B | `pluginSendmail.go` / `pluginBackup.go` | ✅ | `/plugin/sendmail/{list,setting,templates,template,template/preview,test}`（授权码掩码回传=未修改）、`/plugin/backup/{list,dump,status,restore,delete,export,import,cleanup,remark}`（SQL 逻辑备份真实现）。SMTP 发信 / 远程存储未实现 |
| Batch C | `pluginTranslate.go` / `pluginLLMs.go` | ✅ | `/plugin/translate/{config,logs,log/texts,log/text/remove,log/text/save}`（译文修正同步替换 `cache/multiLang` 缓存，`ReplaceTranslateText` 4 组正则）、`/plugin/llms/{status,build,setting}`（**真生成** `public/llms.txt` + `llms-full.txt`，前 1000 条正文导出）。翻译 API 外呼未实现；`HtmlToMarkdown` 为简化版 |
| Batch D | `anqi.go` / `aiChat.go` | ✅ | 见下（Part 5 收官） |

### Batch D — anqi.go + aiChat.go

**anqi.go（`/system/api/anqi/*`）**：

| 路由 | 分流 | 说明 |
|---|---|---|
| `GET /info`、`GET /check` | ✅ 真实 | 读 `settings.anqi`（`AnqiUserConfig` 全字段默认值补齐），`valid = expire_time > now` 现算；云端校验外呼未实现 |
| `POST /login`、`POST /upload`、`POST /template/share`、`POST /template/download`、`POST /feedback`、`POST /extract/keywords`、`POST /extract/description` | ❌ 未实现 | 接口保留 + 参数校验 + 明确提示（需外呼云端 API） |
| `POST /translate`、`POST /ai/pseudo` | ⚠️ 半实现 | 文档/草稿存在性、目标语言是**真实站内校验**；翻译/伪原创本体未实现 |
| `POST /ai/stream`、`GET /ai/stream/data` | ⚠️ 半实现 | stream 产生端未实现；`stream/data` 对未知 stream_id 恒回 `finished`（与 Go 对未知 id 的行为一致） |
| `POST /restart` | ❌ 未实现 | Rust 版无守护重启通道 |

**aiChat.go（`/system/api/anqi/ai/*`，code:0/-1 特例口径）**：

| 路由 | 分流 | 说明 |
|---|---|---|
| `GET/POST /ai/settings` | ✅ 真实 | `settings.ai_setting`（`eino.Configs`）：`configs` 数组整份替换、`last_model`/`mcp` 保留，默认值补齐 |
| `GET /ai/history`、`GET /ai/sessions` | ✅ 真实 | **DB 版实现**：Go 的 `AddMessage` 同步落 `ai_chat_messages`，`/history` 按 `seq ASC` 重建（`files` JSON 解析），`/sessions` 聚合（title=首条用户消息 ≤50 字、last_message ≤100 字、msg_count） |
| `GET /ai/agents`、`GET /ai/agents/:id/logs` | ✅ 真实 | `ai_agents`（id ASC）/ `ai_agent_logs`（agent_id 过滤、id DESC 分页，Go 不回 total） |
| `POST /ai/upload` | ✅ 真实 | 附件落 `<cache>/ai/upload/<session_id>/<时间戳>_<文件名>`，会话 ID 安全字符过滤 |
| `POST /ai/image/confirm`（action=0） | ✅ 真实 | 舍弃是 no-op；action=1 保存需下载远端图片，未实现 |
| `GET /ai/image/histories` | ⚠️ 恒空 | 历史存云端服务侧，Go 拉取失败也返回空，照实返回 `total:0, data:[]` |
| `POST /ai/chat`、`POST /ai/agents/:id/chat`、`GET /ai/health`、`GET /ai/chat/trace` | ❌ 未实现 | Rust 版未内置 LLM 服务（等价 Go `AiSrv==nil` 分支）；agent/chat 的「智能体不存在」「消息不能为空」是真实站内校验 |
| `POST /ai/chat/confirm` | ⚠️ 半实现 | 参数校验与 Go 完全一致（decision 四值白名单）；无对话即无待审批，恒回 `no pending approval` |
| `POST /ai/image` | ❌ 未实现 | 空提示词校验真实；生图本体需云端服务外呼 |

新增：`src/models/anqi.rs`（云端服务登录态 + AI 配置 + 智能体 + 会话消息）。
路由层注意：`anqi` party 在 Go 里全部挂 `ParseAdminToken`，Rust 侧所有 handler 均先 `require_admin`。

> **踩坑记录 4**：MySQL 的 `SHOW TABLES LIKE ?` 不支持参数占位符（报 1064），
> 检查表是否存在要用 `information_schema.tables WHERE table_schema = DATABASE() AND table_name = ?`。
> 另外 `ai_chat_messages.seq` 是 `int(10) unsigned`，取 `i32` 会整条查询失败——
> 与 Part 3 的「无符号列」坑同源。

### 总体完成度结论（2026-11-03）

- **Part 1–5 共 63 个 Go 文件全部覆盖**：配置读写 / 列表 / 状态类接口全部真实实现；
  依赖外部服务的动作（HTTP 抓取、SMTP 发信、图片处理、LLM/翻译/云端服务外呼、
  支付网关真实扣款、远程存储推送等）按统一口径「接口保留 + 参数校验 + 明确未实现提示」。
- 验证基线：`cargo check --all-targets` 无告警；`cargo test --test-threads=1`
  **236 项全绿**（Batch D 新增 6 项：anqi 模型 4 + 路由端到端 2）。
- 已知简化（均有注释与测试说明）：`HtmlToMarkdown` 简化版、sitemap 不分片、
  多站点单连接池、模板 zip 打包/上传、AI 对话与生图。

---

## 十五、前台路由核心管线（2026-11-03，前台批次 1）

| 环节 | Go 源 | Rust 实现 |
|---|---|---|
| 兜底入口 | `route/base.go` 的 `/{path:path}` | `front::reroute`（`routes.rs::router` 的 `.fallback`，`/api`、`/system/api` 精确路由优先） |
| Check301 | `middleware/auth.go:Check301` | 查 `redirects` 表（全 URI 精确匹配），命中且目标≠自身 → 301 |
| ParseUserToken | `middleware/userAuth.go` | header `token` > `Authorization: Bearer` > cookie `token`；JWT 有效且会员启用时注入 `userId/userInfo/userGroup` 到模板上下文；过期前 1 小时回 `update-token` 续签头；JWT 无效回退 `users.api_token`；**账号被禁用直接回 `{"code":1001,"msg":"ThisAccountHasBeenDisabled"}` 并中断**（与 Go 一致） |
| HandlerTimeout | `middleware/timeout.go` | 整条解析渲染管线包 `tokio::time::timeout(30s)`，超时 408 |
| FileServe | `controller/common.go:FileServe` | `public_dir`（config.json，默认 `../public`）下的真实文件按扩展名回 Content-Type；逐段防路径穿越；favicon.ico 缺文件回 400；robots.txt 缺文件回默认内容。**放在 DB 检查之前**（静态资源不依赖库） |
| CheckCloseSite | `controller/common.go` | `settings.system.site_close`（1=全关，2=仅蜘蛛可见）、`ban_spider`、`safe.ua_forbidden`、`safe.ip_forbidden`（前缀匹配、去掩码与 `.0`、放行 127.0.0.1）；闭站页渲染 `errors/close.html` 或纯提示，403 |
| ParseRoute | `controller/common.go:ParseRoute` | 规则顺序对齐：search → archiveIndex → peopleIndex/people → tagIndex → tag → page → category → archive；每条命中都做存在性校验（module 按 `url_token`、category 按 type/status、archive 校验 module/catname 对应关系，对不上直接 404）；固化链接 `archives.fixed_link` 优先 |
| NotFound/500 | `controller.NotFound/InternalServerError/ShowMessage` | `errors/404.html` / `errors/500.html` 渲染失败时退回 ShowMessage 风格内联 HTML；ShowMessage 提示页逐字对齐 Go |

各页面模板选择优先级（与 Go controller 一一对应）：
- 首页 `IndexPage`：`index/index.html` → `index.html`；
- 模块列表 `ArchiveIndex`：`{表名}/index.html` → `{表名}_index.html`；
- 栏目页 `CategoryPage`：栏目 template → `{表名}/{url_token}.html` → `{表名}/list-{id}.html` → `{表名}/list.html` → `{表名}_list.html`；
- 单页 `PagePage`：栏目 template → `page/{url_token}.html` → `page/detail-{id}.html` → `page-{id}.html` → `page/detail.html` → `page_detail.html`；
- 详情页 `ArchiveDetail`：内容 template → 栏目 detail_template → `{表名}/detail-{id}.html` → `{表名}/detail.html` → `{表名}_detail.html`；
- 标签列表 `TagIndexPage`：`tag/index.html` → `tag_index.html`；标签页 `TagPage`：`tag/list-{id}.html` → `tag/list.html` → `tag_list.html`；
- 搜索页 `SearchPage`：`search/{模块token}.html` → `search/index.html` → `search.html`。

新增/修改：
- **新增 `src/front.rs`**（前台管线 + 测试 8 项）；`routes.rs` 删除旧的 `front/index/render_*`（约 200 行），`.fallback` 指向 `front::reroute`，`/preview/list/:cid` 复用 `front::render_category_preview`；
- `src/models/url.rs`：补 `Search`/`PeopleIndex`/`People` 三类默认规则（Go `GetRewritePattern` 对所有模式统一补齐）、`tag_url`/`tag_index_url` 生成器；
- `src/models/module.rs`：新增 `get_by_token`（前台按 `url_token` 匹配模块，对齐 `GetModuleFromCacheByToken`）；
- `src/models/archive.rs`：`ArchiveQuery` 新增 `tag_id` 筛选（`tag_data` 关联表）；
- `src/template/site_provider.rs`：新增 `keyword`（搜索过滤）、`tag`/`tag_index` 上下文、`tagList`/`tagDetail` 标签实现、tdk 的标签分支、分页链接的标签形态；
- `src/template/mod.rs`：for 循环支持遍历字符串（pongo2 行为，`tag/index.html` 需要）。

**踩坑记录 5**：`url::match_path` 的规则串带前导 `/`（Go 是构建正则时才 TrimLeft 去掉），
`parse_route` 必须传完整 `path` 而不是去斜杠后的 param，否则栏目/详情页全部 404。

**已知差异（均有注释，不静默）**：
- mode 4（自定义正则）在 `settings.rewrite.patten` 为空时回落字符串模式 2 的规则（rustcms 既有约定，真实库即此配置）；patten 非空的解析未实现；
- 多语言子站、城市分站（place）、HTML 缓存、蜘蛛访问日志、手机端模板（mobile/ 前缀与 301）、sitemap/robots 自动生成未移植；
- `/people`、`/peoples` 规则能匹配但明确提示「会员主页尚未移植」（会员中心未到批次）。

验证：`cargo check --all-targets` 无告警；`cargo test --test-threads=1` **247 项全绿**（新增 11 项）。

## 十六、前台交互 API 五批并行移植（2026-11-04）

### 批次划分与文件归属（互不重叠，可并行）

| 批次 | 领域 | API 文件 | 数据层文件 | 端点 |
|---|---|---|---|---|
| A | 前台内容只读 + 站点信息 | src/api_content.rs（56KB） | src/models/front_content.rs（43KB） | /archive/* 7、/category/* 2、/module/* 2、/page/* 2、/tag/* 3、/place/* 2、/nav/list、/metadata、/languages、/banner/list、/friendlink/* 4 |
| B | 会员 / 账户 / 认证 | src/api_member.rs（94KB） | src/models/member.rs（70KB） | /login /logout /register /forget /password/reset /verify/email /captcha /account/* /member/* /user/* 20 /sign/* /subscription /attachment/upload |
| C | 购物车 / 订单 / 店铺 / 支付 / 分销 | src/api_trade.rs（44KB） | src/models/trade.rs（120KB） | /cart/* 6、/order/* 9、/orders、/payment/check、/point/logs、/shop/* 7、/retailer/* 9、/user/recharge、/user/withdraw、/alipay/pay、/paypal/* |
| D | 评论 / 留言 / 收藏 / 微信 / 小程序 | src/api_interact.rs（53KB） | src/models/interact.rs（38KB） | /comment/* 3、/favorite/* 4、/guestbook.html、/guestbook/fields、/wechat* 3、/weapp/* 2、/google/url、/log |
| E | 后台 API 缺口 | src/api_admin_gap.rs（26KB） | src/models/admin_gap.rs（35KB） | /system/api/password/* 3、/system/api/archive/* 8、/system/api/attachment/addurl、/system/api/anchor/addfromtitle、/system/api/htmlcache/upload、/system/api/setting/convert/webp |

合计新增约 580KB（10 个文件）。

### 合流方式

各批在自己的 `api_*.rs` 暴露 `pub fn router() -> Router<AppState>`（用绝对路径），主线在 `src/routes.rs::router()` 里 `.merge(...)` 五次后交给 `front::reroute` 兜底。公共件（`ok` / `ok_msg` / `fail` / `failed` / `NO_DB` / `model_error` / `bearer_token` / `require_admin` / `api_not_found`）统一提升为 `pub(crate)` 供各批复用；`src/models/mod.rs` 预先声明 5 个新模块，避免并行改同一文件。

### 端点覆盖度（机器核对，非人工估算）

Go 侧有机器可读的端点目录 `provider/api_catalog.json`（403 条，含 method / path / handler / desc）。新增 `tmp/api-coverage.ps1` 做自动比对：

- 扫 Rust 全部 `.route("/x", get(..).post(..))`；`.nest("/api", …)` / `.nest("/system/api", manage_router())` 的**嵌套前缀要拼到内层相对路径上**（内层路径同样以 `/` 开头，不能按「是否以 / 开头」判断绝对/相对）；
- `api_route!` 宏同时注册裸路径与 `/api` 别名，两条都要计入；
- Go 的 `{id}` 与 Rust axum 0.7 的 `:id` 统一按参数段匹配；
- 口径修正：正则 `\.route\(\s*"([^"]+)"` 只有 **1 个捕获组**，取 `Groups[1]`（曾误用 `Groups[2]` 导致路径恒为空、覆盖率假性为 0）。

**结果：路径级 403/403、方法级 403/403。**

### 本轮修掉的真问题

1. **支付接口越权**：`api_trade.rs` 的 `/order/payment` 取到 `user_id` 却没用 → 登录用户可支付他人订单。已补 `if user_id != 0 && order.user_id != user_id { return fail("权限不足") }`（匿名下单 `user_id=0` 放行），与同文件 `/order/detail` 的归属校验同口径。
2. **管理员端点前缀不对**：原来只挂在公开 `/api/admin/*`，而 Go 的规范前缀是 `/system/api/admin/*`（`manage.Party("/admin", …)`）外加 `/system/api/login`。已在 `manage_router()` 里补齐 13 条，两套前缀行为一致。
3. **htmlcache 证书上传缺两步**：Go 的 `PluginHtmlCacheUploadFile` 落盘后还会把私钥文件名回写 `html_cache.ssh_private_key` 并记一条后台操作日志；Rust 两处实现都只落盘。已补 `site_setting::save_section(pool, plugin::HTML_CACHE, …)` + `admin::record_action(...)`，响应信封统一为 Go 的 `{code:0, msg:"文件上传完成", data:fileName}`。

### 环境硬约束（后续批次必读）

- 本机 16GB 内存 + **8GB 固定页面文件**（`AutomaticManagedPagefile=False`，**无管理员权限无法扩容**），提交上限 24414MB、常驻占用 ~23GB，**提交余量仅 ~1GB**。
- `cargo test` 编测试二进制时 rustc 报 `memory allocation of 2097152 bytes failed` → 退出码 `0xc0000409`。**这是 OOM abort，不是栈溢出**（`rustc.exe` 的 PE 栈预留只有 2MB；虽已备份并抬到 64MB——`rustc.exe.stack-bak`——但与该错误无关）。
- 已验证的降内存参数：`CARGO_BUILD_JOBS=1`、`CARGO_INCREMENTAL=0`、`GOGC=20`、`CARGO_PROFILE_{DEV,TEST}_DEBUG=0`、`CARGO_PROFILE_{DEV,TEST}_CODEGEN_UNITS=1024`、`CARGO_PROFILE_*_LINKFLAGS=-C link-arg=-Wl,--no-keep-memory`、`RUST_MIN_STACK=64MB`。
- **结论：在提交余量 1GB 的现状下无法编译本 crate 的 test harness**（`routes.rs` 内嵌 2 万行测试代码，泛型实例化密集）。要跑全量测试需先释放 2~4GB 内存，或把 crate 拆成多个 crate 分开编译。

### 仍未移植（非本批范围）

- `controller/graphql`（5 文件）—— GraphQL 接口层；
- Go `provider/` 175 文件里前台渲染上下文相关的部分（本批只重写了后台 API + 前台只读 API 所需的数据访问）；
- `pkg/`（AI/eino、MCP、intent）、`tags/`（42 文件自定义模板标签库）、`library/`（28 文件）、`crond/`（定时任务）、`view/`（模板引擎）—— 模板层已用 tera 另起炉灶（`src/template/`），其余未移植；
- 明确未实现的外部依赖类动作维持既有口径「接口保留 + 参数校验 + 明确未实现提示」：云端服务 / 翻译 / LLM 外呼、SMTP 发信、HTTP 抓取与推送、zip 打包解压、图像处理（缩略图 / 水印 / webp）、全文索引重建、静态缓存生成与推送、在线升级、限流器、多语言跨站同步、多站点切换登录、AutoMigrate。

---

## 十七、当前完成度快照（2026-11-04）

### 规模对比

| | Go 版 | rustcms |
|---|---|---|
| 代码行数 | 142,953 行（16 个目录，非测试） | 67,072 行（73 个 `.rs`） |
| 生产代码 | — | 46,933 行（不含 `#[cfg(test)]` 块） |
| 测试代码 | — | 20,139 行（308 个测试函数，分布 70 个文件） |

### 分层完成度

| 层 | 分母 | 已完成 | 比例 | 说明 |
|---|---|---|---|---|
| 后台管理控制器 | 63 个 Go 文件 | 63 | **100%** | Part 1–5 收官（见第十四章） |
| HTTP 端点（路径级） | 403（`provider/api_catalog.json`） | 403 | **100%** | 机器核对，非人工估算 |
| HTTP 端点（方法级） | 403 | 403 | **100%** | 同上 |
| 外部依赖类动作 | — | 58 处占位 | 需外呼 | 接口保留 + 参数校验 + 明确未实现提示 |
| 平台层包 | pkg 88 / tags 42 / library 28 / crond 1 / view 3 | 0 | 0% | 模板层已用 tera 另起炉灶 |
| GraphQL 接口层 | `controller/graphql` 5 文件 | 0 | 0% | 未开始 |

### 剩余三类尾巴

1. **外部服务对接**（58 处占位）：SMTP 发信、HTTP 抓取与推送、zip 打包解压、
   图像处理（缩略图 / 水印 / webp）、全文索引重建、静态缓存生成与推送、在线升级、
   限流器、多语言跨站同步、多站点切换登录、AutoMigrate、支付网关真实扣款、
   翻译 / LLM / 生图外呼、云端服务登录与上传分享。
2. **未移植的包**：`controller/graphql`、`pkg/`（AI/eino、MCP、intent）、
   `tags/`（42 文件自定义模板标签库）、`library/`（28 文件）、`crond/`（定时任务）、
   `provider/` 175 文件里与前台渲染上下文相关的部分。
3. **验证缺口**：本机提交余量约 1GB，`cargo test` 编不出 test harness（rustc OOM abort）。
   最后一次全绿记录是 2026-11-03 的 **236 项**；第十六章新增的五批测试代码尚未跑过。

### 已知简化（均有注释与测试说明）

`HtmlToMarkdown` 简化版、sitemap 不分片、多站点单连接池、模板 zip 打包/上传、AI 对话与生图。

---

## 十八、A 类接线补全（2026-11-07）

本轮把「已有 ext 能力、但 handler 仍占位」的缺口接上，并修正与实现脱节的测试。

### 1. 升级包上传解压
- `POST /system/api/version/upload-upgrade` 真实实现（原为「需 zip/tar 库」占位）。
- 表单：`file`（按魔数识别 zip / tar.gz）、`version`、`force=1`；单路由 `DefaultBodyLimit` 放宽到 512MB + 1MB。
- 对齐 Go `upgrade.go`：包内 `version.json` 可缺省（缺省且非强制时报错）、版本不高于当前且非强制时拒绝；
  剥离公共顶层目录；跳过 `template/ public/ data/ uploads/ logs/ static/`、
  `*.sh/*.bat/*.cmd/*.ps1`、`config.json/favicon.ico/version.json`；
  解压上限 2GB / 20000 条；主程序写 `.new`，成功后备份并替换。
- 在线升级 `version/upgrade` 复用同一 `install_upgrade_archive`（官方包固定 zip）。

### 2. zip 备份恢复
- `models::backup::restore` 增加 `template_dir` / `public_dir` 参数；`.zip` 先还原 `template/`
  到模板目录、`static/` 到 `public/static/<模板目录名>`，再执行 `database.sql`。
- 包内路径逐段校验，拒绝 `..`、绝对路径与盘符；`.sql` / `.zip` 共用 `execute_sql`
  （`_logs` 语句仍跳过）。

### 3. 限流器接线
- `plugin` 新增进程内 `LimiterRuntime`（对齐 Go `Website.Limiter`）：
  `open / white_ips / black_ips / max_requests / block_hours / is_allow_spider /
  ban_empty_agent / ban_empty_refer`，名单支持 IPv4 CIDR。
- `front::reroute` 在进入渲染管线前调用 `limiter_check`：私网放行、白名单放行、
  黑名单/已封禁 403、超限 429；`/static`、`/uploads` 可单独禁空 Referer。
- `GET /plugin/limiter/blockedips` 返回真实的 `{ip, block_time}` 列表；
  `POST /plugin/limiter/blockedip/remove` 走真实封禁器。
- 服务启动时按 `settings.limiter` 装载；保存设置后立即重建（重启后内存封禁列表清空，与 Go 一致）。
- `crates/ext` 的 `IpBlocker` 补 `is_blocked` / `blocked_entries`。

### 4. 测试与基建
- sendmail 测试改为「把服务器指向 127.0.0.1:1 → 断言发送失败」，不再断言旧的「未实现」文案。
- 升级上传测试覆盖：缺文件、非法格式、低版本 zip（版本校验在落盘前拒绝）。
- `xml_fields` 修复 CDATA 与嵌套容器解析（此前 `<xml>…</xml>` 会把整段吞成 `xml` 字段）。
- `test_pool` 支持 `RUSTCMS_TEST_CONFIG`，受限环境可把测试配置放在仓库内。
- 模板 `system.TemplateUrl` 统一为 `/static/<template_name>/`（与 Go `tags.system` 一致）。

### 5. 验证结果（2026-11-07）
- `cargo check --offline --all-targets`：通过，0 告警。
- `cargo test --offline --no-run`：通过（约 2m30s）。
- `crates/ext`：42 项非网络测试全绿；7 项回环网络测试（HTTP/SMTP 本地监听）因本机沙箱
  禁止 `127.0.0.1` 连接而未执行，非代码缺陷。
- 主 crate：**320 passed / 5 failed**。5 项均为环境限制：3 项依赖 MySQL
  （沙箱无法连 `127.0.0.1:3306`）、1 项依赖 `E:/CodeBuddy/clientFiles`（仓库外不可写）、
  1 项是依赖数据库的首页渲染。
- 构建环境备注：本机 gcc 的 cc1/库搜索路径失效，编译前需把
  `mingw64/libexec/gcc/x86_64-w64-mingw32/16.2.0` 加入 `PATH`、
  把 `mingw64/x86_64-w64-mingw32/lib` 等加入 `LIBRARY_PATH`；
  `RUST_MIN_STACK` 必须写字节数（如 `67108864`）。

### 6. 仍存缺口（非本轮范围）
- 备份 **dump 仍只产 `.sql`**：zip 备份包可恢复，但生成侧尚未打 `template/` + `static/`。
- 微信消息推送 / 授权码换用户信息 / 小程序二维码、Google 推送 OAuth、PayPal capture
  等外呼仍未接线。
- 支付宝 RSA2 验签、小程序 AES 解密、GraphQL、水印与更多图像格式需要新增 crate，离线环境不可。
- `tags/`（tera 已替代）、`library/`、`crond/`、`pkg/` 仍按既有结论不移植。

---

## 十九、备份 zip 生成与 sitemap 自动生成（2026-11-07）

### 1. 备份 dump 产出 zip
- `models::backup::dump` 改为生成 `<时间戳>.zip`，包含 `database.sql` + `template/` + `static/`
  （与 Go `BackupData` 一致）；跳过 `_logs` 表；`template/` 来自当前模板目录，
  `static/` 来自 `public/static/<模板目录名>`。
- `dump` 增加 `template_dir` / `public_dir` 参数；`setting::migrate_db`、后台备份、
  设计市场初始化三处调用同步更新。
- 顺带修掉旧实现的分片写盘 bug（`append` 后的尾块 `fs::write` 会覆盖前文）。
- zip 条目暂用 stored（ext 未实现 deflate 压缩），功能等价，体积大于 Go 的 Deflate 包。
- 旧 `.sql` 备份仍可恢复；列表 / 备注 / 导入继续兼容 `.sql` 与 `.zip`。

### 2. 前台 sitemap 自动生成
- `front::serve_file` 增加旧路径重写：`/archive*`、`/tag*`、`/category*` 的 `.xml/.txt`
  改写为 `/sitemap-...`（Go `FileServe`）。
- `sitemap.xml` / `sitemap.txt` 文件不存在时，按 `settings.sitemap` 的 `type` 自动调用
  `plugin::sitemap_build` 并返回产物；未配置 `settings.sitemap` 时保持原 404 行为。
- 与 Go 的差异：仍不按 `page_size` 分片，只产出单个 `sitemap.{type}`。

### 3. 测试与验证
- `dump_writes_real_database_sql` → `dump_writes_real_database_zip`：断言 zip 内
  `database.sql` + `template/index.html` + `static/app.css`（需真实库）。
- 新增 `packs_directory_into_zip_entries`（纯离线，覆盖目录打包与条目名）。
- 新增 `rewrites_legacy_sitemap_paths`，并在静态文件测试里覆盖 `/archive/x.txt` 的实际服务。
- `cargo test --no-run`：通过（约 3m）；主 crate **322 passed / 5 failed**，5 项与上一轮
  相同的环境限制（沙箱 MySQL、`clientFiles` 不可写、依赖 DB 的首页渲染）。
- `cargo check --all-targets`：0 告警。

### 4. 说明
- 工作区曾出现外部进程的全库 `anqi` → `rust` 改名（含 API 路径、settings key、
  `auth.anqicms.com` 域名改写），会破坏与 Go 版的数据/接口兼容，本轮已回滚，未纳入提交。

---

## 二十、sitemap 分片与 HTML 缓存前台命中（2026-11-07）

### 1. sitemap 分片
- `plugin::sitemap_build` 改为与 Go `BuildSitemap` 相同的分片结构：
  - `sitemap-category.{type}`：首页 + 栏目 / 单页（`exclude_*` 源头过滤）；
  - `sitemap-archive-{N}.{type}`：文档按 `page_size` 分片；
  - `sitemap-tag-{N}.{type}`：标签按 `page_size` 分片，`exclude_tag` 时整体跳过；
  - `sitemap.{type}`：index（xml 为 `<sitemapindex>`，txt 每行一个子文件 URL）。
- txt 子文件为 CRLF 连接的 loc（无末尾换行）；xml 子文件带 `anqi-style.xsl` 引用，
  index 带 `anqi-index.xsl`。
- 生成前清理同类型旧分片，避免分页减少后残留。
- `sitemap-time.log` 内容改为 Unix 秒（Go 口径），mtime 继续被 `sitemap_time` 读取。
- 前台 `sitemap*` 文件缺失时自动重建并返回（含子文件请求）。
- 与 Go 差异：不生成空尾页；XML 暂不输出 `lastmod` 与多语言 `xhtml:link`。

### 2. HTML 缓存前台命中
- 后台 build 已能本地生成到 `<cache>/html`；本轮补齐前台读取与回填：
  - 首页用 `index_cache`、详情用 `detail_cache`、列表 / 栏目 / 单页 / 标签用 `category_cache`；
  - 已登录会员、`Cache-Control: no-cache`、搜索 / 404 等不缓存；
  - 命中且未过期直接返回缓存 HTML；渲染成功后写回同路径。
- 缓存路径与 Go `transToLocalPath` 一致：`/` → `/index.html`，无扩展 → `<path>/index.html`，
  带扩展原样；query 统一忽略（与 Rust 后台 build 既有约定一致）。
- 未实现：移动端 `cache/mobile`、query 独立缓存、内存二级缓存、远程推送（沿用既有口径）。

### 3. 测试与验证
- 新增 `sitemap_writes_sharded_formats`（纯离线：txt CRLF、xml urlset/sitemapindex、index 内容）。
- 新增 `html_cache_paths_match_go`（纯离线：路径规则）。
- `sitemap_config_and_build` 更新为 index + 子文件断言。
- `cargo check --all-targets`：0 告警；`cargo test --no-run`：通过。
- 主 crate：**324 passed / 5 failed**，失败项仍是环境限制（沙箱 MySQL、`clientFiles`、
  依赖 DB 的首页渲染）。