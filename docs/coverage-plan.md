# rustcms 能力与覆盖说明

> 本文是 rustcms 的内部工程文档，回答三个问题：**这个项目有哪些能力、每个能力怎么实现、凭什么说它能用**。
> 内容分四部分：能力清单（按模块，含实现要点 / 实测证据 / 已知边界）、验证方法与基线、构建环境约束、遗留与后续。

---

## 一、概览

### 1.1 项目规模（2026-11-04 快照）

| 指标 | 数值 |
|---|---|
| 源码总行数 | 67,072 行（73 个 `.rs`） |
| 生产代码 | 46,933 行（不含 `#[cfg(test)]` 块） |
| 测试代码 | 20,139 行（308 个测试函数，分布 70 个文件） |

### 1.2 能力完成度

| 能力层 | 分母 | 已完成 | 比例 | 说明 |
|---|---|---|---|---|
| 后台管理模块 | 63 个模块 | 63 | **100%** | 配置读写 / 列表 / 状态类接口全部真实实现 |
| HTTP 端点（路径级） | 403 | 403 | **100%** | 机器核对，非人工估算（方法见 3.3） |
| HTTP 端点（方法级） | 403 | 403 | **100%** | 同上 |
| 外部服务对接动作 | — | 58 处占位 | — | 口径：接口保留 + 参数校验 + 明确未实现提示；后续多条工作流已补掉大部分（见 2.5–2.10） |
| GraphQL 接口层 | — | 0 | **0%** | 未开始（见第四章） |
| 平台辅助能力（模板标签库 / 定时任务框架 / 视图层） | — | — | — | 视图层由 tera 承载；定时任务已有 2 条内置任务（见 2.11） |

### 1.3 分层结构

- **数据访问层**：`src/models/<领域>.rs`（sqlx + `FromRow` + `ModelError`）。
- **接口层**：`src/routes.rs`（后台 `/system/api/*`）+ `src/api_*.rs`（前台与对外 `/api/*`），统一信封 `{"code":0,"msg":"","data":...}`。
- **渲染层**：`src/front.rs` + `src/template/`（tera 模板 + 自定义标签与上下文提供者）。
- **能力层**：`crates/ext`（进程外部世界：HTTP / SMTP / ZIP / 限流 / 表结构同步），零新依赖。

---

## 二、模块能力清单

### 2.1 后台管理 API

按主题分五个分区，模块数与动作数为规划期估算。

| 分区 | 主题 | 模块数 | 动作数（估算） |
|---|---|---|---|
| **Part 1** | 内容体系（内容 / 分类 / 模块 / 附件 / 导航 / 装修 / 采集 / 技能 / 公共） | 9 | ~69 |
| **Part 2** | 设置 / 设计 / 站点 / 升级 | 7 | ~95 |
| **Part 3** | SEO / 收录 / 链接类插件 | 15 | ~70 |
| **Part 4** | 内容安全 / 互动 / 增强类插件 | 15 | ~87 |
| **Part 5** | 商业 / 交易 / 集成 | 16 | ~105 |

依赖顺序：Part 1 是底座（公共辅助 + 内容/分类/模块模型被其余分区复用），优先落地；
Part 2 的设置与设计体量最大、联动最多；Part 3/4/5 多为独立配置页 + 保存逻辑，可并行。

#### 2.1.1 内容体系（9 模块）

| 模块 | 能力 | 已知边界 |
|---|---|---|
| 公共 | `version/info`、`statistic/summary`、`statistic/dashboard`、`captcha` | `version/check`、`version/upgrade` 见 2.5 |
| 分类 | `list` / `detail` / `save` / `delete` / `count` 全实现 | 模板默认值推导未做 |
| 模块 | `list` / `detail` / `save` / `delete` / `field-delete` 全实现 | — |
| 导航 | 导航 + 导航分组的列表 / 保存 / 删除 | — |
| 内容 | `/system/api/archive/*`：`list` / `detail` / `save` / `delete`（回收站）/ `recover` / `status` / `recommend` / `sort` / `parent` / `category` / `time` | 快速导入（Excel / 分片）、删除单图、标签批量未做；定时发布由 2.11 的 crond 承接 |
| 附件 | `list` / `detail` / `upload`（multipart + MD5 去重 + 图片宽高）/ `delete` / `edit` / `category` / `scan` | 远程 URL 导入由 2.4 批 E 补；WebP 批量转换、缩略图生成未做 |
| 装修 | `types` / `page` / `save` / `reset`（`diy_pages` 表，8 种区块类型，页面 key 白名单） | — |
| 采集 | 配置读 / 存（`settings.collector`）、内容替换（SQL `REPLACE`）、关键词落库 | 真正抓取 / 问答组合 / 关键词拓展未实现（需 HTTP 抓取与 AI 外呼），接口明确返回未实现提示 |
| 技能 | `list` / `detail` / `edit` / `delete` / `reload`（文件系统 `data/skills/<name>/SKILL.md`，frontmatter 解析） | — |

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
- `src/routes.rs` 新增 `manage_router`。

**schema 事实**：`archives` 只有 30 列、**没有 `status`**；`archive_drafts` 有 31 列（含 `status`）。
内容在两张表之间用 `REPLACE ... SELECT` 搬运（`archive_admin.rs` 的 `ARCHIVE_COLUMNS` / `DRAFT_COLUMNS`）。

**实测证据**：`cargo build` 无告警；`cargo test` **106 项全绿**。

#### 2.1.2 设置 / 设计 / 站点 / 升级（7 模块）

| 模块 | 能力 | 已知边界 |
|---|---|---|
| 统计 | `statistic/spider`、`traffic`、`include` 支持 `days` 参数（1~365，默认 30；前端工具条切换 7/30/90 天）；`dates`（`data/statistic/*.log` 只列 `YYYYMMDD` 日期文件）、`detail`（按行解析日志、按 `type` 过滤、`q` 关键词匹配 URL/IP/域名/UA/蜘蛛/设备、倒序分页，日期参数校验防路径穿越）、`detail/export`（CSV，UTF-8 BOM）、`include/detail`（`spider_includes`，当天无快照时补采一次）、`include/collect`（POST 手动采集，互斥锁防重复）。采集链路：前台请求写 `data/statistic/<日期>.log`（蜘蛛/设备/URL/UA 截 250 字节）→ 后台每 10 分钟聚合到 `statistic_logs` → 每天 8 点（服务启动时补一次）抓取五大引擎收录数到 `spider_includes`，并清理 31 天前明细日志。前端增强见 `public/system/statistic-entry.js`（天数切换/关键词搜索/CSV 导出/手动更新收录工具条，通过 hook XHR/fetch 给接口附带参数） | — |
| 设置 | 23 个动作：`system` / `content` / `index` / `contact` / `safe` / `ai` 读写（字段级 merge）、`cache`（读清理时间 + 切换 `cache_type`）、`diyfield`（数组整份覆盖）、`favicon` 上传 / 删除、`banner` 增改删 | 缩略图重建未做；`AutoMigrateDB` 由 2.9 的 `ensure_columns` 承接 |
| 会员认证 | `plugin/userauth/{list,detail,audit}`（`user_auths` 表，`0/1/-1` 状态，驳回必填原因，记 `audit_time` / `audit_admin_id`） | — |
| 站点 | `website/{list,info,save,delete}` + `siteinfo`（`websites` 表 CRUD，站点 1 的 mysql 取 `config.json` 且抹掉密码） | `website/login`（子站切换登录）与真正的多站点数据隔离未实现——当前单连接池 |
| 升级 | `version/env`（版本 / 平台 / 主程序名 / 路径 / 上传上限 / 跳过规则） | `version/check`、`version/upgrade`、`version/upload-upgrade` 见 2.5 |
| 设计 | `design/{list,info,save,use,delete,upload/check}`；模板文件 `file/{info,save,copy,delete,upload,restore,histories,history/info,history/delete,templates}`；`docs` / `helpers` 返回空数组 | zip 打包下载 / 上传解压（见 2.5）、演示数据备份初始化未做 |
| 云端服务 | 见 2.1.6 | 见 2.1.6 |

**实测证据**：`cargo build` 无告警；`cargo test` **133 项全绿**。

#### 2.1.3 SEO / 收录 / 链接类插件（15 模块）

