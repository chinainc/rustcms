//! 模板标签的真实数据源：把 `archiveList` / `archiveDetail` / `categoryDetail` /
//! `navList` 等标签接到数据库上。
//!
//! 链接一律走 [`crate::models::url`] 的伪静态规则生成，与前台路由用的是同一套规则。

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;

use anyhow::Result;
use serde_json::{Map, Value};
use sqlx::mysql::MySqlPool;

use super::Provider;
use crate::models::{archive, category, multilang, site_setting, tag, url, website, Category, Module};

/// 模板数据源：既可以代表「某个栏目页」，也可以代表「某篇内容详情页」。
pub struct SiteProvider {
    pub pool: MySqlPool,
    pub mode: i32,
    /// 当前栏目（0 = 首页/模块列表页）
    pub category_id: u32,
    /// 当前模块（模块列表页时使用，0 = 不限）
    pub module_id: u32,
    pub archive: Option<archive::Archive>,
    /// 详情页正文（`archive_data.content`）
    pub content: String,
    pub page: u32,
    pub page_size: u32,
    /// 搜索页关键词（`/search/?q=...`）
    pub keyword: String,
    /// 当前标签（标签详情页）
    pub tag: Option<tag::Tag>,
    /// 标签列表页（`/tags/`）
    pub tag_index: bool,
    /// 当前站点语言（`tr` 翻译用；一次渲染只读一次库）
    language: tokio::sync::OnceCell<String>,
}

impl SiteProvider {
    /// 栏目页 / 首页数据源（`category_id = 0` 表示首页）。
    pub async fn new(pool: MySqlPool, category_id: u32) -> Result<Self> {
        let mode = url::load_mode(&pool).await?;
        Ok(Self {
            pool,
            mode,
            category_id,
            module_id: 0,
            archive: None,
            content: String::new(),
            page: 1,
            page_size: 10,
            keyword: String::new(),
            tag: None,
            tag_index: false,
            language: tokio::sync::OnceCell::new(),
        })
    }

    /// 模块列表页（`/news/`）数据源。
    pub async fn for_module(pool: MySqlPool, module_id: u32) -> Result<Self> {
        let mut provider = Self::new(pool, 0).await?;
        provider.module_id = module_id;
        Ok(provider)
    }

    /// 内容详情页数据源（分类从内容上带出来）。
    pub async fn for_archive(pool: MySqlPool, id: i64) -> Result<Option<Self>> {
        let mode = url::load_mode(&pool).await?;
        let Some(item) = archive::get(&pool, id).await? else {
            return Ok(None);
        };
        let content = archive::content(&pool, id).await?.unwrap_or_default();
        Ok(Some(Self {
            pool,
            mode,
            category_id: item.category_id,
            module_id: item.module_id,
            archive: Some(item),
            content,
            page: 1,
            page_size: 10,
            keyword: String::new(),
            tag: None,
            tag_index: false,
            language: tokio::sync::OnceCell::new(),
        }))
    }

    pub fn with_page(mut self, page: u32) -> Self {
        self.page = page.max(1);
        self
    }

    /// 搜索关键词（列表按标题模糊匹配）。
    pub fn with_keyword(mut self, keyword: &str) -> Self {
        self.keyword = keyword.trim().to_string();
        self
    }

    /// 标签详情页上下文。
    pub fn with_tag(mut self, item: tag::Tag) -> Self {
        self.tag = Some(item);
        self
    }

    /// 标签列表页上下文。
    pub fn as_tag_index(mut self) -> Self {
        self.tag_index = true;
        self
    }

    /// 分类索引与模块索引（用于拼链接）。
    async fn indexes(&self) -> Result<(HashMap<u32, Category>, HashMap<u32, Module>)> {
        let categories = category::list_all(&self.pool).await?;
        let modules = crate::models::module::list(&self.pool).await?;
        Ok((
            categories.into_iter().map(|item| (item.id, item)).collect(),
            modules.into_iter().map(|item| (item.id, item)).collect(),
        ))
    }

    fn link_for(
        &self,
        item: &archive::Archive,
        categories: &HashMap<u32, Category>,
        modules: &HashMap<u32, Module>,
    ) -> String {
        if let Some(fixed) = &item.fixed_link {
            if !fixed.trim().is_empty() {
                return fixed.clone();
            }
        }
        let cat = categories.get(&item.category_id);
        let module = modules.get(&item.module_id);
        url::archive_url(
            self.mode,
            cat.map(|c| c.url_token.as_str()).unwrap_or(""),
            module.map(|m| m.url_token.as_str()).unwrap_or(""),
            item.id,
            &item.url_token,
            item.category_id,
            1,
        )
    }

