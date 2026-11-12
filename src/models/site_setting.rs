//! 站点设置（`settings` 表里的各个配置段）。
//!
//! 行为约定：
//! - `system` / `content` / `index` / `contact` / `safe` / `ai_setting` / `front` 存在
//! `settings` 表，值是 JSON 对象，保存走「字段级 merge」；
//! - `diy_fields` 是 JSON **数组**，保存整份覆盖；
//! - `banner` 形如 `{"banner":[{"type":"default","list":[{...}]}]}`；
//! - `cache_type` 是**纯字符串**（`file` / `memory`），不是 JSON。

use serde_json::{json, Value};
use sqlx::MySqlPool;

use super::{setting, ModelError};

pub const SYSTEM: &str = "system";
pub const CONTENT: &str = "content";
pub const INDEX: &str = "index";
pub const CONTACT: &str = "contact";
pub const SAFE: &str = "safe";
pub const BANNER: &str = "banner";
pub const DIY_FIELDS: &str = "diy_fields";
pub const CACHE_TYPE: &str = "cache_type";
pub const AI: &str = "ai_setting";
/// 前台展示配置（导航/区块开关、页脚栏目、首页内容）。
pub const FRONT: &str = "front";

/// 读一个对象型配置段；不存在或不是对象时返回 `{}`。
pub async fn get_section(pool: &MySqlPool, key: &str) -> anyhow::Result<Value> {
    Ok(setting::get_json(pool, key)
        .await?
        .filter(Value::is_object)
        .unwrap_or_else(|| json!({})))
}

/// 读一个数组型配置段；不存在或不是数组时返回 `[]`。
pub async fn get_array(pool: &MySqlPool, key: &str) -> anyhow::Result<Value> {
    Ok(setting::get_json(pool, key)
        .await?
        .filter(Value::is_array)
        .unwrap_or_else(|| json!([])))
}

/// 字段级合并保存（只覆盖 `patch` 里出现的顶层字段）。
pub async fn save_section(
    pool: &MySqlPool,
    key: &str,
    patch: &Value,
) -> anyhow::Result<Value> {
    if !patch.is_object() {
        return Err(anyhow::anyhow!("配置必须是 JSON 对象"));
    }
    setting::merge_json(pool, key, patch).await
}

/// 整份覆盖（数组 / 标量配置用）。
pub async fn save_value(pool: &MySqlPool, key: &str, value: &Value) -> anyhow::Result<()> {
    setting::save_json(pool, key, value).await
}

// ---------------------------- 前台展示配置（front） ---------------------------

/// 读前台展示配置：`settings.front` 与默认值合并（缺字段用默认，数组整体覆盖）。
pub async fn get_front_ui(pool: &MySqlPool) -> anyhow::Result<Value> {
    let stored = get_section(pool, FRONT).await?;
    Ok(merge_front_ui(front_defaults(), stored))
}