| 模块 | 能力 | 已知边界 |
|---|---|---|
| 伪静态 | `plugin/rewrite` 读 / 写（`settings.rewrite`，`{mode, patten}` 字段级 merge） | 自定义正则模式见 2.3 的「已知差异」 |
| robots | `plugin/robots` 读 / 写；内容存**文件** `public/robots.txt`，文件缺失时回默认内容 | — |
| sitemap | `plugin/sitemap` 读 / 写（`type` 归一为 `xml`/`txt`，切格式时自动清理旧文件）+ `plugin/sitemap/build` 真生成 `public/sitemap.{xml,txt}` 并写 `cache/sitemap-time.log` | 分片与自动重建见 2.5 |
| 友情链接 | `plugin/link/{list,detail,delete}`（`links` 表，保存时 `status` 强制归零等回链检查） | `plugin/link/check` 未实现（需 HTTP 客户端） |
| 关键词 | `plugin/keyword/{setting,list,detail,delete,export,import}`（`keywords` 表 + `settings.keyword`）：批量新增自动去重、单条更新查重、单/批量/全量删除、导出表头+数据、导入按行解析 `title[, category_id]` | — |
| 标签 | `plugin/tag/{list,detail,delete,fields}`（`tags` + `tag_contents` + `tag_data`）：标题模糊搜索 + 分类筛选、`partial` PATCH 语义、别名自动去重加后缀、首字母推导、正文/自定义字段 upsert、字段定义存 `settings.tag_fields` | — |
| 301 跳转 | `plugin/redirect/{list,detail,delete,import}`（`redirects` 表，`from_url` 唯一）：地址自动补前导 `/`、来源≠目标校验、同来源自动复用记录、导入按行 `from, to` | — |
| 锚文本 | `plugin/anchor/{list,detail,delete,replace,export,import,setting,add-from-title}`（`anchors` 表，`title` 唯一）：关键词搜索、链接剥前台域名前缀、密度 <10 归一 100、批量替换把正文里**未被链接**的词包成 `<a>`、可从分类/内容/标签标题批量生成锚文本 | — |
| 全站替换 | `plugin/replace/values`：按 14 个「位置」批量替换；正文字段用 Rust 扫描器**尊重 `replace_tag`**（false 时跳过 `<...>` 内内容），其余字段走 SQL `REPLACE` | — |
| 时间因子 | `plugin/timefactor/setting` 读 / 写（`settings.time_factor`）：默认值补齐；保存时丢弃 `update_running`、**保留 `today_count` / `last_sent` / `today_update` / `last_update` 等运行期计数** | — |
| JSON-LD | `plugin/jsonld/config` 读 / 写（`settings.json_ld`）：返回前把 38 个字段的默认值补齐，保存走字段级 merge | — |
| 站长验证 | `plugin/google/setting` 读 / 写（`settings.google_auth`）：`redirect_url` / `client_id` / `client_secret` | — |
| 模板包分发 | `plugin/transfer/task` 恒空（内存态）；`plugin/transfer/download` **真实可用**（校验 provider 名、防穿越、按 `train→.wpm` 映射、作为附件返回） | `create` / `modules` / `start` 需 HTTP 拉远端数据，未实现 |
| 多语言 | 10 个动作可用：`config` 读写（`default_language` 回写 `system.language`、`site_type` 空值归一、`single` 模式自动剔除默认语言子站）、`sites`（补 `websites` 名称/根目录）、`validsites`、`site/save`（按 id 或 language 去重）、`site/remove`（只删配置）、`sync/status`（恒无任务）、`site/html/{logs,caches,cache/remove}`（扫 `cache/multiLang`） | `site/sync`（跨站点复制 + 翻译外呼）由 2.10 补（附件复制 + `auto_translate`） |
| 标题图 | `config` 读写（尺寸/字号归零回落 800/600/32）、`upload` **真实可用**（字体只收 `.ttf` 且固定落 `uploads/titleimage/title_font.ttf` 并回写 `font_path`；背景图按 `md5(原文件名)` 命名且限 6 种图片后缀） | `preview`、`generate` 由 2.8 补齐（真光栅化） |

新增：`src/models/plugin.rs`（rewrite / robots / sitemap / timefactor / jsonld / google）、
`src/models/link.rs`（友情链接）、`src/models/tag.rs`（标签）、`src/models/redirect.rs`（301 跳转）、
`src/models/anchor.rs`（锚文本 + 批量加链接）、`src/models/replace.rs`（全站替换 + HTML 感知扫描器）、
`src/models/collector.rs` 扩展（关键词 CRUD + `settings.keyword`）。

**实测证据**：`cargo build` 无告警；`cargo test` **166 项全绿**（新增 39 项：plugin 6 + link 1 + tag 3 + redirect 3 + anchor 4 + replace 6 + transfer 3 + multilang 3 + titleimage 2 + 路由端到端 8）。

**schema 事实**：`archive_data.id` 是 `bigint unsigned`、`tag_contents.id` 是 `int unsigned`。
sqlx 按无符号解码，**直接取成 `i64` 会整条查询失败**（且容易被 `Err(_) => 跳过` 静默吞掉）。
统一用 `CAST(id AS CHAR)` 再解析。

#### 2.1.4 内容安全 / 互动 / 增强类插件（15 模块）

| 模块 | 能力 | 已知边界 |
|---|---|---|
| 敏感词 | `setting/sensitive/{words,check,sync}`：数组配置 + 三种规则（`词` / `词\|替换词` / `{正则}[|替换词]`，用 `regex` 依赖真跑正则）、命中检测、等长 `*` 掩码 | `sync` 需外呼，未实现 |
| 评论 | `plugin/comment/{list,detail,delete,check}`（`comments` 表）：列表补 `item_title`、改内容**强制转义并置为正常**、单条切换/批量置状态、删除时扣减 `comment_count` | — |
| 留言 | `plugin/guestbook/{list,detail,status,delete,export,setting}`（`guestbooks` 表）：关键词（用户名/联系方式/内容）+ `default/ok/spam` 三态筛选、单/批量改状态与删除、导出按配置字段顺序拼表头（末 5 列固定 Created Time/IP/Refer/Site Id/Spam）、配置 `fields` 按 `field_name` 去重 | — |
| 订阅用户 | `plugin/subscriber/{list,detail,save,delete,category/list,category/save,category/delete}`（`subscribers` + `subscriber_categories`）：邮箱必填、分类名唯一 | `send` / `send/status` 的 SMTP 由 2.9 承接；`send/status` 缺 `job_id` 时给出可执行的下一步提示 |
| 会员管理 | 12 个动作：`user/fields` 读写（字段名 `^[a-z][0-9a-z_]+$` 校验、去重、`required` 强制 false）、`user/field/delete`（同步 `ALTER TABLE users DROP COLUMN`）、`user/list`（id / 分组 / 用户名 / 真实姓名 / 手机号 / 综合 `q` / 三态 status / `subscribed·ordered·repurchase` 筛选）、`detail` 读写（PATCH 语义 + bcrypt + 用户名查重）、`balance`（±金额、备注必填）、`delete`（连带清 `user_auths`）、用户组 list/detail/save/delete（名称唯一、组内有会员拒绝删除） | — |
| 验证文件 | `fileupload/{list,upload,delete}`：只收 `txt/htm/html/xml`、拒绝路径分隔符与点开头、落在 `public` 根、清单按 `md5(文件名)` 登记（同名不重复），列表补 `link` | — |
| 素材 | `material/{list,detail,import,delete,category/{list,detail,delete},convert/file}`（`materials` + `material_categories`）：分类名唯一、分类下还有素材拒绝删除、保存/删除后自动重算 `material_count`；`convert/file` 支持剥 BOM + 去标签压空白 | **非 UTF-8 明确报错**（需转码库） |
| 水印 | `watermark/config` 读写（尺寸/位置/透明度/最小边归零回落，默认 20/9/100/400/`#ffffff`）、`watermark/upload` **真实可用**（字体只收 ttf 固定名，图片 6 种后缀固定名） | `preview`、`generate` 由 2.8 补齐 |
| 干扰码 | `interference/config` 读写（`open` / `mode` / 禁选择·禁复制·禁右键） | — |
| 限流器 | `limiter/setting` 读写（白/黑名单、最大请求数、封禁时长、UA 黑名单、内存限制等 12 项） | 真正的限流器由 2.9 补（`LimiterRuntime`） |
| HTML 缓存 | 13 个动作里可用 5 个：`config` 读写（`storage_url` 自动补尾部 `/`、`keep_local` 恒定 false）、`build/status` 与 `push/status`（恒 `null`）、`push/logs`（恒空）、`clean`（**真删** `<cache>/html` 并回条数）、`upload/file`（SSH 私钥落 `<data>/cert/htmlcache_ssh_private_key.key`） | `build` 与 `build/{index,category,archive,tag}`、`push` 由 2.5 补齐 |
| Akismet | `akismet/setting` 读写（`open` / `api_key` / `check_type[]` / reCAPTCHA 三字段） | — |
| 全文索引 | `fulltext/config` 读写（引擎 / 地址 / 账号 / 评分 / 包含长度等 12 项）、`status`（恒 `running:false`） | `rebuild` 未实现（需索引引擎） |
| 导入 API | `import/api` 读 + `import/token` **真生成**两个 32 位随机 token 并落库 | — |
| AI 生成 | `aigenerate/setting` 读写（`settings.ai_generate`）、`plans`（恒空） | `checkapi`、`article/{collect,start}` 未实现（需外呼，见 2.10） |

**Part 4 覆盖 15 / 15**（其中 7 个为「配置可用、动作明确未实现」；其中 3 个的「未实现」部分后续已被 2.5 / 2.8 / 2.9 补掉）。

新增：`src/models/sensitive.rs`（敏感词规则 + 掩码）、`src/models/comment.rs`（评论 CRUD + HTML 转义 + `&nbsp;` 折叠）、
`src/models/guestbook.rs`（留言 + 导出）、`src/models/subscriber.rs`（订阅用户 + 分类）、
`src/models/user_admin.rs`（会员 / 用户组 / 扩展字段）、`src/models/upload_file.rs`（验证文件）、
`src/models/material.rs`（素材 + 分类 + 文本转换）、`src/models/watermark.rs`（水印配置 + 资源上传）。

**实测证据**：`cargo build` 无告警；`cargo test` **195 项全绿**（新增 29 项：sensitive 4 + comment 2 + guestbook 2 + subscriber 2 + user_admin 5 + upload_file 2 + material 4 + watermark 2 + plugin 1 + 路由端到端 5）。

**schema 事实**：`subscribers` / `subscriber_categories` 的 `id` 与时间列都是**有符号** `int(11)`，
必须映射成 `i32`；取成 `u32`/`i64` 会解码失败（与 `archive_data.id` 的无符号坑正好相反）。

#### 2.1.5 商业 / 交易 / 集成（16 模块）