    /// 列表查询条件：标签参数 > 当前上下文（栏目 / 模块）。
    async fn query_for(&self, args: &Map<String, Value>) -> Result<archive::ArchiveQuery> {
        let mut query = archive::ArchiveQuery::new();

        let categories: Vec<Category> = category::list_all(&self.pool).await?;
        let arg_category = args
            .get("categoryId")
            .and_then(|value| super::as_text(value).parse::<u32>().ok());
        let arg_module = args
            .get("moduleId")
            .and_then(|value| super::as_text(value).parse::<u32>().ok());

        if let Some(cid) = arg_category.filter(|id| *id > 0) {
            query.category_ids = category::self_and_sub_ids(&categories, cid);
        } else if self.category_id > 0 {
            query.category_ids = category::self_and_sub_ids(&categories, self.category_id);
        }

        if let Some(mid) = arg_module.filter(|id| *id > 0) {
            query.module_id = Some(mid);
        } else if self.category_id == 0 && self.module_id > 0 {
            query.module_id = Some(self.module_id);
        }

        // 标签参数（archiveList 支持 tagId=）优先于当前标签上下文
        let arg_tag = args
            .get("tagId")
            .and_then(|value| super::as_text(value).parse::<u32>().ok())
            .filter(|id| *id > 0);
        if let Some(tag_id) = arg_tag {
            query = query.tag(tag_id);
        } else if let Some(item) = &self.tag {
            query = query.tag(item.id);
        }

        if !self.keyword.is_empty() {
            query = query.keyword(&self.keyword);
        }

        Ok(query)
    }

    /// 联系我们信息（`{% contact with name="Cellphone" %}`）。
    async fn contact(&self, args: &Map<String, Value>) -> Result<Value> {
        let name = args.get("name").map(super::as_text).unwrap_or_default();
        let field = match name.as_str() {
            "UserName" | "Contact" | "Contacts" | "ContactsName" => "user_name",
            "Cellphone" | "Phone" | "Tel" => "cellphone",
            "Wechat" => "wechat",
            "Email" => "email",
            "Address" => "address",
            "Qrcode" => "qrcode",
            _ => "",
        };
        if field.is_empty() {
            return Ok(Value::String(String::new()));
        }
        let contact = crate::models::site_setting::get_section(
            &self.pool,
            crate::models::site_setting::CONTACT,
        )
        .await
        .unwrap_or(Value::Null);
        Ok(Value::String(
            contact
                .get(field)
                .map(super::as_text)
                .unwrap_or_default(),
        ))
    }

    /// 当前时间（`{% now "2006" %}`）。
    fn now_value(&self, args: &Map<String, Value>) -> Value {
        let layout = args
            .get("__arg0")
            .map(super::as_text)
            .unwrap_or_else(|| "2006-01-02".into());
        let pattern = super::go_layout_to_chrono(&layout);
        Value::String(chrono::Local::now().format(&pattern).to_string())
    }

    /// 上一篇 / 下一篇（同模块、同栏目，按 ID 排序）。
    async fn adjacent_archive(&self, args: &Map<String, Value>, previous: bool) -> Result<Value> {
        let _ = args;
        let Some(current) = self.archive.as_ref() else {
            return Ok(Value::Null);
        };
        let Some(item) = archive::adjacent(
            &self.pool,
            current.id,
            current.module_id,
            current.category_id,
            previous,
        )
        .await?
        else {
            return Ok(Value::Null);
        };
        let (categories, modules) = self.indexes().await?;
        let mut map = Map::new();
        map.insert("Id".into(), Value::from(item.id));
        map.insert("Title".into(), Value::String(item.title.clone()));
        map.insert(
            "Link".into(),
            Value::String(self.link_for(&item, &categories, &modules)),
        );
        Ok(Value::Object(map))
    }

    async fn archive_list(&self, args: &Map<String, Value>) -> Result<Value> {
        let (categories, modules) = self.indexes().await?;

        let limit = args
            .get("limit")
            .map(|value| super::as_text(value).parse::<u32>().unwrap_or(10))
            .unwrap_or(self.page_size)
            .clamp(1, 200);
        let page = args
            .get("page")
            .map(|value| super::as_text(value).parse::<u32>().unwrap_or(1))
            .unwrap_or(self.page);

        let query = self.query_for(args).await?;
        let order = match args.get("order").map(super::as_text).as_deref() {
            Some("views") | Some("hot") => archive::Order::ViewsDesc,
            Some("created") => archive::Order::CreatedDesc,
            Some("updated") => archive::Order::UpdatedDesc,
            _ => archive::Order::IdDesc,
        };

        let result = archive::list(&self.pool, &query, order, page, limit).await?;
        let list: Vec<Value> = result
            .list
            .iter()
            .map(|item| {
                let thumb = item.cover();
                let mut map = Map::new();
                map.insert("Id".into(), Value::from(item.id));
                map.insert("Title".into(), Value::String(item.title.clone()));
                map.insert(
                    "Link".into(),
                    Value::String(self.link_for(item, &categories, &modules)),
                );
                map.insert(
                    "Description".into(),
                    Value::String(item.description.clone()),
                );
                map.insert("Keywords".into(), Value::String(item.keywords.clone()));
                map.insert(
                    "CreatedTime".into(),
                    Value::from(item.created_time.unwrap_or(0)),
                );
                map.insert(
                    "UpdatedTime".into(),
                    Value::from(item.updated_time.unwrap_or(0)),
                );
                map.insert("Views".into(), Value::from(item.views));
                map.insert("CommentCount".into(), Value::from(item.comment_count));
                map.insert("CategoryId".into(), Value::from(item.category_id));
                map.insert("ModuleId".into(), Value::from(item.module_id));
                map.insert("Thumb".into(), Value::String(thumb.clone()));
                map.insert("Logo".into(), Value::String(thumb));
                Value::Object(map)
            })
            .collect();

        Ok(Value::Array(list))
    }