/// 前台展示配置默认值（与默认模板的硬编码内容一致，未配置时前台外观不变）。
pub fn front_defaults() -> Value {
    json!({
        "blocks": {
            "topbar": true,
            "search": true,
            "header_cta": true,
            "home_entries": true,
            "home_features": true,
            "home_scenes": true,
            "home_cases": true,
            "home_stats": true,
            "home_news": true,
            "flink": true,
            "footer_columns": true,
            "footer_contact": true,
            "beian": true,
        },
        "header": { "cta_text": "免费试用", "cta_url": "/products/3/" },
        "hero": {
            "badge": "RustCMS 企业数字化平台",
            "title": "让企业协作与经营<br>像使用 Rust 一样简单",
            "subtitle": "一体化 ERP、协同办公、数据分析与低代码平台，帮助团队高效协作、敏捷经营。",
            "btn1_text": "免费试用",
            "btn1_url": "/products/3/",
            "btn2_text": "预约演示",
            "btn2_url": "/news/12/",
        },
        "slides": [
            { "image": "/uploads/software/theme-01.png", "title": "RustCMS ERP 产品界面" },
            { "image": "/uploads/software/theme-03.png", "title": "RustCMS 数据可视化大屏" },
            { "image": "/uploads/software/theme-09.png", "title": "RustCMS 数据中台" },
        ],
        "home_cards": [
            { "icon": "fa-cubes", "title": "企业管理软件", "desc": "ERP · 财务", "url": "/products/3/" },
            { "icon": "fa-comments-o", "title": "协同办公", "desc": "审批 · 文档 · 会议", "url": "/products/4/" },
            { "icon": "fa-bar-chart", "title": "数据分析", "desc": "BI · 指标 · 大屏", "url": "/products/5/" },
            { "icon": "fa-code", "title": "定制开发", "desc": "门户 · 集成 · 迁移", "url": "/products/6/" },
            { "icon": "fa-briefcase", "title": "客户案例", "desc": "行业落地实践", "url": "/cases/15/" },
            { "icon": "fa-newspaper-o", "title": "新闻资讯", "desc": "产品与行业动态", "url": "/news/" },
        ],
        "stats": [
            { "num": "500+", "text": "服务客户" },
            { "num": "98%", "text": "客户满意度" },
            { "num": "20+", "text": "覆盖行业" },
            { "num": "7×12", "text": "服务响应" },
        ],
        "scenes": [
            {
                "tag": "管理数字化",
                "title": "一个视图看清企业经营",
                "desc": "以 ERP 为核心，打通财务、供应链、生产与销售数据，建立统一的主数据和经营看板，让管理决策有据可依。",
                "link_text": "了解更多 →",
                "url": "/news/12/",
                "image": "/uploads/software/theme-03.png",
                "reverse": false,
            },
            {
                "tag": "协同数字化",
                "title": "审批、文档、会议一站完成",
                "desc": "以 OA 和工作流为入口，连接组织、流程与文档，支持移动办公与多端同步，让协作不再依赖即时消息。",
                "link_text": "了解更多 →",
                "url": "/products/4/",
                "image": "/uploads/software/theme-02.png",
                "reverse": true,
            },
            {
                "tag": "数据智能化",
                "title": "从数据到洞察只需几分钟",
                "desc": "数据中台完成采集、治理与建模，BI 可视化让业务人员自助分析，指标实时触达管理者。",
                "link_text": "了解更多 →",
                "url": "/products/5/",
                "image": "/uploads/software/theme-09.png",
                "reverse": false,
            },
        ],
        "footer_columns": [
            {
                "title": "产品",
                "links": [
                    { "text": "企业管理软件", "url": "/products/3/" },
                    { "text": "协同办公", "url": "/products/4/" },
                    { "text": "数据分析", "url": "/products/5/" },
                    { "text": "制造执行", "url": "/products/8/" },
                    { "text": "客户关系", "url": "/products/9/" },
                ],
            },
            {
                "title": "解决方案",
                "links": [
                    { "text": "数字化转型总览", "url": "/news/12/" },
                    { "text": "制造企业数字化", "url": "/news/17.html" },
                    { "text": "智能客服与知识库", "url": "/news/18.html" },
                    { "text": "供应链协同", "url": "/news/20.html" },
                    { "text": "低代码应用", "url": "/news/67.html" },
                ],
            },
            {
                "title": "资源中心",
                "links": [
                    { "text": "新闻资讯", "url": "/news/" },
                    { "text": "技术分享", "url": "/news/14/" },
                    { "text": "客户案例", "url": "/cases/15/" },
                    { "text": "网站地图", "url": "/sitemap.xml" },
                ],
            },
            {
                "title": "关于我们",
                "links": [
                    { "text": "公司介绍", "url": "/1.html" },
                    { "text": "联系我们", "url": "/2.html" },
                    { "text": "生态伙伴", "url": "/news/64.html" },
                ],
            },
        ],
        "footer_copyright": "",
    })
}

/// 把后台保存的前台配置合并到默认值上：
/// 对象逐层合并（缺字段用默认），数组与标量整体覆盖。
pub fn merge_front_ui(mut base: Value, over: Value) -> Value {
    match (&mut base, over) {
        (Value::Object(base_map), Value::Object(over_map)) => {
            for (key, value) in over_map {
                match base_map.get_mut(&key) {
                    // 两边都是对象：再合并一层（blocks / hero / header）
                    Some(Value::Object(inner)) if value.is_object() => {
                        if let Value::Object(over_inner) = value {
                            for (k, v) in over_inner {
                                inner.insert(k, v);
                            }
                        }
                    }
                    _ => {
                        base_map.insert(key, value);
                    }
                }
            }
            base
        }
        _ => base,
    }
}