| 模块 | 能力 | 已知边界 |
|---|---|---|
| 城市站 | `plugin/place/{setting,list,detail,delete}`（`places` 表 + `settings.place`）：树形/平铺双模式、`CAST(latitude AS CHAR)` 规避 `decimal(10,6)` 解码、别名自动去重加后缀、上级循环引用校验、删除连带整棵子树、`url_type`/`content_type` 非法值归一 | — |
| 推送 | `plugin/push` 读写（`settings.push`，`logs` 不被前端覆盖）、`plugin/push/logs`（最近 20 条）、`plugin/push/push` 先记一条失败日志再明确报错 | 真正调用搜索引擎推送接口由 2.10 补齐（Google 分支） |
| 存储 | `plugin/storage` 读写（`storage_url` 补尾部 `/`、FTP/SSH webroot 去尾斜杠、`keep_local` 由用户决定）+ `plugin/storage/upload`（SSH 私钥落 `<data>/cert/ssh_private_key.key` 并回写配置） | — |
| 小程序 | `plugin/weapp/config` 读写（`settings.weapp`），`server_url` 派生为 `base_url/api/wechat` 不落库 | — |
| 微信 | `plugin/wechat/config` 读写（`settings.wechat`）+ `message/{list,delete,reply}` + `reply/rule/{list,save,delete}`（默认规则唯一）+ `menu/{list,save,delete}`（名称 ≤20 字符预校验、删除连带子级） | `message/reply` 真推送、`menu/sync` 由 2.7 补齐 |
| 订单 / 财务 / 分销 / 佣金 / 提现 / 支付 | `/plugin/order/{list,detail,pay,deliver,finished,canceled,refund,refund/apply,export}`、`/plugin/finance/{list,detail}`、`/plugin/retailer/{list,detail,config,realname,apply}`、`/plugin/commission/{list,detail}`、`/plugin/withdraw/{list,detail,approval,finished,apply}`、`/plugin/pay/{accounts,detail,delete,upload,statistic}` | 真实支付扣款 / 提现打款的外呼由 2.6 补齐 |
| 邮件 / 备份 | `/plugin/sendmail/{list,setting,templates,template,template/preview,test}`（授权码掩码回传=未修改）、`/plugin/backup/{list,dump,status,restore,delete,export,import,cleanup,remark}`（SQL 逻辑备份真实现） | SMTP 发信由 2.9 承接；zip 备份包见 2.5 |
| 翻译 / LLMs | `/plugin/translate/{config,logs,log/texts,log/text/remove,log/text/save}`（译文修正同步替换 `cache/multiLang` 缓存，`ReplaceTranslateText` 4 组正则）、`/plugin/llms/{status,build,setting}`（**真生成** `public/llms.txt` + `llms-full.txt`，前 1000 条正文导出） | 翻译 API 外呼由 2.10 补齐；`HtmlToMarkdown` 为简化版 |
| 云端服务 / AI 助手 | 见 2.1.6 | — |

**实测证据**：`cargo check --all-targets` 无告警；`cargo test` **202 项全绿**（新增 7 项：place 3 + wechat 2 + 路由端到端 2）。

**schema 事实**：`wechat_menus.name` 是 `varchar(20)`、`type` 是 MySQL 关键字——
Rust 侧字段改名 `kind` 且要同时 `#[serde(rename = "type")] #[sqlx(rename = "type")]`，否则
`FromRow` 按字段名找列 `kind` 会查空；名称超长要在入库前预校验，别让用户看到 MySQL 1406。

#### 2.1.6 云端服务与 AI 助手

**云端服务（`/system/api/anqi/*`）**：

| 路由 | 分流 | 说明 |
|---|---|---|
| `GET /info`、`GET /check` | ✅ 真实 | 读 `settings.anqi`（账号配置全字段默认值补齐），`valid = expire_time > now` 现算 |
| `POST /login`、`POST /upload`、`POST /template/share`、`POST /template/download`、`POST /feedback`、`POST /extract/keywords`、`POST /extract/description` | ✅ 真实（2.10 补齐） | 云端登录 / 上传 / 模板分享 / 关键词与摘要提取 |
| `POST /translate`、`POST /ai/pseudo` | ✅ 真实（2.10 补齐） | 文档/草稿存在性、目标语言是真实站内校验；翻译与伪原创本体走 2.10 的引擎 |
| `POST /ai/stream`、`GET /ai/stream/data` | ⚠️ 半实现 | stream 产生端未实现；`stream/data` 对未知 `stream_id` 恒回 `finished` |
| `POST /restart` | ❌ 未实现 | 无守护重启通道 |

**AI 助手（`/system/api/anqi/ai/*`，`code:0/-1` 特例口径）**：

| 路由 | 分流 | 说明 |
|---|---|---|
| `GET/POST /ai/settings` | ✅ 真实 | `settings.ai_setting`：`configs` 数组整份替换、`last_model`/`mcp` 保留，默认值补齐 |
| `GET /ai/history`、`GET /ai/sessions` | ✅ 真实 | **DB 版实现**：`ai_chat_messages` 按 `seq ASC` 重建（`files` JSON 解析）；`/sessions` 聚合（title=首条用户消息 ≤50 字、last_message ≤100 字、msg_count） |
| `GET /ai/agents`、`GET /ai/agents/:id/logs` | ✅ 真实 | `ai_agents`（id ASC）/ `ai_agent_logs`（agent_id 过滤、id DESC 分页，不回 total） |
| `POST /ai/upload` | ✅ 真实 | 附件落 `<cache>/ai/upload/<session_id>/<时间戳>_<文件名>`，会话 ID 安全字符过滤 |
| `POST /ai/image/confirm`（action=0） | ✅ 真实 | 舍弃是 no-op；action=1 保存需下载远端图片，未实现 |
| `GET /ai/image/histories` | ⚠️ 恒空 | 历史存云端服务侧，拉取失败也返回空，照实返回 `total:0, data:[]` |
| `POST /ai/chat`、`POST /ai/agents/:id/chat`、`GET /ai/health`、`GET /ai/chat/trace` | ⚠️ 需配置模型服务 | 未内置 LLM 服务（等价「模型服务未配置」分支）；agent/chat 的「智能体不存在」「消息不能为空」是真实站内校验 |
| `POST /ai/chat/confirm` | ⚠️ 半实现 | 参数校验齐全（decision 四值白名单）；无对话即无待审批，恒回 `no pending approval` |
| `POST /ai/image` | ✅ 真实（2.10 补齐） | 空提示词校验真实；生图走 2.10 的云端 / OpenAI 兼容分支 |

新增：`src/models/cloud.rs`（云端登录态 + AI 配置 + 智能体 + 会话消息）。
鉴权：云端分组的全部路由均先 `require_admin`。

**schema 事实**：MySQL 的 `SHOW TABLES LIKE ?` 不支持参数占位符（报 1064），
检查表是否存在要用 `information_schema.tables WHERE table_schema = DATABASE AND table_name = ?`。
另外 `ai_chat_messages.seq` 是 `int(10) unsigned`，取 `i32` 会整条查询失败——与 2.1.3 的无符号列坑同源。

**完成度结论**：63 个后台模块全部覆盖；配置读写 / 列表 / 状态类接口全部真实实现；
依赖外部服务的动作按统一口径「接口保留 + 参数校验 + 明确未实现提示」。
验证基线：`cargo check --all-targets` 无告警；`cargo test --test-threads=1` **236 项全绿**（本批新增 6 项：云端服务模型 4 + 路由端到端 2）。

### 2.2 通用约定与横切事实

1. **路由前缀**：后台接口挂 `/system/api/...`；插件类接口形如 `/system/api/plugin/<plugin>/...`。
2. **鉴权**：绝大多数业务路由「解析令牌 + 校验管理员权限」（Rust 侧 `require_admin`，权限暂按 `group_id==1` 超管兜底）；
   `captcha` 只校验 URL 令牌；`version/*`、云端服务路由只校验登录令牌。
3. **配置存储**：几乎全部插件配置存 `settings` 表（`key VARCHAR(190)` 主键 + `value LONGTEXT` 存 JSON 字符串），
   没有独立 config 表。统一入口 `src/models/setting.rs`（`get_setting_value(key)` / `save_setting_value(key, json)`）。
   - 例外：`robots.txt` 存文件 `public/robots.txt`；sitemap 更新时间存文件 `cache/sitemap-time.log`；多语言 HTML 缓存存文件 `cache/multiLang/`。
4. **分层**：控制器层只做「参数绑定 + 信封」，实体逻辑落在 `models/*.rs`。
5. **部分更新语义**：大量 `*ConfigForm` 保存用「读现配置为基底 + 字段级 merge 只覆盖请求里出现的字段」，
   在 `serde_json::Value` 层实现，**不能用整块覆盖**。
6. **列表响应**：分页接口顶层额外带 `"total":N`（**不是**放在 data 里）。
7. **AI 接口特例**：成功/失败用 `code:0` / `code:-1`，与普通插件的 `0/1` 不同。
8. **金额单位**：一律 `int64`（分）。
9. **每个写动作记操作日志**：复用 `models/admin.rs::record_action`。

公共依赖：`require_admin`、`ok/ok_msg/fail/failed`（`routes.rs`）、`models/setting.rs`、`admin::record_action`。

#### 插件配置读写的两条硬规则

- 配置统一走 `models/site_setting.rs`（字段级 merge）；**数组型配置必须用 `get_array`，`get_section` 只认对象**。
  用错时表现为「写入成功、读回永远是空数组」，很容易误判成保存逻辑有问题。
- 上传类统一「临时 `public_dir` + 扩展名白名单 + md5 命名」；
  表格 `id` 若是无符号，取 `i64` 前先 `CAST(id AS CHAR)`；`decimal` 列同理先 `CAST(... AS CHAR)`。

### 2.3 前台渲染管线

| 环节 | Rust 实现 |
|---|---|
| 兜底入口 | `front::reroute`（`routes.rs::router` 的 `.fallback`，`/api`、`/system/api` 精确路由优先） |
| 301 检查 | 查 `redirects` 表（全 URI 精确匹配），命中且目标≠自身 → 301 |
| 会员令牌解析 | header `token` > `Authorization: Bearer` > cookie `token`；JWT 有效且会员启用时注入 `userId/userInfo/userGroup` 到模板上下文；过期前 1 小时回 `update-token` 续签头；JWT 无效回退 `users.api_token`；**账号被禁用直接回 `{"code":1001,"msg":"ThisAccountHasBeenDisabled"}` 并中断** |
| 整管线超时 | `tokio::time::timeout(30s)`，超时 408 |
| 静态文件服务 | `public_dir`（`config.json`，默认 `../public`）下的真实文件按扩展名回 Content-Type；逐段防路径穿越；`favicon.ico` 缺文件回 400；`robots.txt` 缺文件回默认内容。**放在 DB 检查之前**（静态资源不依赖库） |
| 商家工作台 | 精确路由 `/business`（301 到 `/business/`）、`/business/`（出 `public/business/index.html`）、`/business/*rest`（转 `serve_file` 直出子资源，缺失 404）。页面与静态资源都在 `public/business/` 下，随完整安装包分发（对齐 anqicms 3.6.9；axum 0.7 的 catch-all 不命中 `/business/`，须显式注册） |
| 闭站 / 屏蔽判定 | `settings.system.site_close`（1=全关，2=仅蜘蛛可见）、`ban_spider`、`safe.ua_forbidden`、`safe.ip_forbidden`（前缀匹配、去掩码与 `.0`、放行 127.0.0.1）；闭站页渲染 `errors/close.html` 或纯提示，403 |
| 路由解析 | 规则顺序：search → archiveIndex → peopleIndex/people → tagIndex → tag → page → category → archive；每条命中都做存在性校验（module 按 `url_token`、category 按 type/status、archive 校验 module/catname 对应关系，对不上直接 404）；固化链接 `archives.fixed_link` 优先 |
| 404 / 500 | `errors/404.html` / `errors/500.html` 渲染失败时退回内联 HTML 提示页 |