    async fn category_detail(&self, args: &Map<String, Value>) -> Result<Value> {
        let Some(current) = category::get(&self.pool, self.category_id).await? else {
            return Ok(Value::Null);
        };
        let name = args.get("name").map(super::as_text).unwrap_or_default();
        let modules = crate::models::module::list(&self.pool).await?;
        let module_token = modules
            .iter()
            .find(|item| item.id == current.module_id)
            .map(|item| item.url_token.clone())
            .unwrap_or_default();
        let value = match name.as_str() {
            "Title" => {
                if current.seo_title.is_empty() {
                    current.title.clone()
                } else {
                    current.seo_title.clone()
                }
            }
            "Name" | "TitleName" => current.title.clone(),
            "Link" => url::category_url(
                self.mode,
                &current.url_token,
                &module_token,
                current.id,
                self.page,
            ),
            "Description" => current.description.clone(),
            "Keywords" => current.keywords.clone(),
            "CreatedTime" => current.created_time.unwrap_or(0).to_string(),
            "Id" => current.id.to_string(),
            "Logo" => current.logo.clone(),
            _ => String::new(),
        };
        Ok(Value::String(value))
    }

    async fn archive_detail(&self, args: &Map<String, Value>) -> Result<Value> {
        let Some(item) = self.archive.as_ref() else {
            return Ok(Value::Null);
        };
        let name = args.get("name").map(super::as_text).unwrap_or_default();
        let format = args
            .get("format")
            .map(super::as_text)
            .filter(|value| !value.is_empty());

        let stamp = |timestamp: i64| -> Value {
            match &format {
                Some(layout) => {
                    let pattern = layout
                        .replace("2006", "%Y")
                        .replace("01", "%m")
                        .replace("02", "%d")
                        .replace("15", "%H")
                        .replace("04", "%M")
                        .replace("05", "%S");
                    match chrono::DateTime::from_timestamp(timestamp, 0) {
                        Some(dt) => Value::String(dt.format(&pattern).to_string()),
                        None => Value::String(String::new()),
                    }
                }
                None => Value::from(timestamp),
            }
        };

        let value = match name.as_str() {
            "Id" => Value::from(item.id),
            "Title" => Value::String(item.title.clone()),
            "SeoTitle" => Value::String(item.seo_title.clone()),
            "Keywords" => Value::String(item.keywords.clone()),
            "Description" => Value::String(item.description.clone()),
            "Content" | "Data" => Value::String(self.content.clone()),
            "CreatedTime" => stamp(item.created_time.unwrap_or(0)),
            "UpdatedTime" => stamp(item.updated_time.unwrap_or(0)),
            "Views" => Value::from(item.views),
            "CommentCount" => Value::from(item.comment_count),
            "CategoryId" => Value::from(item.category_id),
            "ModuleId" => Value::from(item.module_id),
            "Link" => {
                let (categories, modules) = self.indexes().await?;
                Value::String(self.link_for(item, &categories, &modules))
            }
            "Thumb" | "Logo" => Value::String(item.cover()),
            "Images" => {
                let images: Vec<Value> = item.image_list().into_iter().map(Value::String).collect();
                Value::Array(images)
            }
            _ => Value::Null,
        };
        Ok(value)
    }

    /// 当前列表页第 `page` 页的地址。
    async fn page_link(&self, page: u32) -> String {
        if let Some(item) = &self.tag {
            return url::tag_url(self.mode, item.id, &item.url_token, page);
        }
        if self.tag_index {
            return url::tag_index_url(self.mode, page);
        }
        if self.category_id > 0 {
            if let Ok(Some(current)) = category::get(&self.pool, self.category_id).await {
                let modules = crate::models::module::list(&self.pool)
                    .await
                    .unwrap_or_default();
                let token = modules
                    .iter()
                    .find(|item| item.id == current.module_id)
                    .map(|item| item.url_token.clone())
                    .unwrap_or_default();
                return url::category_url(self.mode, &current.url_token, &token, current.id, page);
            }
        }
        if self.module_id > 0 {
            let modules = crate::models::module::list(&self.pool)
                .await
                .unwrap_or_default();
            if let Some(item) = modules.iter().find(|item| item.id == self.module_id) {
                return url::archive_index_url(self.mode, &item.url_token, page);
            }
        }
        format!("/{page}")
    }