/// 读缓存类型（纯字符串，非 JSON）。
pub async fn get_cache_type(pool: &MySqlPool) -> anyhow::Result<String> {
    Ok(setting::get_raw(pool, CACHE_TYPE)
        .await?
        .unwrap_or_default()
        .trim()
        .trim_matches('"')
        .to_string())
}

/// 写缓存类型（纯字符串）。
pub async fn save_cache_type(pool: &MySqlPool, value: &str) -> anyhow::Result<()> {
    setting::save_raw(pool, CACHE_TYPE, value.trim()).await
}

/// Banner 列表（`{"banner":[...]}` 里的 `banner` 数组）。
pub async fn banner_list(pool: &MySqlPool) -> anyhow::Result<Value> {
    let config = get_section(pool, BANNER).await?;
    Ok(config
        .get("banner")
        .filter(|value| value.is_array())
        .cloned()
        .unwrap_or_else(|| json!([])))
}

fn group_list<'a>(config: &'a Value, kind: &str) -> Vec<&'a Value> {
    config
        .get("banner")
        .and_then(Value::as_array)
        .map(|groups| {
            groups
                .iter()
                .filter(|group| group.get("type").and_then(Value::as_str) == Some(kind))
                .collect()
        })
        .unwrap_or_default()
}

fn max_item_id(group: &Value) -> i64 {
    group
        .get("list")
        .and_then(Value::as_array)
        .map(|list| {
            list.iter()
                .filter_map(|item| item.get("id").and_then(Value::as_i64))
                .max()
                .unwrap_or(0)
        })
        .unwrap_or(0)
}

/// 新增 / 更新一条 Banner（`id = 0` 表示新增）。
///
/// 返回保存后的条目。
pub async fn banner_save(
    pool: &MySqlPool,
    item: &Value,
    type_hint: &str,
) -> Result<Value, ModelError> {
    if !item.is_object() {
        return Err(ModelError::invalid("请选择图片"));
    }
    let logo = item.get("logo").and_then(Value::as_str).unwrap_or("").trim();
    if logo.is_empty() {
        return Err(ModelError::invalid("请选择图片"));
    }

    let mut saved_item = item.clone();
    let kind = item
        .get("type")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or(type_hint)
        .to_string();
    let kind = if kind.is_empty() { "default".to_string() } else { kind };

    if let Value::Object(map) = &mut saved_item {
        map.insert("type".into(), Value::String(kind.clone()));
    }
    let item_id = item.get("id").and_then(Value::as_i64).unwrap_or(0);

    let mut config = get_section(pool, BANNER).await?;
    if !config.get("banner").is_some_and(Value::is_array) {
        config = json!({ "banner": [] });
    }

    let groups = config["banner"].as_array_mut().expect("刚保证是数组");
    let index = groups
        .iter()
        .position(|group| group.get("type").and_then(Value::as_str) == Some(kind.as_str()));

    match index {
        Some(group_index) => {
            if !groups[group_index].get("list").is_some_and(Value::is_array) {
                groups[group_index]["list"] = json!([]);
            }
            let list = groups[group_index]["list"].as_array_mut().expect("刚保证是数组");
            if item_id == 0 {
                let next_id = list
                    .iter()
                    .filter_map(|entry| entry.get("id").and_then(Value::as_i64))
                    .max()
                    .unwrap_or(0)
                    + 1;
                if let Value::Object(map) = &mut saved_item {
                    map.insert("id".into(), json!(next_id));
                }
                list.push(saved_item.clone());
            } else {
                let target = list
                    .iter()
                    .position(|entry| entry.get("id").and_then(Value::as_i64) == Some(item_id));
                match target {
                    Some(position) => list[position] = saved_item.clone(),
                    None => {
                        // 传了 id 但不存在：按新增处理
                        if let Value::Object(map) = &mut saved_item {
                            map.insert("id".into(), json!(item_id));
                        }
                        list.push(saved_item.clone());
                    }
                }
            }
        }
        None => {
            let next_id = if item_id == 0 { 1 } else { item_id };
            if let Value::Object(map) = &mut saved_item {
                map.insert("id".into(), json!(next_id));
            }
            groups.push(json!({
                "type": kind,
                "list": [saved_item.clone()],
            }));
        }
    }

    save_value(pool, BANNER, &config).await?;
    Ok(saved_item)
}