各页面模板选择优先级：

- 首页：`index/index.html` → `index.html`；
- 模块列表：`{表名}/index.html` → `{表名}_index.html`；
- 栏目页：栏目 template → `{表名}/{url_token}.html` → `{表名}/list-{id}.html` → `{表名}/list.html` → `{表名}_list.html`；
- 单页：栏目 template → `page/{url_token}.html` → `page/detail-{id}.html` → `page-{id}.html` → `page/detail.html` → `page_detail.html`；
- 详情页：内容 template → 栏目 detail_template → `{表名}/detail-{id}.html` → `{表名}/detail.html` → `{表名}_detail.html`；
- 标签列表：`tag/index.html` → `tag_index.html`；标签页：`tag/list-{id}.html` → `tag/list.html` → `tag_list.html`；
- 搜索页：`search/{模块token}.html` → `search/index.html` → `search.html`。

新增 / 修改：

- **新增 `src/front.rs`**（前台管线 + 测试 8 项）；`routes.rs` 删除旧的 `front/index/render_*`（约 200 行），`.fallback` 指向 `front::reroute`，`/preview/list/:cid` 复用 `front::render_category_preview`；
- `src/models/url.rs`：补 `Search` / `PeopleIndex` / `People` 三类默认规则（对所有模式统一补齐）、`tag_url` / `tag_index_url` 生成器；
- `src/models/module.rs`：新增 `get_by_token`（前台按 `url_token` 匹配模块）；
- `src/models/archive.rs`：`ArchiveQuery` 新增 `tag_id` 筛选（`tag_data` 关联表）；
- `src/template/site_provider.rs`：新增 `keyword`（搜索过滤）、`tag`/`tag_index` 上下文、`tagList`/`tagDetail` 标签实现、tdk 的标签分支、分页链接的标签形态；
- `src/template/mod.rs`：for 循环支持遍历字符串（`tag/index.html` 需要）。

**已知差异（均有注释，不静默）**：

- 自定义正则模式（mode 4）在 `settings.rewrite.patten` 为空时回落字符串模式 2 的规则（既有约定，真实库即此配置）；`patten` 非空的解析未实现；
- 多语言子站、城市分站、HTML 缓存、蜘蛛访问日志、手机端模板（`mobile/` 前缀与 301）、sitemap/robots 自动生成——其中 HTML 缓存与 sitemap 已在 2.5 补齐，其余仍在待办；
- `/people`、`/peoples` 规则能匹配但明确提示「会员主页尚未开放」。

**实测证据**：`cargo check --all-targets` 无告警；`cargo test --test-threads=1` **247 项全绿**（新增 11 项）。

### 2.4 前台交互 API（五批并行）

| 批次 | 领域 | API 文件 | 数据层文件 | 端点 |
|---|---|---|---|---|
| A | 前台内容只读 + 站点信息 | `src/api_content.rs`（56KB） | `src/models/front_content.rs`（43KB） | `/archive/*` 7、`/category/*` 2、`/module/*` 2、`/page/*` 2、`/tag/*` 3、`/place/*` 2、`/nav/list`、`/metadata`、`/languages`、`/banner/list`、`/friendlink/*` 4 |
| B | 会员 / 账户 / 认证 | `src/api_member.rs`（94KB） | `src/models/member.rs`（70KB） | `/login` `/logout` `/register` `/forget` `/password/reset` `/verify/email` `/captcha` `/account/*` `/member/*` `/user/*` 20 `/sign/*` `/subscription` `/attachment/upload` |
| C | 购物车 / 订单 / 店铺 / 支付 / 分销 | `src/api_trade.rs`（44KB） | `src/models/trade.rs`（120KB） | `/cart/*` 6、`/order/*` 9、`/orders`、`/payment/check`、`/point/logs`、`/shop/*` 7、`/retailer/*` 9、`/user/recharge`、`/user/withdraw`、`/alipay/pay`、`/paypal/*` |
| D | 评论 / 留言 / 收藏 / 微信 / 小程序 | `src/api_interact.rs`（53KB） | `src/models/interact.rs`（38KB） | `/comment/*` 3、`/favorite/*` 4、`/guestbook.html`、`/guestbook/fields`、`/wechat*` 3、`/weapp/*` 2、`/google/url`、`/log` |
| E | 后台 API 缺口 | `src/api_admin_gap.rs`（26KB） | `src/models/admin_gap.rs`（35KB） | `/system/api/password/*` 3、`/system/api/archive/*` 8、`/system/api/attachment/addurl`、`/system/api/anchor/addfromtitle`、`/system/api/htmlcache/upload`、`/system/api/setting/convert/webp` |

合计新增约 580KB（10 个文件）。

**合流方式**：各批在自己的 `api_*.rs` 暴露 `pub fn router -> Router<AppState>`（用绝对路径），
主线在 `src/routes.rs::router` 里 `.merge(...)` 五次后交给 `front::reroute` 兜底。
公共件（`ok` / `ok_msg` / `fail` / `failed` / `NO_DB` / `model_error` / `bearer_token` / `require_admin` / `api_not_found`）
统一提升为 `pub(crate)` 供各批复用；`src/models/mod.rs` 预先声明 5 个新模块，避免并行改同一文件。

**本轮修掉的真问题**：

1. **支付接口越权**：`api_trade.rs` 的 `/order/payment` 取到 `user_id` 却没用 → 登录用户可支付他人订单。
   已补 `if user_id != 0 && order.user_id != user_id { return fail("权限不足") }`（匿名下单 `user_id=0` 放行），
   与同文件 `/order/detail` 的归属校验同口径。
2. **管理员端点前缀不对**：原来只挂在公开 `/api/admin/*`，规范前缀是 `/system/api/admin/*` 外加 `/system/api/login`。
   已在 `manage_router` 里补齐 13 条，两套前缀行为一致。
3. **htmlcache 证书上传缺两步**：落盘后还要把私钥文件名回写 `html_cache.ssh_private_key` 并记一条后台操作日志。
   已补 `site_setting::save_section(pool, plugin::HTML_CACHE, …)` + `admin::record_action(...)`，
   响应信封统一为 `{code:0, msg:"文件上传完成", data:fileName}`。

### 2.5 升级包、备份、sitemap、HTML 缓存

#### 2.5.1 升级包上传与解压

- `POST /system/api/version/upload-upgrade` 真实实现。
- 表单：`file`（按魔数识别 zip / tar.gz）、`version`、`force=1`、`checksum`（可选）、`restart=1`（可选）；
  单路由 `DefaultBodyLimit` 放宽到 512MB + 1MB。
- 校验口径：包内 `version.json` 可缺省（缺省且非强制时报错）、版本不高于当前且非强制时拒绝；
  显式 `checksum` 时按 `sha256:<hex>` 或裸 64 位 hex 校验包体；剥离公共顶层目录；
  解压上限 2GB / 20000 条；主程序写 `.new`，成功后备份并替换。
- **覆盖规则**：`template/` 与 `public/` 属于发行产物，升级时整体替换；
  只保护 `config.json` / `config.sample.json` / `favicon.ico` / `version.json`、
  `data/ uploads/ logs/ cache/ tmp/ upload/ public/upload/ public/uploads/` 与
  `*.sh/*.bat/*.cmd/*.ps1/.bak/.old/.new`。
- 版本口径统一：`version/info`、`version/env`、`version/check`、升级接口都读 `deployed_version()`
  （程序目录 / 当前目录 `version.json` 优先，回退编译期版本号）。
- 在线升级 `version/upgrade` 由 `version.json` 清单驱动：按平台优先取 `packages.<windows|linux>`，
  退回顶层 `url`；相对地址按清单所在目录解析；下载后强制 sha256 校验，再复用同一解压安装链路。
  成功后写站点 `version.json`、按清单 `sql` 执行迁移（失败只记日志不阻断）、
  生成自升级脚本，配置 `auto_restart` 或请求 `restart=1` 时自动执行。
- 发行产物由 `deploy/release/pack-release.ps1` 生成，详见 `deploy/upgrade.md`。

#### 2.5.2 备份包（dump / restore）

- `models::backup::restore` 增加 `template_dir` / `public_dir` 参数；`.zip` 先还原 `template/`
  到模板目录、`static/` 到 `public/static/<模板目录名>`，再执行 `database.sql`。
- 包内路径逐段校验，拒绝 `..`、绝对路径与盘符；`.sql` / `.zip` 共用 `execute_sql`（`_logs` 语句仍跳过）。
- `models::backup::dump` 生成 `<时间戳>.zip`，包含 `database.sql` + `template/` + `static/`；
  跳过 `_logs` 表；`template/` 来自当前模板目录，`static/` 来自 `public/static/<模板目录名>`；
  `setting::migrate_db`、后台备份、设计市场初始化三处调用同步更新。
- 顺带修掉旧实现的分片写盘 bug（`append` 后的尾块 `fs::write` 会覆盖前文）。
- zip 条目现已用 deflate 真压缩（见 2.9），旧 `.sql` 备份仍可恢复；列表 / 备注 / 导入继续兼容 `.sql` 与 `.zip`。

#### 2.5.3 sitemap 生成

- `front::serve_file` 增加旧路径重写：`/archive*`、`/tag*`、`/category*` 的 `.xml/.txt`
  改写为 `/sitemap-...`。