    /// 分页数据（供 `{% pagination pages with show="5" %}` 使用）。
    async fn pagination(&self, args: &Map<String, Value>) -> Result<Value> {
        let show = args
            .get("show")
            .map(|value| super::as_text(value).parse::<u32>().unwrap_or(5))
            .unwrap_or(5)
            .clamp(1, 20);

        let query = self.query_for(&Map::new()).await?;
        let total = archive::list(&self.pool, &query, archive::Order::IdDesc, 1, 1)
            .await?
            .total;
        let page_size = self.page_size.max(1) as i64;
        let total_pages = if total <= 0 {
            0
        } else {
            ((total as f64) / (page_size as f64)).ceil() as i64
        };
        let current = (self.page as i64).clamp(1, total_pages.max(1));

        let mut half = (show / 2) as i64;
        if half == 0 {
            half = 1;
        }
        let mut start = (current - half).max(1);
        let mut end = start + show as i64 - 1;
        if end > total_pages {
            end = total_pages.max(1);
            start = (end - show as i64 + 1).max(1);
        }

        let mut pages = Vec::new();
        for page in start..=end {
            let mut item = Map::new();
            item.insert("Name".into(), Value::String(page.to_string()));
            item.insert(
                "Link".into(),
                Value::String(self.page_link(page as u32).await),
            );
            item.insert("IsCurrent".into(), Value::Bool(page == current));
            pages.push(Value::Object(item));
        }

        let mut map = Map::new();
        map.insert("TotalItems".into(), Value::from(total));
        map.insert("TotalPages".into(), Value::from(total_pages.max(0)));
        map.insert("CurrentPage".into(), Value::from(current));
        map.insert(
            "FirstPage".into(),
            serde_json::json!({
                "Link": self.page_link(1).await,
                "IsCurrent": current == 1,
            }),
        );
        map.insert(
            "LastPage".into(),
            serde_json::json!({
                "Link": self.page_link(total_pages.max(1) as u32).await,
                "IsCurrent": current == total_pages.max(1),
            }),
        );
        if current > 1 {
            map.insert(
                "PrevPage".into(),
                serde_json::json!({ "Link": self.page_link((current - 1) as u32).await }),
            );
        } else {
            map.insert("PrevPage".into(), Value::Null);
        }
        if current < total_pages {
            map.insert(
                "NextPage".into(),
                serde_json::json!({ "Link": self.page_link((current + 1) as u32).await }),
            );
        } else {
            map.insert("NextPage".into(), Value::Null);
        }
        map.insert("Pages".into(), Value::Array(pages));
        Ok(Value::Object(map))
    }

    /// 栏目列表（`{% categoryList list with moduleId="1" parentId="0" %}`）。
    async fn category_list(&self, args: &Map<String, Value>) -> Result<Value> {
        let module_id = args
            .get("moduleId")
            .and_then(|value| super::as_text(value).parse::<u32>().ok())
            .unwrap_or(self.module_id);
        let parent_id = args
            .get("parentId")
            .and_then(|value| super::as_text(value).parse::<u32>().ok())
            .unwrap_or(0);

        let categories = category::list_all(&self.pool).await?;
        let modules = crate::models::module::list(&self.pool).await?;
        let list: Vec<Value> = categories
            .iter()
            .filter(|item| item.status == 1)
            .filter(|item| module_id == 0 || item.module_id == module_id)
            .filter(|item| item.parent_id == parent_id)
            .map(|item| {
                let token = modules
                    .iter()
                    .find(|module| module.id == item.module_id)
                    .map(|module| module.url_token.clone())
                    .unwrap_or_default();
                let has_children = categories
                    .iter()
                    .any(|child| child.parent_id == item.id && child.status == 1);
                let mut map = Map::new();
                map.insert("Id".into(), Value::from(item.id));
                map.insert("Title".into(), Value::String(item.title.clone()));
                map.insert(
                    "Link".into(),
                    Value::String(url::category_url(
                        self.mode,
                        &item.url_token,
                        &token,
                        item.id,
                        1,
                    )),
                );
                map.insert(
                    "Description".into(),
                    Value::String(item.description.clone()),
                );
                map.insert("Keywords".into(), Value::String(item.keywords.clone()));
                map.insert("Logo".into(), Value::String(item.logo.clone()));
                map.insert("Thumb".into(), Value::String(item.cover()));
                map.insert("HasChildren".into(), Value::Bool(has_children));
                Value::Object(map)
            })
            .collect();
        Ok(Value::Array(list))
    }

    /// 模块信息（`{% moduleDetail with name="Link" id="2" %}`）。
    async fn module_detail(&self, args: &Map<String, Value>) -> Result<Value> {
        let id = args
            .get("id")
            .and_then(|value| super::as_text(value).parse::<u32>().ok())
            .unwrap_or(self.module_id);
        let modules = crate::models::module::list(&self.pool).await?;
        let Some(item) = modules.iter().find(|module| module.id == id) else {
            return Ok(Value::Null);
        };
        let name = args.get("name").map(super::as_text).unwrap_or_default();
        let value = match name.as_str() {
            "Title" | "Name" => item.name.clone(),
            "Link" => url::archive_index_url(self.mode, &item.url_token, 1),
            "Id" => item.id.to_string(),
            _ => String::new(),
        };
        Ok(Value::String(value))
    }

