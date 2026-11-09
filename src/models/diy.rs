//! 可视化装修（表 `diy_pages`）。
//!
//! 约定：
//! - 一个页面 = 若干「区块」(`DiyModule`)，区块类型固定，字段由 [`module_types`] 描述；
//! - 页面数据只存一张表，`modules` 列是 JSON 数组；
//! - 只接受已知页面 key（目前仅 `home`），防止后台被写入任意键。

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;

/// 可装修的页面（`value` 是入库的 key）。
pub const PAGE_KEYS: &[(&str, &str)] = &[("网站首页", "home")];

/// 可装修页面列表（`{label, value}`）。
pub fn page_keys() -> Vec<Value> {
    PAGE_KEYS
        .iter()
        .map(|(label, value)| json!({ "label": label, "value": value }))
        .collect()
}

/// 全部可用区块类型（顺序即后台模块库顺序）。
pub fn module_types() -> Value {
    json!([
        {
            "type": "banner",
            "title": "轮播图",
            "icon": "🖼",
            "desc": "顶部大图轮播，可配多张",
            "fields": [
                {
                    "name": "items",
                    "label": "图片",
                    "type": "list",
                    "tip": "建议 1920×600，第一张为首屏图",
                    "fields": [
                        {"name": "image", "label": "图片", "type": "image"},
                        {"name": "link", "label": "跳转链接", "type": "text", "placeholder": "留空则不跳转"},
                        {"name": "title", "label": "标题", "type": "text", "placeholder": "可留空"}
                    ]
                },
                {"name": "height", "label": "高度(px)", "type": "number", "default": 420}
            ]
        },
        {
            "type": "notice",
            "title": "公告条",
            "icon": "📢",
            "desc": "一行提示文字，可带链接",
            "fields": [
                {"name": "text", "label": "内容", "type": "text", "placeholder": "例如：双十一全场 8 折，满 999 减 100"},
                {"name": "link", "label": "链接", "type": "text", "placeholder": "可留空"},
                {"name": "color", "label": "文字颜色", "type": "color", "default": "#0068AB"}
            ]
        },
        {
            "type": "richtext",
            "title": "富文本",
            "icon": "📝",
            "desc": "自由排版，支持 HTML",
            "fields": [
                {"name": "title", "label": "区块标题", "type": "text", "placeholder": "可留空"},
                {"name": "html", "label": "内容", "type": "richtext", "tip": "支持 HTML 标签"}
            ]
        },
        {
            "type": "image",
            "title": "单图",
            "icon": "🏞",
            "desc": "一张通栏或居中的图片",
            "fields": [
                {"name": "image", "label": "图片", "type": "image"},
                {"name": "link", "label": "跳转链接", "type": "text", "placeholder": "可留空"},
                {
                    "name": "align",
                    "label": "对齐",
                    "type": "select",
                    "default": "center",
                    "options": [{"label": "居中", "value": "center"}, {"label": "通栏", "value": "full"}]
                }
            ]
        },
        {
            "type": "nav",
            "title": "图文导航",
            "icon": "🧭",
            "desc": "图标 + 标题 + 链接的宫格",
            "fields": [
                {"name": "title", "label": "区块标题", "type": "text", "placeholder": "可留空"},
                {
                    "name": "items",
                    "label": "导航项",
                    "type": "list",
                    "fields": [
                        {"name": "icon", "label": "图标图片", "type": "image"},
                        {"name": "title", "label": "标题", "type": "text"},
                        {"name": "link", "label": "链接", "type": "text"}
                    ]
                }
            ]
        },
        {
            "type": "products",
            "title": "产品列表",
            "icon": "📦",
            "desc": "按栏目取产品，卡片网格",
            "fields": [
                {"name": "title", "label": "区块标题", "type": "text", "default": "热门产品"},
                {"name": "category_id", "label": "栏目 ID", "type": "number", "tip": "留空表示全部产品"},
                {"name": "limit", "label": "数量", "type": "number", "default": 8}
            ]
        },
        {
            "type": "articles",
            "title": "文章列表",
            "icon": "📰",
            "desc": "按栏目取文章，标题 + 摘要",
            "fields": [
                {"name": "title", "label": "区块标题", "type": "text", "default": "新闻资讯"},
                {"name": "category_id", "label": "栏目 ID", "type": "number", "tip": "留空表示全部文章"},
                {"name": "limit", "label": "数量", "type": "number", "default": 6}
            ]
        },
        {
            "type": "divider",
            "title": "分隔/留白",
            "icon": "➖",
            "desc": "控制区块之间的间距",
            "fields": [
                {"name": "height", "label": "高度(px)", "type": "number", "default": 40}
            ]
        }
    ])
}

/// 装修页面。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct DiyPage {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    #[sqlx(rename = "key")]
    pub key: String,
    pub title: String,
    pub modules: Option<String>,
    pub status: i8,
}

/// 一个区块。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiyModule {
    #[serde(default)]
    pub id: String,
    #[serde(rename = "type", default)]
    pub kind: String,
    #[serde(default)]
    pub settings: serde_json::Map<String, Value>,
}