- `sitemap.xml` / `sitemap.txt` 文件不存在时，按 `settings.sitemap` 的 `type` 自动调用
  `plugin::sitemap_build` 并返回产物；未配置 `settings.sitemap` 时保持原 404 行为。
- 分片结构：
  - `sitemap-category.{type}`：首页 + 栏目 / 单页（`exclude_*` 源头过滤）；
  - `sitemap-archive-{N}.{type}`：文档按 `page_size` 分片；
  - `sitemap-tag-{N}.{type}`：标签按 `page_size` 分片，`exclude_tag` 时整体跳过；
  - `sitemap.{type}`：index（xml 为 `<sitemapindex>`，txt 每行一个子文件 URL）。
- txt 子文件为 CRLF 连接的 loc（无末尾换行）；xml 子文件与 index **不再带 xsl 样式表引用**
  （对齐 anqicms 的 e203813，`public/rust-index.xsl` / `rust-style.xsl` 已删除）。
- 生成前清理同类型旧分片，避免分页减少后残留。
- `cache/sitemap-time.log` 内容为 Unix 秒，mtime 继续被 `sitemap_time` 读取。
- 前台 `sitemap*` 文件缺失时自动重建并返回（含子文件请求）。
- 已知差异：不生成空尾页；XML 暂不输出 `lastmod` 与多语言 `xhtml:link`。

#### 2.5.4 HTML 缓存前台命中

- 后台 build 本地生成到 `<cache>/html`；前台读取与回填：
  - 首页用 `index_cache`、详情用 `detail_cache`、列表 / 栏目 / 单页 / 标签用 `category_cache`；
  - 已登录会员、`Cache-Control: no-cache`、搜索 / 404 等不缓存；
  - 命中且未过期直接返回缓存 HTML；渲染成功后写回同路径。
- 缓存路径规则：`/` → `/index.html`，无扩展 → `<path>/index.html`，带扩展原样；query 统一忽略。
- 未实现：移动端 `cache/mobile`、query 独立缓存、内存二级缓存、远程推送。

#### 2.5.5 测试与基建（同批）

- sendmail 测试改为「把服务器指向 `127.0.0.1:1` → 断言发送失败」，不再断言旧的「未实现」文案。
- 升级上传测试覆盖：缺文件、非法格式、低版本 zip（版本校验在落盘前拒绝）。
- `xml_fields` 修复 CDATA 与嵌套容器解析（此前 `<xml>…</xml>` 会把整段吞成 `xml` 字段）。
- `test_pool` 支持 `RUSTCMS_TEST_CONFIG`，受限环境可把测试配置放在仓库内。
- 模板 `system.TemplateUrl` 统一为 `/static/<template_name>/`。
- 新增用例：`dump_writes_real_database_zip`（断言 zip 内 `database.sql` + `template/index.html` + `static/app.css`，需真实库）、
  `packs_directory_into_zip_entries`（纯离线，覆盖目录打包与条目名）、sitemap 分片三格式用例（纯离线）、
  HTML 缓存路径规则用例（纯离线）；静态文件测试覆盖 `/archive/x.txt` 的实际服务。

**实测证据（2026-11-07）**：

- `cargo check --offline --all-targets`：通过，0 告警。
- `cargo test --offline --no-run`：通过（约 2m30s ~ 3m）。
- `crates/ext`：42 项非网络测试全绿；7 项回环网络测试（HTTP/SMTP 本地监听）因本机沙箱
  禁止 `127.0.0.1` 连接而未执行，非代码缺陷。
- 主 crate 演进：**320 / 5 → 322 / 5 → 324 / 5**（passed / failed）。5 项失败均为环境限制：
  3 项依赖 MySQL（沙箱无法连 `127.0.0.1:3306`）、1 项依赖仓库外目录 `E:/CodeBuddy/clientFiles`（不可写）、
  1 项是依赖数据库的首页渲染。后由 3.1 节的脚手架修正消除。
- 构建环境备注：本机 gcc 的 cc1/库搜索路径失效，编译前需把
  `mingw64/libexec/gcc/x86_64-w64-mingw32/16.2.0` 加入 `PATH`、
  把 `mingw64/x86_64-w64-mingw32/lib` 等加入 `LIBRARY_PATH`；
  `RUST_MIN_STACK` 必须写字节数（如 `67108864`）。

> 说明：工作区曾出现外部进程的全库键名批量改名（含 API 路径、`settings` key、云端服务域名改写），
> 会破坏既有数据与接口兼容，该改动已回滚、未纳入提交。

### 2.6 支付网关（微信 V2 / 支付宝 / PayPal）

新增 `src/models/payment/{crypto,rsa,wechat,alipay}.rs`（约 2.1k 行）+ `src/models/payment.rs` 扩写。