    /// 单页信息（`{% pageDetail with name="Title" token="about" %}`）。
    async fn page_detail(&self, args: &Map<String, Value>) -> Result<Value> {
        let token = args.get("token").map(super::as_text).unwrap_or_default();
        let Some(item) = category::get_by_token(&self.pool, &token).await? else {
            return Ok(Value::Null);
        };
        let modules = crate::models::module::list(&self.pool).await?;
        let module_token = modules
            .iter()
            .find(|module| module.id == item.module_id)
            .map(|module| module.url_token.clone())
            .unwrap_or_default();
        let name = args.get("name").map(super::as_text).unwrap_or_default();
        let value = match name.as_str() {
            "Title" => item.title.clone(),
            "Description" => item.description.clone(),
            "Keywords" => item.keywords.clone(),
            "Logo" => item.logo.clone(),
            "Link" => format!("/{}.html", item.url_token),
            "Id" => item.id.to_string(),
            _ => {
                let _ = module_token;
                String::new()
            }
        };
        Ok(Value::String(value))
    }

    /// 轮播图（`{% bannerList banners %}`），数据在 `settings.banner`。
    async fn banner_list(&self) -> Result<Value> {
        let raw: Option<String> =
            sqlx::query_scalar("SELECT value FROM settings WHERE `key` = 'banner'")
                .fetch_optional(&self.pool)
                .await?;
        let items: Value = raw
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        let list: Vec<Value> = items
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .map(|item| {
                        let mut map = Map::new();
                        map.insert(
                            "Logo".into(),
                            Value::String(item.get("logo").map(super::as_text).unwrap_or_default()),
                        );
                        map.insert(
                            "Link".into(),
                            Value::String(item.get("link").map(super::as_text).unwrap_or_default()),
                        );
                        map.insert(
                            "Alt".into(),
                            Value::String(item.get("alt").map(super::as_text).unwrap_or_default()),
                        );
                        map.insert(
                            "Description".into(),
                            Value::String(
                                item.get("description")
                                    .map(super::as_text)
                                    .unwrap_or_default(),
                            ),
                        );
                        Value::Object(map)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(Value::Array(list))
    }

    /// 友情链接（`{% linkList friendLinks %}`）。
    async fn link_list(&self) -> Result<Value> {
        let rows: Vec<(String, String, String)> = sqlx::query_as(
            "SELECT title, link, logo FROM links WHERE status = 1 ORDER BY sort ASC, id ASC",
        )
        .fetch_all(&self.pool)
        .await?;
        let list: Vec<Value> = rows
            .into_iter()
            .map(|(title, link, logo)| {
                let mut map = Map::new();
                map.insert("Title".into(), Value::String(title));
                map.insert("Link".into(), Value::String(link));
                map.insert("Logo".into(), Value::String(logo));
                Value::Object(map)
            })
            .collect();
        Ok(Value::Array(list))
    }

    /// 标签列表（`{% tagList tags with letter="A" limit="20" %}`）。
    async fn tag_list(&self, args: &Map<String, Value>) -> Result<Value> {
        let limit = args
            .get("limit")
            .map(|value| super::as_text(value).parse::<u32>().unwrap_or(20))
            .unwrap_or(20)
            .clamp(1, 200);
        let letter = args.get("letter").map(super::as_text).unwrap_or_default();

        let rows: Vec<tag::Tag> = if letter.is_empty() {
            sqlx::query_as("SELECT * FROM tags WHERE status = 1 ORDER BY id ASC LIMIT ?")
                .bind(limit)
                .fetch_all(&self.pool)
                .await?
        } else {
            // 首字母筛选：优先 first_letter 字段，其次标题首字符
            sqlx::query_as(
                "SELECT * FROM tags WHERE status = 1 AND (first_letter = ? OR LEFT(title, 1) = ?) \
                 ORDER BY id ASC LIMIT ?",
            )
            .bind(&letter)
            .bind(&letter)
            .bind(limit)
            .fetch_all(&self.pool)
            .await?
        };

        let list: Vec<Value> = rows
            .iter()
            .map(|item| {
                let mut map = Map::new();
                map.insert("Id".into(), Value::from(item.id));
                map.insert("Title".into(), Value::String(item.title.clone()));
                map.insert(
                    "Link".into(),
                    Value::String(url::tag_url(
                        self.mode,
                        item.id,
                        &item.url_token,
                        1,
                    )),
                );
                map.insert(
                    "Description".into(),
                    Value::String(item.description.clone()),
                );
                map.insert("Keywords".into(), Value::String(item.keywords.clone()));
                map.insert("Logo".into(), Value::String(item.logo.clone()));
                Value::Object(map)
            })
            .collect();
        Ok(Value::Array(list))
    }

    /// 当前标签信息（`{% tagDetail with name="Title" %}`）。
    async fn tag_detail(&self, args: &Map<String, Value>) -> Result<Value> {
        // 支持通过 id / urlToken 指定，缺省用当前标签上下文
        let item = if let Some(id) = args
            .get("id")
            .and_then(|value| super::as_text(value).parse::<u32>().ok())
            .filter(|id| *id > 0)
        {
            tag::get(&self.pool, id).await?
        } else if let Some(token) = args.get("urlToken").map(super::as_text).filter(|v| !v.is_empty())
        {
            tag::get_by_token(&self.pool, &token).await?
        } else {
            self.tag.clone()
        };
        let Some(item) = item else {
            return Ok(Value::Null);
        };

        let name = args.get("name").map(super::as_text).unwrap_or_default();
        let value = match name.as_str() {
            "Title" => {
                if item.seo_title.is_empty() {
                    item.title.clone()
                } else {
                    item.seo_title.clone()
                }
            }
            "Name" | "TitleName" => item.title.clone(),
            "Link" => url::tag_url(self.mode, item.id, &item.url_token, 1),
            "Description" => item.description.clone(),
            "Keywords" => item.keywords.clone(),
            "Logo" => item.logo.clone(),
            "Id" => item.id.to_string(),
            "Content" => tag::content(&self.pool, item.id).await?.0,
            _ => String::new(),
        };
        Ok(Value::String(value))
    }

    async fn system(&self, args: &Map<String, Value>) -> Result<Value> {
        let name = args.get("name").map(super::as_text).unwrap_or_default();
        let raw: Option<String> =
            sqlx::query_scalar("SELECT value FROM settings WHERE `key` = 'system'")
                .fetch_optional(&self.pool)
                .await?;
        let system: Value = raw
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        if name == "TemplateUrl" {
            // 系统标签的约定：模板静态资源在 `/static/{TemplateName}/` 下。
            let template = system
                .get("template_name")
                .map(super::as_text)
                .unwrap_or_default();
            let template = if template.trim().is_empty() {
                "default".to_string()
            } else {
                template
            };
            return Ok(Value::String(format!("/static/{template}/")));
        }
        let field = match name.as_str() {
            "SiteName" => "site_name",
            "SiteLogo" => "site_logo",
            "BaseUrl" => "base_url",
            "SiteIcp" => "site_icp",
            "Language" => "language",
            _ => "",
        };
        Ok(Value::String(
            system.get(field).map(super::as_text).unwrap_or_default(),
        ))
    }

    async fn nav_list(&self, args: &Map<String, Value>) -> Result<Value> {
        let type_id = args
            .get("typeId")
            .map(|value| super::as_text(value).parse::<u32>().unwrap_or(1))
            .unwrap_or(1);
        let rows: Vec<(String, String)> = sqlx::query_as(
            "SELECT title, link FROM navs WHERE type_id = ? AND status = 1 ORDER BY sort ASC, id ASC",
        )
        .bind(type_id)
        .fetch_all(&self.pool)
        .await?;
        let list: Vec<Value> = rows
            .into_iter()
            .map(|(title, link)| {
                let mut map = Map::new();
                map.insert("Title".into(), Value::String(title));
                map.insert("Link".into(), Value::String(link));
                Value::Object(map)
            })
            .collect();
        Ok(Value::Array(list))
    }

    /// 首页 / 模块列表页的 TDK（`settings.index` 的 SEO + 站点名）。
    async fn tdk_site(&self, name: &str) -> Result<Value> {
        let index_raw: Option<String> =
            sqlx::query_scalar("SELECT value FROM settings WHERE `key` = 'index'")
                .fetch_optional(&self.pool)
                .await?;
        let index: Value = index_raw
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        let system_raw: Option<String> =
            sqlx::query_scalar("SELECT value FROM settings WHERE `key` = 'system'")
                .fetch_optional(&self.pool)
                .await?;
        let system: Value = system_raw
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or(Value::Null);
        let site_name = system
            .get("site_name")
            .map(super::as_text)
            .unwrap_or_default();

        let value = match name {
            "Title" => {
                if self.module_id > 0 {
                    let modules = crate::models::module::list(&self.pool).await?;
                    match modules.iter().find(|item| item.id == self.module_id) {
                        Some(item) if !item.title.trim().is_empty() => item.title.clone(),
                        Some(item) => format!("{} - {site_name}", item.name),
                        None => site_name.clone(),
                    }
                } else {
                    let seo = index
                        .get("seo_title")
                        .map(super::as_text)
                        .unwrap_or_default();
                    if seo.trim().is_empty() {
                        site_name.clone()
                    } else {
                        seo
                    }
                }
            }
            "Keywords" => index
                .get("seo_keywords")
                .map(super::as_text)
                .unwrap_or_default(),
            "Description" => index
                .get("seo_description")
                .map(super::as_text)
                .unwrap_or_default(),
            "CanonicalUrl" => "/".to_string(),
            _ => String::new(),
        };
        Ok(Value::String(value))
    }

    async fn tdk(&self, args: &Map<String, Value>) -> Result<Value> {
        let name = args.get("name").map(super::as_text).unwrap_or_default();

        // 标签详情页：用标签自身的 SEO 字段
        if let Some(item) = &self.tag {
            let value = match name.as_str() {
                "Title" => {
                    if item.seo_title.is_empty() {
                        item.title.clone()
                    } else {
                        item.seo_title.clone()
                    }
                }
                "Keywords" => item.keywords.clone(),
                "Description" => item.description.clone(),
                "CanonicalUrl" => url::tag_url(self.mode, item.id, &item.url_token, 1),
                _ => String::new(),
            };
            return Ok(Value::String(value));
        }

        // 首页 / 模块列表页：用 settings.index 的 SEO 或站点名
        if self.category_id == 0 {
            return self.tdk_site(&name).await;
        }

        // 详情页优先用内容自身的 SEO 字段
        if let Some(item) = self.archive.as_ref() {
            let value = match name.as_str() {
                "Title" => {
                    if item.seo_title.is_empty() {
                        item.title.clone()
                    } else {
                        item.seo_title.clone()
                    }
                }
                "Keywords" => item.keywords.clone(),
                "Description" => item.description.clone(),
                "CanonicalUrl" => {
                    let (categories, modules) = self.indexes().await?;
                    self.link_for(item, &categories, &modules)
                }
                _ => String::new(),
            };
            return Ok(Value::String(value));
        }

        let Some(current) = category::get(&self.pool, self.category_id).await? else {
            return Ok(Value::Null);
        };
        let value = match name.as_str() {
            "Title" => {
                if current.seo_title.is_empty() {
                    current.title.clone()
                } else {
                    current.seo_title.clone()
                }
            }
            "Keywords" => current.keywords.clone(),
            "Description" => current.description.clone(),
            "CanonicalUrl" => {
                url::category_url(self.mode, &current.url_token, "", current.id, self.page)
            }
            _ => String::new(),
        };
        Ok(Value::String(value))
    }
}

impl Provider for SiteProvider {
    fn data<'a>(
        &'a self,
        tag: &'a str,
        args: &'a Map<String, Value>,
    ) -> Pin<Box<dyn Future<Output = Result<Value>> + Send + 'a>> {
        Box::pin(async move {
            match tag {
                "archiveList" => self.archive_list(args).await,
                "archiveDetail" => self.archive_detail(args).await,
                "categoryDetail" => self.category_detail(args).await,
                "categoryList" => self.category_list(args).await,
                "moduleDetail" => self.module_detail(args).await,
                "pageDetail" => self.page_detail(args).await,
                "bannerList" => self.banner_list().await,
                "linkList" => self.link_list().await,
                "pagination" => self.pagination(args).await,
                "system" => self.system(args).await,
                "navList" => self.nav_list(args).await,
                "tdk" => self.tdk(args).await,
                "tagList" => self.tag_list(args).await,
                "tagDetail" => self.tag_detail(args).await,
                "tagDataList" => self.archive_list(args).await,
                "contact" => self.contact(args).await,
                "prevArchive" => self.adjacent_archive(args, true).await,
                "nextArchive" => self.adjacent_archive(args, false).await,
                "now" => Ok(self.now_value(args)),
                // 翻译：按当前站点语言查语言包（`locales/<lang>/default.yml`），
                // 缺词条回退词条名本身，并做 %d/%s 占位替换
                "tr" => {
                    let key = args.get("__arg0").map(super::as_text).unwrap_or_default();
                    let lang = self.current_language().await;
                    let pack = super::language_pack(&lang);
                    let mut text = pack.get(&key).cloned().unwrap_or(key);
                    let mut index = 1usize;
                    while let Some(value) = args.get(&format!("__arg{index}")) {
                        let value = super::as_text(value);
                        if text.contains("%d") {
                            text = text.replacen("%d", &value, 1);
                        } else if text.contains("%s") {
                            text = text.replacen("%s", &value, 1);
                        }
                        index += 1;
                    }
                    Ok(Value::String(text))
                }
                // 多语言子站列表（语言切换器 / alternate 链接）
                "languages" => self.languages_list().await,
                _ => Ok(Value::Null),
            }
        })
    }
}