/// 删除一条 Banner，返回是否删掉。
pub async fn banner_delete(pool: &MySqlPool, id: i64, kind: &str) -> Result<bool, ModelError> {
    if id == 0 {
        return Err(ModelError::invalid("Banner 不存在"));
    }

    let mut config = get_section(pool, BANNER).await?;
    let Some(groups) = config.get_mut("banner").and_then(Value::as_array_mut) else {
        return Ok(false);
    };

    let mut removed = false;
    let mut empty_group: Option<usize> = None;
    let mut group_type = String::new();

    for (group_index, group) in groups.iter_mut().enumerate() {
        let this_type = group
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if !kind.is_empty() && this_type != kind {
            continue;
        }
        let Some(list) = group.get_mut("list").and_then(Value::as_array_mut) else {
            continue;
        };
        if let Some(position) = list
            .iter()
            .position(|entry| entry.get("id").and_then(Value::as_i64) == Some(id))
        {
            list.remove(position);
            removed = true;
            if list.is_empty() {
                empty_group = Some(group_index);
                group_type = this_type;
            }
            break;
        }
    }

    if let Some(index) = empty_group {
        if group_type != "default" {
            groups.remove(index);
        }
    }

    if !removed {
        return Ok(false);
    }
    save_value(pool, BANNER, &config).await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn banner_helpers_handle_missing_fields() {
        let empty = json!({});
        assert!(group_list(&empty, "default").is_empty());
        assert_eq!(max_item_id(&empty), 0);
        assert_eq!(
            max_item_id(&json!({"list":[{"id":2},{"id":7}]})),
            7
        );
    }

    #[tokio::test]
    async fn section_roundtrip_and_restore() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let key = "__rustcms_test_section__";
        let backup = setting::get_raw(&pool, key).await.unwrap();

        let saved = save_section(&pool, key, &json!({"site_name": "A", "language": "zh"}))
            .await
            .unwrap();
        assert_eq!(saved["site_name"], json!("A"));

        // 字段级 merge：只改 site_name
        let merged = save_section(&pool, key, &json!({"site_name": "B"}))
            .await
            .unwrap();
        assert_eq!(merged["site_name"], json!("B"));
        assert_eq!(merged["language"], json!("zh"));

        // 非对象被拒
        assert!(save_section(&pool, key, &json!([])).await.is_err());

        match backup {
            Some(raw) => setting::save_raw(&pool, key, &raw).await.unwrap(),
            None => setting::remove(&pool, key).await.unwrap(),
        }
    }

    #[tokio::test]
    async fn banner_crud_roundtrip() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let backup = setting::get_raw(&pool, BANNER).await.unwrap();

        // 缺少 logo 被拒
        assert!(banner_save(&pool, &json!({"type": "rustcms-test"}), "")
            .await
            .is_err());

        let saved = banner_save(
            &pool,
            &json!({"logo": "/uploads/a.webp", "link": "/x", "type": "rustcms-test"}),
            "",
        )
        .await
        .unwrap();
        assert_eq!(saved["id"], json!(1));

        let saved2 = banner_save(
            &pool,
            &json!({"logo": "/uploads/b.webp", "type": "rustcms-test"}),
            "",
        )
        .await
        .unwrap();
        assert_eq!(saved2["id"], json!(2));

        // 更新第一条
        let updated = banner_save(
            &pool,
            &json!({"id": 1, "logo": "/uploads/c.webp", "type": "rustcms-test"}),
            "",
        )
        .await
        .unwrap();
        assert_eq!(updated["logo"], json!("/uploads/c.webp"));

        let list = banner_list(&pool).await.unwrap();
        assert!(list.as_array().unwrap().iter().any(|group| {
            group["type"] == json!("rustcms-test") && group["list"].as_array().unwrap().len() == 2
        }));

        // 删除一条
        assert!(banner_delete(&pool, 2, "rustcms-test").await.unwrap());
        // 再删一条后组被清空并移除（非 default 组）
        assert!(banner_delete(&pool, 1, "rustcms-test").await.unwrap());
        let list = banner_list(&pool).await.unwrap();
        assert!(!list
            .as_array()
            .unwrap()
            .iter()
            .any(|group| group["type"] == json!("rustcms-test")));

        match backup {
            Some(raw) => setting::save_raw(&pool, BANNER, &raw).await.unwrap(),
            None => setting::remove(&pool, BANNER).await.unwrap(),
        }
    }
}