- **密码学**：手写 SHA-256 / HMAC-SHA256（FIPS 180-4、RFC 2104 向量验证）与 MD5 封装（复用 `md-5`）；
  支付宝所需的 **RSA PKCS#1 v1.5 验签与签名**（u32 limb 大整数 + Montgomery CIOS 模幂 + ASN.1 DER，
  支持 `PUBLIC KEY`(SPKI) / `RSA PUBLIC KEY`(PKCS#1) / `CERTIFICATE`(X.509) 三种 PEM，私钥支持 PKCS#8/PKCS#1）。
- **微信 V2**：签名（去 `sign` 与空值、字典序、`&key=`、MD5 或 HMAC-SHA256 大写，`sign_type` 参与签名）、
  XML 构造/解析（CDATA + 实体）、Native 统一下单、JSAPI/小程序下单 + paySign、查单、沙盒 `getsignkey` 缓存。
- **支付宝**：待签串（去 `sign`/`sign_type`/空值 + 字典序 + 原值不编码）、RSA2 与 MD5 双路验签、
  `alipay.trade.page.pay` 跳转地址、RFC3986 编码。
- **PayPal**：`paypal_create`（OAuth2 → `/v2/checkout/orders`，返回 `approve_url` 并回写 `terrace_id`），
  capture 复用同一落库口径。
- **统一落库**：`payment::settle_gateway_payment` 一处收口 —— payments → `orders.payment_id` →
  finances（买卖双方流水，`after` 取当前余额）→ `trade::success_paid_order` → 账户统计；
  带 `WHERE paid_time = 0` 的 **CAS 幂等**，并发回调（微信重试 + 主动查单）不会重复写资金流水。
- `api_trade.rs`：`order_payment` 的四条「需要对接支付网关」占位全部换成真实分支；
  `alipay_notify` 重写（验签 → 幂等 → 退款只落 `order_refunds` → 落库 → 纯文本 `success`/`fail`）；
  新增 `wechat_notify`（验签 + **金额核对** + 退款分支 + XML 应答）、`wechat_query`（回调丢失兜底）；
  额外注册 `/notify/alipay/pay`、`/notify/paypal/pay`、`/return/paypal/pay|cancel`。
- 顺手修掉一个真实缺陷：账户选择在「全部账户都被日/月限额挡掉」时曾下标越界 panic，现退回默认账户。

### 2.7 微信生态（公众号 / 小程序）

- `models/wechat.rs`：`Section`（公众号/小程序各自 `settings` 键与缓存分区）、`access_token` /
  `weapp_access_token`（进程内缓存 + 单飞锁，`expires_in-300s` 取整、过期前复用，并发只取一次）、
  `invalidate_access_token`、`send_with_token`（`40001/40014/42001` 自动清缓存重试一次）、
  `push_text`、`menu_sync`、`weapp_qrcode_image`、`text_reply_xml`；
  `message_reply` 改成**推送失败即整体失败**（不再假成功）。
- 新增 `models/wechat/crypto.rs`：手写 AES-128（GF(2^8) 逆元现场生成 S 盒）+ CBC + PKCS#7（块 32）+
  `msg_signature` + 加解密信封（16 字节随机串 / 4 字节大端长度 / appid 校验）。
- 新增 `models/wechat/wxa.rs`：`<Encrypt>` 信封、`xml_fields`（CDATA/实体/自闭合/嵌套）、订阅消息两类事件解析。
- `models/interact.rs`：`save_wechat_user` 重写（openid 复用记录、`union_id` 关联已有会员、找不到才建会员；
  `users` 表无 `union_id` 列，关系只落 `user_wechats`，`IF(?='', union_id, ?)` 保住库里原值）、`save_subscribed`。
- `api_interact.rs`：`GET /wechat` 回显 `echostr`（原来回 `success`，公众号后台保存服务器配置会失败）；
  `POST /wechat` 支持明文与安全模式（验签 → AES 解密 → 分发 → **被动回复 XML**，加密模式回复也加密）；
  `/wechat/auth` 走 `code` → `sns/oauth2/access_token` → `sns/userinfo` → upsert，`state=code` 时生成验证码并下发；
  `/weapp/msg` 真解密分发（订阅消息两类事件落 `subscribed_users`）；
  `/weapp/qrcode` 调 `/wxa/getwxacode` 落盘 + 存储地址。
- 消息回调：自动回复落库后调用 `wechat::push_text` 推送到客服接口（未配置 AppID/Secret 时静默跳过）；
  小程序码落 `uploads/qrcode/<md5 前3>/<3..6>/<6..>.png`，写 `attachments` 与 `weapp_qrcodes`，返回可访问 URL。
- 微信支付回调三条入口（`/wechat/pay`、`/notify/wechat/pay`、`/wechat/notify`）共用 `api_trade::wechat_notify`；
  部署需放行 `/wechat/notify`、`/wechat/query`、`/alipay/pay`、`/paypal/pay`、`/notify/*`、`/return/*`。

**已知差异 / 剩余风险**：

- 微信 `code_url` 仍是原生链接（不引 `qrcode` crate），前端需用 JS 二维码组件出图。
- 微信 `EncodingAESKey` 的 AES-128-CBC **key 与 IV 都取前 16 字节**（官方实现同款），
  后 16 字节不参与运算 —— 改成「后 16 字节做 IV」会让所有微信回调解不开。
- 小程序码使用 `/wxa/getwxacode`，未使用 `getwxacodeunlimit`。
- 未配 `settings.system.base_url` 时下单直接报中文错误（不静默失败）。

### 2.8 图像处理（水印 / 缩略图 / 多格式 / 标题图文字）

- `models/titleimage/image_ops.rs`：PNG 编码加行过滤 + **自写 DEFLATE**（固定 Huffman + LZ77，环形哈希链，
  内存与图大小无关）；缩放改**预乘 alpha 双线性**。
- 新增 `image_ops` 同目录 `jpeg.rs`（基线解码：SOF0/SOF1、任意采样、DRI 重启；渐进式/算术/CMYK 明确报错）、
  `jpeg_encode.rs`（4:2:0 + 按频率生成最优 Huffman 表）、`gif.rs`（首帧解码：全局/局部调色板、交错、透明）、
  `background.rs`。
- 新增 `models/watermark/render.rs`：九宫格坐标、透明度、缩放、alpha 合成、预览 data URL（最长边限 800px）、落盘写回。
- `supports_extension` 仅 `png/bmp/jpg/jpeg`（GIF 只解码不回写，批量任务跳过）；WebP 只识别尺寸与是否动图。
- **实测（不是"能编译"）**：图像编解码 + 水印共 **47 个单测**（`models::titleimage` 37、`models::watermark` 10），
  实跑 `cargo test titleimage` 38 通过、`cargo test watermark` 10 通过，**0 失败**；
  并**与语言标准库双向交叉验证** —— 外部工具生成的 96×64 JPEG 能被本解码器解出（尺寸正确，平均绝对误差 2.95，
  属 q95 压缩损失）、本编码器产出能被标准库解码（1568B，误差 4.28，证明是合规基线文件）、PNG 往返无损、
  四色 GIF 图逐像素 0 处不符。
  性能（release）：JPEG 编码 1200×800 108ms、解码 130ms、cover 250×250 + 编码 18ms、PNG 250×250 编码 9.5ms。
- 水印缩放用 `resize_fit`（只缩不放，避免小 logo 被放大糊掉）；九宫格 2/4/6/8 补全；上采样用最近邻。
- **标题图文字**：由 `models/titleimage/draw.rs` 在位图上真实绘制（复用 `watermark::font` 的 TrueType 光栅化）：
  - `auto_font_size`：`可用宽度(画布宽-100) / 标题字数`，夹到 `[配置字号(≥16, 缺省 32), 100]`；
  - `split_words` / `wrap_lines`：CJK 逐字、ASCII 连写、`\n` 强制断行；按字体**真实 advance**
    测宽（不是「ASCII 半字号」估算），同一行不会超宽；
  - `draw_title`：行高 1.6、`int(size/1.5)` 首行补偿、逐行居中、基线定位、可选 `font_bg_color`
    底色带（alpha 混合）；
  - `load_face`：先读配置 `font_path`（相对 `public`），坏了/没配就退回系统字体
    （msyh/simhei/simsun/arial…）；
  - `render`：底图 + 文字一步出图（`face` 为 `None` 时只出底图）。
- `watermark::font::Face` 新增 `text_width`：与 `draw_text` 的推进口径一致（缺字形按半个字号留白），
  测量与实际绘制不会打架。
- 路由：`/system/api/plugin/titleimage/preview` 优先返回 `data:image/png;base64,`（预览即所得）；
  批量生成改落 `uploads/titleimage/generated/<md5>.png`。**一个字体都找不到时**才退回 SVG（`.svg`）兜底，
  所以老部署不会因为缺字体直接生成失败。
- 测试：`models::titleimage::draw` 6 例（颜色解析 / 切分不丢字 / 自动字号单调 / 折行不超宽 + 强制断行 /
  真画字：文字像素数量、纵向范围在画布内、底色带完全不透明、空标题只出底图 / render→PNG 编码往返），
  用系统字体，机器上没有字体时跳过，不依赖仓库资产；像素断言刻意用 ASCII 标题，
  只装了纯西文字体的机器也能跑。连库用例额外断言「PNG 预览的 base64 必须带 PNG 魔数」。
- 已知差异：折行按字切分（与带中文分词的实现相比，**折行位置可能略有不同**）；
  空颜色按白色处理（与本模块 SVG 预览默认色一致，而非透明）。

### 2.9 ext 能力层（deflate / 内存限流 / 表结构同步 / SMTP / HTTP）

#### 2.9.1 DEFLATE 与 ZIP

- `crates/ext/src/packer.rs`：raw DEFLATE 编码器（LZ77 + 固定/动态 Huffman，不可压自动回退存储块），
  zip 条目改 `method=8` 写入；读取侧支持任意合规 DEFLATE 流（含外部标准库产出的包）。
- **互通实测（同一份样本两边各压一遍）**：本实现产出的 deflate 被外部标准库完整还原（4/4 OK），
  外部 `BestCompression` 产出的流也被本实现还原（4/4 OK）；
  4.7MB 重复 SQL **5.7%**（对照 level=9 为 6.3%、level=6 为 6.4%，本实现更小）、
  模板 HTML 264KB **0.5%**（1251B vs 对照 1261B）、
  随机 200KB 与高熵 3.2MB **回退存储块**（100.0%，只多 20 / 245 字节块头；对照同数据多 70 / 985 字节）。
- 动态 Huffman 的关键是 **zlib 式码长限长修复**（溢出上提 + 按频次重分配）：没有它真实数据几乎总是超 15 位，
  会一直退回固定编码（这是"压缩率上不去"的隐藏根因）。
- 块头两处优化：码长表走 **RFC 1951 的 RLE**（0 的连续段用 17/18、非 0 重复用 16），
  且 HLIT/HDIST/HCLEN **裁到最后一个非 0**。原来固定写 316 个码长 + 19 个码长码（各 3 位），
  现在常见情况只剩十几个字节 —— 这是压缩率拉平（甚至略优）的主因。
- `models/backup.rs`：dump 产出的 zip 现在是**真压缩**。
- **遗留**：动态 Huffman 仍只有**单块**（异构数据多块还能再省一点，但要改 LZ77 分块与位预算，
  收益小、风险高，暂不做）。

#### 2.9.2 内存占用与限流

- `crates/ext/src/ratelimit.rs`：`MemoryInfo` / `system_memory` / `process_memory_bytes` / `memory_usage` /
  `mem_over_threshold`（Windows `GlobalMemoryStatusEx` + `K32GetProcessMemoryInfo`，Linux `/proc/meminfo` + `/proc/self/statm`）。
- `models/plugin.rs`（仅 limiter 段）：`mem_percent`（仅 `mem_limit` 打开时生效，夹 `50..=100`，非法回落 90）+
  5 秒粘滞 429。
- **限流器接线**：`plugin` 新增进程内 `LimiterRuntime`：
  `open / white_ips / black_ips / max_requests / block_hours / is_allow_spider /
  ban_empty_agent / ban_empty_refer`，名单支持 IPv4 CIDR；
  `front::reroute` 在进入渲染管线前调用 `limiter_check`：私网放行、白名单放行、
  黑名单/已封禁 403、超限 429；`/static`、`/uploads` 可单独禁空 Referer。
  `GET /plugin/limiter/blockedips` 返回真实的 `{ip, block_time}` 列表；
  `POST /plugin/limiter/blockedip/remove` 走真实封禁器。
  服务启动时按 `settings.limiter` 装载；保存设置后立即重建（重启后内存封禁列表清空，属既定行为）。
  `crates/ext` 的 `IpBlocker` 补 `is_blocked` / `blocked_entries`。
- Linux `/proc` 分支**已做真实类型检查**：`rustup target add x86_64-unknown-linux-gnu` 之后，把
  `ratelimit.rs` 抽成零依赖临时 crate 跑 `cargo check --target x86_64-unknown-linux-gnu`
  （整个工作区交叉 check 需要 linux gcc 编 `ring`，本机没有）。同时把 `/proc/meminfo`、`/proc/self/statm`
  的解析抽成**平台无关纯函数**并在 Windows 上单测（`MemAvailable` 优先、老内核退化 `MemFree`、
  脏数据夹回总量、缺列返回 `None`），不再只靠肉眼检查 cfg 块。

#### 2.9.3 表结构同步

- `crates/ext/src/tblsync.rs`：`SqlType` 逻辑映射、`auto_increment`/`after`、修掉
  `bigint(20) unsigned ≠ bigint unsigned` 的假 MODIFY、纯函数化（`add_column_sql`/`modify_column_sql`/
  `create_index_sql`/`diff_columns`/`extra_columns`/`index_covered`）、`MigrationReport.extra_columns`（只报告不删列）。
- **已接线**：`models::setting::migrate_db` 的 `users.api_token` 与 `models::module::migrate` 的自定义字段列
  都走 `Migrator::ensure_columns` —— 「只补缺失列、绝不 MODIFY」，列存在性用 `COLUMN_TYPE` 归一化后比较
  （`int(11)` ≡ `int`）。新增用例 `ensure_columns_only_adds_and_never_modifies`：断言定义成 `varchar(255)`
  也不会把已有的 `varchar(32)` 改掉，且二次调用返回空（幂等）。
- **遗留**：`create_table` / 索引迁移能力尚未被后台调用（当前只用到补列）。

#### 2.9.4 SMTP 与 HTTP

- `smtp.rs`：`smtplist` 多账号容灾（前一个账号失败自动换下一个）、按服务器广告选择认证机制
  （`AUTH PLAIN` / `AUTH LOGIN`）、网络错误与 4xx 临时错误重试一次而 5xx 永久错误不重试、
  邮件头带 `Message-ID`、正文 UTF-8 quoted-printable 编码；另含连接测试（握手 + 认证 + QUIT，不发信）。
  TLS 走系统 `curl` 的 `smtp://` / `smtps://`（curl 自带 TLS 栈），不引入 rustls/openssl 绑定。
- `http.rs`：统一入口 `send`，两后端 —— `Backend::Curl`（http/https、重定向、代理、gzip、任意方法与请求头）
  与 `Backend::Native`（仅明文 `http://`，零外部依赖）；默认 `Backend::Auto`：优先 curl（探测一次并缓存），
  不可用时退回原生。
- 设计约束：这一层**不引入任何新依赖**，只用主 crate 已编译过的
  `tokio` / `serde` / `sqlx` / `anyhow` / `chrono` / `tracing`，以便在低内存机器上也能单独编译与验证。

#### 2.9.5 ext 测试基线

- `crates/ext` 现有 **74 个 `#[test]`**（`cargo test -p rustcms-ext` → **74 passed / 0 failed / 2 ignored**）。
  两个 `#[ignore]` 是与其他实现做双向互通测量的手工用例（样本落 `%TEMP%`，
  见 `measure_deflate_samples` / `verify_std_deflate_samples`）；本轮一条旧用例未删。

### 2.10 外呼集成（翻译 / 云端服务 / AI / 搜索推送）

- `models/translate.rs`：百度（`baidu_sign` = `md5(appid+q+salt+密钥)`，小写；语言映射）、
  有道（sha256 双截断签名、`truncate` 截断、语言映射）、DeepL（免费/付费端点切换）；
  `translate_texts` 批量合并 + 行数不符退回逐段（防错位），`translate_html` 只翻标签之间文字
  （`split_html` / `rebuild_html`），结果写 `translate_logs` / `translate_text_logs`。
- `models/cloud.rs`：`login`（云端登录并回写 token/过期时间）、`auth_check(force)`、`cloud_post`/`cloud_post_raw`、
  `parse_envelope`、`cloud_translate`、`ai_provider`/`chat_payload`/`parse_chat_content`/`ai_complete`、
  `image_payload`/`parse_image_result`/`ai_generate_image`（云端为主路径，`custom:<下标>` 走 OpenAI 兼容分支）。
- `models/multilang.rs`：`site_sync` 补附件文件复制（`copy_attachment_files`，按目标站点存储配置）与
  `auto_translate` 接翻译引擎。
- `routes.rs`：`manage_cloud_info` / `manage_cloud_check`、翻译、伪原创、提取、AI 对话/生图 handler 全部接到上述模型，
  `fail_ai` 一族只剩真实校验失败的文案。
- **搜索推送**：Google 分支改为真实实现 —— 解析服务账号 JSON，
  RS256 JWT（`jsonwebtoken`）+ `urn:ietf:params:oauth:grant-type:jwt-bearer` 换 token，
  再逐条 POST `indexing.googleapis.com/v3/urlNotifications:publish`（`URL_UPDATED`）；
  缺少 `client_email` / `private_key`、私钥格式错误均返回明确文案；单条失败汇总返回。
- 测试：`google_oauth_rejects_invalid_service_account`（纯离线：非法 JSON、缺 `private_key`、坏私钥）。
  微信 / PayPal 成功路径需要真实凭据，未在本机验证；参数与错误路径由编译与代码审查覆盖。

**已知差异**：PayPal 下单（建单）依赖库里已有 `terrace_id`，本轮只补 capture；
支付宝 / 微信 / PayPal 的**联网回调闭环**未实测（无商户凭据），账务一致性由
`settle_gateway_payment` 单点覆盖。

### 2.11 定时任务与模板标签

#### 2.11.1 模板标签

- `contact`：读取 `settings.contact`，支持 `UserName/Cellphone/Wechat/Email/Address/Qrcode`。
- `now`：支持格式串 `{% now "2006" %}`，页脚版权年份恢复。
- `tagDataList`：复用 `archiveList` 的 `tagId` 查询，标签详情页文章列表恢复。
- `prevArchive` / `nextArchive`：新增 `archive::adjacent`（同模块、同栏目按 ID 相邻），详情页上下篇恢复。

验证：`/tag/1` 列出 3 篇文章；`/news/22.html` 显示下一篇 `/news/23.html`；首页页脚版权显示 2026。

#### 2.11.2 定时任务

- 新增 `src/crond.rs`：随 `server::run` 启动（`tokio::spawn`，60 秒间隔）。
- `archive_admin::publish_planned`：取 `archive_drafts.status = 2` 且 `created_time < now` 的草稿
  （最多 100 条），转入 `archives`、删除草稿并重算栏目计数，保留计划发布时间。
- `archive_admin::clean_recycle_bin`：每天清理 30 天前 `status = 99` 的回收站内容。
- 依赖外部服务的定时任务（采集、AI 写作、统计上报、提现打款等）仍由各自接口按需触发。

验证：草稿 `id=52` 临时设为计划态（`status=2`、`created_time=now-60`），启动服务后 8 秒内自动转入
`archives` 并删除草稿；验证后已回滚草稿并重算栏目计数。

改动文件：`src/template/site_provider.rs`、`src/template/mod.rs`、`src/models/archive.rs`、
`src/models/archive_admin.rs`、`src/crond.rs`、`src/main.rs`、`src/server.rs`。

---

## 三、测试与验证方法

### 3.1 测试基线演进

| 里程碑 | 主 crate（passed / failed） | `crates/ext` | 说明 |
|---|---|---|---|
| 内容体系落地 | 106 / 0 | — | 后台 9 模块 |
| 设置/设计/站点落地 | 133 / 0 | — | 后台 7 模块 |
| SEO 类插件落地 | 166 / 0 | — | 新增 39 项 |
| 安全/互动类插件落地 | 195 / 0 | — | 新增 29 项 |
| 商业/交易类插件落地 | 202 / 0 | — | 新增 7 项 |
| 五批后台全覆盖 | 236 / 0 | — | 新增 6 项 |
| 前台管线落地 | 247 / 0 | — | 新增 11 项 |
| 接线补全阶段 | 320 / 5 → 322 / 5 → 324 / 5 → 325 / 5 | 42 项非网络用例全绿 | 5 项为环境限制（沙箱 MySQL / 仓库外目录 / 依赖库的首页渲染） |
| 外呼与图像补全 | **470 / 0** | 74 passed / 2 ignored | 5 项环境失败已消除；`tmp/test-config.json` 指向真实本地库 |
| 标题图文字补全 | **471 / 0** | 74 passed / 2 ignored / 0 failed | 含新增 6 个标题图用例 |

最新基线：

- `cargo check --offline --all-targets`：**0 error**，仅 `crates/ext` 2 条既有 `never used` warning
  （`api_admin_gap.rs:329` 未使用赋值、`routes.rs:7360` 不可达语句，非新增）。
- `cargo test --offline`（主 crate）：**471 passed / 0 failed**。
- `cargo test --offline -p rustcms-ext`：**74 passed / 2 ignored / 0 failed**，全部纯逻辑（不联网）。
- 按模块统计的新增单测：`models::payment` 38、`models::wechat` 19、`models::titleimage` 37、
  `models::watermark` 10、`models::translate` 14。

### 3.2 测试脚手架的三条稳定性约定

1. `models::test_pool` **必须显式 opt-in**：只认 `RUSTCMS_TEST_CONFIG` 指定的配置文件，
   不自动读仓库根 `config.json`。理由：连库用例会真写库，且其中 7 项依赖线上真实配置
   （公众号 AppID/AppSecret、插件密钥等），默认连库会让 `cargo test` 变红并动到开发库数据。
   没有可用的库就返回 `None`，调用方一律跳过 —— 与「`None` 即跳过，`cargo test` 不能红」的约定一致。
2. 新增 `models::test_template_dir`（`#[cfg(test)]`）：优先仓库根 `template/default`，回退老布局
   `../template/default`。`routes::test_app`、`front::test_state`、`api_member::test_state`、
   `template::site_provider` 的用例统一改用它。
3. `routes::test_app_with_token` 在没有库时直接返回 `None`（原来照样返回一个「无池」的 Router），
   于是 `manage_api_requires_admin_token` / `renders_home_and_module_index_with_pagination` /
   `serves_categories_and_archives_from_db` / `write_api_crud_roundtrip` 从「断言必红」变成「跳过」。

连库跑法（**会写开发库 `cms`**，自行确认后再跑）：

```powershell
$env:RUSTCMS_TEST_CONFIG="tmp/test-config.json"
cargo test -- --test-threads=1
```

实测 4 个此前必红的用例连库后**全绿**（`serves_categories_and_archives_from_db`、
`write_api_crud_roundtrip`、`manage_api_requires_admin_token`、`renders_home_and_module_index_with_pagination`）；
另有 7 项（`manage_*` 系列、`serves_front_pages_by_rewrite_rules`）会因**线上配置缺失**而红
（例：`请先在公众号配置中填写 AppID 和 AppSecret`、伪静态规则未配），属数据依赖，按需再补。

`models::transfer::tests::finds_real_client_files` 重写为在临时目录铺全套客户端文件验证
路径拼接与防穿越（含「只有一个版本化脚本文件时也算没有」），仓库/站点根真有 `clientFiles/`
才额外校验。本工作区没有该目录（`clientFiles/` 是部署随包资产，仓库未内置），
故模板包分发接口在本地会回「客户端文件不存在」——这是资产缺失，不是路由缺陷。

### 3.3 端点覆盖度的机器核对

端点契约基线共 **403 条**（机器可读清单：method / path / handler / desc）。`tmp/api-coverage.ps1` 做自动比对：

- 扫 Rust 全部 `.route("/x", get(..).post(..))`；`.nest("/api", …)` / `.nest("/system/api", manage_router)`
  的**嵌套前缀要拼到内层相对路径上**（内层路径同样以 `/` 开头，不能按「是否以 `/` 开头」判断绝对/相对）；
- `api_route!` 宏同时注册裸路径与 `/api` 别名，两条都要计入；
- 契约里的 `{id}` 与 axum 0.7 的 `:id` 统一按参数段匹配；
- 口径修正：正则 `\.route\(\s*"([^"]+)"` 只有 **1 个捕获组**，取 `Groups[1]`
  （曾误用 `Groups[2]` 导致路径恒为空、覆盖率假性为 0）。

**结果：路径级 403/403、方法级 403/403。**

### 3.4 构建环境与内存约束

- 本机 16GB 内存 + **8GB 固定页面文件**（`AutomaticManagedPagefile=False`，**无管理员权限无法扩容**），
  提交上限 24414MB、常驻占用 ~23GB，**提交余量曾低至 ~1GB**。
- 重压时 `cargo test` 编测试二进制会报 `memory allocation of 2097152 bytes failed` → 退出码 `0xc0000409`。
  **这是 OOM abort，不是栈溢出**（`rustc.exe` 的 PE 栈预留只有 2MB；虽已备份并抬到 64MB——`rustc.exe.stack-bak`——
  但与该错误无关）。
- 已验证的降内存参数：`CARGO_BUILD_JOBS=1`、`CARGO_INCREMENTAL=0`、`GOGC=20`、
  `CARGO_PROFILE_{DEV,TEST}_DEBUG=0`、`CARGO_PROFILE_{DEV,TEST}_CODEGEN_UNITS=1024`、
  `CARGO_PROFILE_*_LINKFLAGS=-C link-arg=-Wl,--no-keep-memory`、`RUST_MIN_STACK=64MB`。
- 只对最终 crate 关调试信息（依赖产物复用）：
  - bin：`cargo rustc --bin rustcms -- -C debuginfo=0`（已固化进 `tmp/build.ps1`）；
  - 测试：`cargo rustc --bin rustcms -- --test -C debuginfo=0` 后直接跑
    `target/debug/deps/rustcms-*.exe --test-threads=1`。
- 脚本已固化：`tmp/check.ps1` / `tmp/test.ps1` / `tmp/build.ps1`。
- 结论：提交余量 1GB 时编不出 test harness；2026-11-07 起内存条件下该限制不再成立
  （`cargo test --offline --no-run` 约 2m15s ~ 3m 通过）。
- gcc 侧：本机 cc1/库搜索路径失效，编译前需把 `mingw64/libexec/gcc/x86_64-w64-mingw32/16.2.0`
  加入 `PATH`、把 `mingw64/x86_64-w64-mingw32/lib` 等加入 `LIBRARY_PATH`；`RUST_MIN_STACK` 必须写字节数。

### 3.5 验收口径

1. `cargo build` / `cargo check --all-targets` 通过且无新增告警；`cargo test` 全绿（连库用例通过 `test_pool` 自动跳过）。
2. 每个动作都有对应的模型函数 + 路由处理器 + 测试。
3. 响应 JSON 字段名与既定契约逐字段一致；业务错误中文文案一致。
4. 无 `unimplemented!` / `todo!` 残留；外部依赖类动作一律「接口保留 + 参数校验 + 明确未实现提示」，
   不许静默成功或静默失败。

### 3.6 踩坑记录（结论性经验）

1. **同一文件的多处修改必须串行**：`replace_in_file` 曾两次报告「成功」但内容没写进去，
   根因是同一文件在同一并行批次里被编辑多次——第二次基于旧快照回写，把第一次的改动覆盖掉了。
   改完 `manage_router` 一定要 grep 复核。
2. **解析器里的静默跳过 + 没有覆盖该写法的用例 = 功能悄悄消失**：
   正则敏感词写成 `{\d{6,12}}` 时，解析必须用 `rfind('}')` 而不是 `find('}')`——
   量词 `{6,12}` 自带 `}`，取第一个会把正则截断成 `\d{6,12`，编译失败后被静默跳过。
3. **配置读出空值时先怀疑取值 API**：`site_setting::get_section` 只认**对象**，
   读数组型配置（如 `upload_file`）必须用 `get_array`。用错时表现为「写入成功、读回永远是空数组」，
   很容易误判成保存逻辑有问题。
4. **无符号列 / 关键字列**：`archive_data.id`（bigint unsigned）、`tag_contents.id`（int unsigned）、
   `ai_chat_messages.seq`（int unsigned）取 `i64`/`i32` 会整条查询失败，统一 `CAST(id AS CHAR)` 再解析；
   `wechat_menus.type` 是 MySQL 关键字，字段改名 + 双重 `rename` 才不查空。
5. **`SHOW TABLES LIKE ?` 不支持参数占位符**（报 1064），检查表是否存在要用
   `information_schema.tables WHERE table_schema = DATABASE AND table_name = ?`。
6. **路由规则串带前导 `/`**：`url::match_path` 的规则串包含前导 `/`，
   `parse_route` 必须传完整 `path` 而不是去斜杠后的 param，否则栏目/详情页全部 404。
7. **主线汇合时修掉的真问题**：
   - `packer.rs` 建树 `heap.pop?.1` 对 `Reverse<_>` 取字段（编译错误）→ 解包 `Reverse`；
   - `payment::wechat::parse_xml` 把 CDATA 误判成嵌套容器（`<![CDATA[` 也以 `<` 开头）→
     所有 CDATA 字段解析为空，微信回调会直接失败。**这是单测抓出来的真 bug**，已修正判定顺序；
   - `api_interact.rs` 的 `wechat_pay_notify` 薄转发少包一层 `State(state)`（E0308）；
   - 三处**测试期望写错**（实现是对的）：`EncodingAESKey` 用非规范 base64（末位索引必须能被 4 整除才对）、
     百度签名期望串多了一个 `-`、`Engine::from_key("google")` 应返回 `Some(Google)`；
   - `translate.rs` / `cloud.rs` 文件头「外呼未实现」的过期注释改成现状描述。

---

## 四、遗留与后续

### 4.1 未开始

- **GraphQL 接口层**：尚未开始。

### 4.2 需真实凭据才能端到端验证

- 三个支付网关的**联网回调闭环**（微信 V2 / 支付宝 / PayPal）未实测；账务一致性已由
  `settle_gateway_payment` 单点覆盖。
- 支付宝 RSA2 目前只有 **63 字节模数的玩具密钥向量**（`q=3,e=3` + CRT 开三次方造真签名），
  能验证 ASN.1 → 模幂 → PKCS#1 填充 → 摘要整链，但**不能替代 2048 位真实证书的端到端验证**。
- 微信 / 小程序成功路径需要真实 AppID/Secret；搜索推送需要服务账号私钥。

### 4.3 已知简化与边界

| 项 | 现状 |
|---|---|
| 备份 dump | 已产 zip（`database.sql` + `template/` + `static/`），zip 条目已真压缩 |
| sitemap | 已分片；不生成空尾页，XML 暂不输出 `lastmod` 与多语言 `xhtml:link` |
| HTML 缓存 | 前台命中 + 回填已通；移动端 `cache/mobile`、query 独立缓存、内存二级缓存、远程推送未做 |
| 多站点 | 单连接池；子站切换登录与数据隔离未做 |
| 设计模块 | zip 打包下载 / 上传解压、演示数据备份初始化未做 |
| 采集 | 抓取 / 问答组合 / 关键词拓展未实现（需外呼） |
| 全文索引 | 配置与状态可用；`rebuild` 需索引引擎，未实现 |
| 友情链接检查 | `plugin/link/check` 未实现（需 HTTP 客户端） |
| 图像 | GIF 只解码不回写、WebP 只识别尺寸；文字光栅化不做 hinting / kerning，CFF(OTTO) 字体明确报错 |
| 文字折行 | 按字切分（不带中文分词），折行位置与带分词的实现可能略有不同 |
| 微信 code_url | 仍是原生链接，前端需用 JS 二维码组件出图 |
| 小程序码 | 使用 `/wxa/getwxacode`，未使用 `getwxacodeunlimit` |
| `HtmlToMarkdown` | 简化版 |
| AI 对话 | 未内置 LLM 服务，需配置模型服务；`/ai/stream` 产生端未实现 |
| DEFLATE | 动态 Huffman 只有单块（异构数据多块还能再省一点，收益小、风险高） |
| 表结构同步 | `create_table` / 索引迁移能力已实现但未被后台调用（当前只用到补列） |
| 平台辅助层 | 模板标签库与视图层由 tera + 自研 provider 承载；定时任务已内置 2 条任务 |

### 4.4 独立部署：默认零外呼与可配置开关

源码里**不再有任何硬编码的外部服务地址**。独立部署不配置就是纯本地运行：

| 能力 | 配置方式 | 未配置时的行为 |
|---|---|---|
| 云端登录 / 翻译 / AI / 敏感词同步 | `settings.cloud.api_base`，或环境变量 `RUSTCMS_CLOUD_API` | 明确回「云端服务未配置…」，**不发任何请求** |
| 云端 AI 对话（官方模型分支） | 同上；模型名 `cloud-flash` / `cloud-pro` | 同上；`custom:<下标>` 分支读站内配置，不受影响 |
| 在线版本检查 | `settings.upgrade.version_url`，或环境变量 `RUSTCMS_VERSION_URL` | `has_new:false`，**不发任何请求** |
| 在线升级包下载 | 同上（清单 `packages.<平台>` / 顶层 `url`；相对地址按清单目录解析） | 明确回「未配置在线升级源」；**上传本地升级包**仍可用 |
| 升级包主程序识别 | — | 白名单为 `rustcms` / `rustcms.exe` / `rustcms-server*` / `rustcms-cli*` |

发行侧（`deploy/release/pack-release.ps1`）的在线安装脚本默认镜像是占位符
`https://your-mirror.example.com`，不写任何真实外部服务地址；只有运维显式传 `-Mirror` 才会写入。

请求头 User-Agent 统一为 `rustcms/<版本>`；前端打包产物的 webpack chunk 全局名、
品牌文案与中文提示也已整体改为 RustCMS，并移除了原先的运行时文案替换脚本。

**仍保留的兼容契约**（不是溯源引用，改名会破坏已有数据与前端调用）：

- 后台路由命名空间 `/system/api/anqi/*`（前端打包产物按此地址调用）；
- settings 键 `settings.anqi.*`、插件键 `anqi`（已持久化数据）；
- 浏览器本地存储键 `anqi-adminToken`（改了会让已登录用户被登出）；
- `public/` 是构建产物：内部 locale key、CSS 类名、注入标记等仍带历史前缀，
  重新构建前端资源时才能一并清理；**出厂文案已不含任何外部品牌字样**。

### 4.5 维护提示

- 改完接口或模板后先跑 `tmp/check.ps1`（`cargo check --all-targets`），再跑 `tmp/test.ps1`。
- 连库用例会**写开发库**，跑之前确认 `RUSTCMS_TEST_CONFIG` 指向的是测试库。
- 新增模块时同步更新本文件第二节对应表格与实测基线，避免文档与实现脱节。