impl SiteProvider {
    /// 当前站点语言（`settings.system.language`，归一到 `zh-cn` / `en-us` 等）。
    async fn current_language(&self) -> String {
        self.language
            .get_or_init(|| async {
                let system = site_setting::get_section(&self.pool, site_setting::SYSTEM)
                    .await
                    .unwrap_or_default();
                let lang = system
                    .get("language")
                    .and_then(Value::as_str)
                    .unwrap_or("zh-cn");
                super::normalize_language(lang)
            })
            .await
            .clone()
    }

    /// 多语言子站列表（对齐 `GetLanguages` 的语义）：
    /// 主站（id=1）永远第一条且 `IsCurrent = true`，子站链接按部署形态生成。
    async fn languages_list(&self) -> Result<Value> {
        let config = multilang::config_get(&self.pool).await?;
        let open = config.get("open").and_then(Value::as_bool).unwrap_or(false);
        if !open {
            // 既有实现：多语言未开启时语言切换器不渲染（空数组）
            return Ok(Value::Array(Vec::new()));
        }
        let site_type = config
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or("directory")
            .to_string();
        let default_language = config
            .get("default_language")
            .and_then(Value::as_str)
            .unwrap_or("zh-cn")
            .to_string();

        let mut items: Vec<Value> = Vec::new();
        // 主站条目（切换回主站）
        let main_icon = website::get(&self.pool, 1)
            .await
            .ok()
            .flatten()
            .map(|site| site.language_icon)
            .unwrap_or_default();
        items.push(serde_json::json!({
            "LanguageName": super::language_display_name(&default_language),
            "LanguageEmoji": super::language_emoji(&default_language),
            "LanguageIcon": main_icon,
            "Link": "/",
            "IsCurrent": true,
        }));

        // 子站条目
        let subs = config
            .get("sub_sites")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        for sub in subs {
            let language = sub
                .get("language")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if language.is_empty() {
                continue;
            }
            let id = sub.get("id").and_then(Value::as_u64).unwrap_or(0) as u32;
            let (root_path, site_icon) = match website::get(&self.pool, id).await {
                Ok(Some(site)) => (site.root_path, site.language_icon),
                _ => (String::new(), String::new()),
            };
            let icon = sub
                .get("language_icon")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty())
                .map(str::to_string)
                .unwrap_or(site_icon);
            // directory 部署：`/{root_path}/`；domain 部署：子站域名由部署侧解析，回退主站链接
            let link = if site_type == "domain" {
                "/".to_string()
            } else {
                let root = root_path.trim().trim_matches('/');
                if root.is_empty() {
                    format!("/{}/", language)
                } else {
                    format!("/{root}/")
                }
            };
            items.push(serde_json::json!({
                "LanguageName": super::language_display_name(&language),
                "LanguageEmoji": super::language_emoji(&language),
                "LanguageIcon": icon,
                "Link": link,
                "IsCurrent": false,
            }));
        }
        Ok(Value::Array(items))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::db_lock;
    use crate::models::test_pool;
    use crate::template::Engine;