fn page_key_allowed(key: &str) -> bool {
    PAGE_KEYS.iter().any(|(_, value)| *value == key)
}

/// 取页面配置。
pub async fn get_page(pool: &MySqlPool, key: &str) -> anyhow::Result<Option<DiyPage>> {
    Ok(
        sqlx::query_as::<_, DiyPage>("SELECT * FROM diy_pages WHERE `key` = ? LIMIT 1")
            .bind(key)
            .fetch_optional(pool)
            .await?,
    )
}

/// 解析出区块列表。
pub async fn modules(pool: &MySqlPool, key: &str) -> anyhow::Result<Vec<DiyModule>> {
    let Some(page) = get_page(pool, key).await? else {
        return Ok(Vec::new());
    };
    let Some(raw) = page.modules.map(|value| value.trim().to_string()).filter(|v| !v.is_empty()) else {
        return Ok(Vec::new());
    };
    Ok(serde_json::from_str::<Vec<DiyModule>>(&raw).unwrap_or_default())
}

/// 保存页面区块，返回区块数量。
pub async fn save_page(
    pool: &MySqlPool,
    key: &str,
    title: &str,
    modules: &[DiyModule],
) -> Result<usize, ModelError> {
    let key = key.trim();
    if !page_key_allowed(key) {
        return Err(ModelError::invalid("非法操作"));
    }

    // 补齐 id / settings，保证前端排序稳定
    let mut prepared: Vec<DiyModule> = modules.to_vec();
    let total = prepared.len();
    for (index, module) in prepared.iter_mut().enumerate() {
        if module.id.trim().is_empty() {
            module.id = format!("{}-{}-{}", module.kind, index, total);
        }
    }

    let raw = serde_json::to_string(&prepared)?;
    let now = chrono::Utc::now().timestamp();
    let title = if title.trim().is_empty() { key } else { title.trim() };

    if let Some(page) = get_page(pool, key).await? {
        sqlx::query(
            "UPDATE diy_pages SET title = ?, modules = ?, updated_time = ?, status = 1 WHERE id = ?",
        )
        .bind(title)
        .bind(&raw)
        .bind(now)
        .bind(page.id)
        .execute(pool)
        .await?;
    } else {
        sqlx::query(
            "INSERT INTO diy_pages (created_time, updated_time, `key`, title, modules, status) \
             VALUES (?, ?, ?, ?, ?, 1)",
        )
        .bind(now)
        .bind(now)
        .bind(key)
        .bind(title)
        .bind(&raw)
        .execute(pool)
        .await?;
    }
    Ok(prepared.len())
}

/// 清空装修（前台回退到模板默认内容）。
pub async fn reset_page(pool: &MySqlPool, key: &str) -> Result<(), ModelError> {
    let key = key.trim();
    if !page_key_allowed(key) {
        return Err(ModelError::invalid("非法操作"));
    }
    let Some(page) = get_page(pool, key).await? else {
        return Ok(());
    };
    sqlx::query("UPDATE diy_pages SET modules = '[]', updated_time = ? WHERE id = ?")
        .bind(chrono::Utc::now().timestamp())
        .bind(page.id)
        .execute(pool)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn exposes_page_keys_and_types() {
        let keys = page_keys();
        assert_eq!(keys.len(), 1);
        assert_eq!(keys[0]["value"], "home");
        let types = module_types();
        assert_eq!(types.as_array().unwrap().len(), 8);
        assert!(page_key_allowed("home"));
        assert!(!page_key_allowed("admin"));
    }

    #[tokio::test]
    async fn save_read_and_reset_diy_page() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        // 备份原值，用例结束还原
        let backup = get_page(&pool, "home").await.unwrap();

        let blocks = vec![
            DiyModule {
                id: String::new(),
                kind: "notice".into(),
                settings: serde_json::Map::new(),
            },
            DiyModule {
                id: String::new(),
                kind: "divider".into(),
                settings: serde_json::Map::new(),
            },
        ];
        let count = save_page(&pool, "home", "首页", &blocks).await.unwrap();
        assert_eq!(count, 2);

        let loaded = modules(&pool, "home").await.unwrap();
        assert_eq!(loaded.len(), 2);
        // 缺省 id 被自动补齐
        assert!(!loaded[0].id.is_empty(), "{:?}", loaded[0].id);

        // 非法页面 key 被拒
        assert!(save_page(&pool, "admin", "", &[]).await.is_err());

        reset_page(&pool, "home").await.unwrap();
        assert!(modules(&pool, "home").await.unwrap().is_empty());

        // 还原
        match backup {
            Some(page) => {
                sqlx::query("UPDATE diy_pages SET title = ?, modules = ?, status = ? WHERE id = ?")
                    .bind(&page.title)
                    .bind(&page.modules)
                    .bind(page.status)
                    .bind(page.id)
                    .execute(&pool)
                    .await
                    .unwrap();
            }
            None => {
                let _ = sqlx::query("DELETE FROM diy_pages WHERE `key` = 'home'")
                    .execute(&pool)
                    .await;
            }
        }
    }
}