    fn template_dir() -> std::path::PathBuf {
        crate::models::test_template_dir()
    }

    #[tokio::test]
    async fn languages_tag_follows_multilang_config() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = crate::models::setting::get_raw(&pool, "multilang").await.unwrap();
        crate::models::setting::remove(&pool, "multilang").await.unwrap();
        let provider = SiteProvider::new(pool.clone(), 0).await.unwrap();

        // 多语言未开启：空数组（语言切换器不渲染）
        let data = provider
            .data("languages", &Map::new())
            .await
            .unwrap();
        assert!(data.as_array().unwrap().is_empty(), "{data}");

        // 开启（无子站）：主站条目 IsCurrent=true
        crate::models::setting::save_json(
            &pool,
            "multilang",
            &serde_json::json!({"open": true, "type": "directory", "sub_sites": []}),
        )
        .await
        .unwrap();
        let data = provider
            .data("languages", &Map::new())
            .await
            .unwrap();
        let list = data.as_array().unwrap();
        assert_eq!(list.len(), 1, "{data}");
        assert_eq!(list[0]["IsCurrent"], serde_json::json!(true));
        assert_eq!(list[0]["Link"], serde_json::json!("/"));
        assert!(!list[0]["LanguageName"].as_str().unwrap_or("").is_empty());

        // 带子站：directory 部署按 root_path 生成链接
        crate::models::setting::save_json(
            &pool,
            "multilang",
            &serde_json::json!({
                "open": true,
                "type": "directory",
                "sub_sites": [
                    {"id": 999999, "language": "en-us", "language_icon": ""}
                ]
            }),
        )
        .await
        .unwrap();
        let data = provider
            .data("languages", &Map::new())
            .await
            .unwrap();
        let list = data.as_array().unwrap();
        assert_eq!(list.len(), 2, "{data}");
        // 子站 id 不存在：root_path 为空 → 回退语言代码目录
        assert_eq!(list[1]["Link"], serde_json::json!("/en-us/"));
        assert_eq!(list[1]["IsCurrent"], serde_json::json!(false));
        assert_eq!(list[1]["LanguageName"], serde_json::json!("English"));

        // 恢复
        match backup {
            Some(raw) => crate::models::setting::save_raw(&pool, "multilang", &raw)
                .await
                .unwrap(),
            None => crate::models::setting::remove(&pool, "multilang").await.unwrap(),
        }
    }

    #[tokio::test]
    async fn renders_real_list_template() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        let categories = category::list_all(&pool).await.unwrap();
        let Some(target) = categories
            .iter()
            .find(|item| item.kind == 1 && item.archive_count > 0)
        else {
            return;
        };

        let provider = SiteProvider::new(pool.clone(), target.id).await.unwrap();
        let engine = Engine::new(template_dir());
        let html = engine
            .render("article/list.html", &Map::new(), &provider)
            .await
            .unwrap();

        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains(&target.title));
        assert!(!html.contains("{%"), "残留模板标签");
        assert!(!html.contains("{{"), "残留变量占位");
        // 链接走伪静态规则：/{catname}/{id}.html
        assert!(
            html.contains(&format!("/{}/", target.url_token)),
            "链接未按规则生成"
        );
    }

    #[tokio::test]
    async fn renders_real_detail_template() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        let Some((sample, _)) = archive::detail(&pool, 1).await.unwrap() else {
            return;
        };
        let Some(provider) = SiteProvider::for_archive(pool.clone(), sample.id)
            .await
            .unwrap()
        else {
            return;
        };

        let engine = Engine::new(template_dir());
        let html = engine
            .render("article/detail.html", &Map::new(), &provider)
            .await
            .unwrap();

        assert!(html.contains("<!DOCTYPE html>"));
        assert!(html.contains(&sample.title), "标题没渲染出来");
        assert!(html.contains("<h1>"), "详情页结构缺失");
        assert!(!html.contains("{%"));
        assert!(!html.contains("{{"));
    }
}
