//! 导航与导航分组（表 `navs` / `nav_types`）。
//!
//! 对齐 Go 版 `provider` 的 `GetNavList` / `SaveNav` / `SaveNavType`。

use serde::{Deserialize, Serialize};
use sqlx::mysql::MySqlPool;
use sqlx::FromRow;

use super::ModelError;

/// 导航项。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Nav {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    pub sub_title: String,
    pub description: String,
    pub parent_id: u32,
    pub nav_type: u32,
    pub page_id: i64,
    pub type_id: u32,
    pub link: String,
    pub logo: String,
    pub style: String,
    pub sort: u32,
    pub status: u8,
}

/// 导航分组。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct NavType {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
}

/// 带下级的导航节点（下级放在 `nav_list`，与 Go 的 json tag 一致）。
#[derive(Debug, Clone, Serialize)]
pub struct NavNode {
    #[serde(flatten)]
    pub nav: Nav,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub nav_list: Vec<NavNode>,
}

/// 导航保存入参（`request.NavConfig`）。
#[derive(Debug, Default, Clone, Deserialize)]
pub struct NavInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub sub_title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub parent_id: Option<u32>,
    #[serde(default)]
    pub nav_type: Option<u32>,
    #[serde(default)]
    pub page_id: Option<i64>,
    #[serde(default)]
    pub type_id: Option<u32>,
    #[serde(default)]
    pub link: Option<String>,
    #[serde(default)]
    pub sort: Option<u32>,
    #[serde(default)]
    pub style: Option<String>,
    #[serde(default)]
    pub status: Option<u8>,
    #[serde(default)]
    pub logo: Option<String>,
    #[serde(default)]
    pub update_all: bool,
    #[serde(default)]
    pub partial: bool,
}

/// 导航分组保存入参（`request.NavTypeRequest`）。
#[derive(Debug, Default, Clone, Deserialize)]
pub struct NavTypeInput {
    #[serde(default)]
    pub id: u32,
    #[serde(default)]
    pub title: Option<String>,
}

fn apply_str(target: &mut String, input: Option<&str>, update_all: bool) {
    if let Some(value) = input {
        if update_all || !value.is_empty() {
            *target = value.to_string();
        }
    }
}

fn apply_num<T: Copy + Default + PartialOrd>(target: &mut T, input: Option<T>, update_all: bool) {
    if let Some(value) = input {
        if update_all || value > T::default() {
            *target = value;
        }
    }
}

/// 某分组下的全部导航（按 `sort ASC, id ASC`）。
pub async fn list_by_type(pool: &MySqlPool, type_id: u32) -> anyhow::Result<Vec<Nav>> {
    let list = sqlx::query_as::<_, Nav>(
        "SELECT * FROM navs WHERE type_id = ? ORDER BY sort ASC, id ASC",
    )
    .bind(type_id)
    .fetch_all(pool)
    .await?;
    Ok(list)
}

/// 组装成树（根是 `parent_id = 0`）。
pub fn tree(list: &[Nav]) -> Vec<NavNode> {
    fn build(parent: u32, all: &[Nav]) -> Vec<NavNode> {
        all.iter()
            .filter(|item| item.parent_id == parent)
            .map(|item| NavNode {
                nav: item.clone(),
                nav_list: build(item.id, all),
            })
            .collect()
    }
    build(0, list)
}

/// 按 ID 取导航。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Nav>> {
    Ok(sqlx::query_as::<_, Nav>("SELECT * FROM navs WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?)
}

/// 新建 / 更新导航。
pub async fn save(pool: &MySqlPool, input: &NavInput) -> Result<Nav, ModelError> {
    let update_all = input.update_all || !input.partial;
    let existing = if input.id > 0 {
        get(pool, input.id).await?
    } else {
        None
    };
    let is_new = existing.is_none();

    let mut nav = existing.unwrap_or(Nav {
        id: input.id,
        created_time: None,
        updated_time: None,
        title: String::new(),
        sub_title: String::new(),
        description: String::new(),
        parent_id: 0,
        nav_type: 0,
        page_id: 0,
        type_id: 1,
        link: String::new(),
        logo: String::new(),
        style: String::new(),
        sort: 99,
        status: 1,
    });

    apply_str(&mut nav.title, input.title.as_deref(), update_all);
    apply_str(&mut nav.sub_title, input.sub_title.as_deref(), update_all);
    apply_str(&mut nav.description, input.description.as_deref(), update_all);
    apply_str(&mut nav.link, input.link.as_deref(), update_all);
    apply_str(&mut nav.logo, input.logo.as_deref(), update_all);
    apply_str(&mut nav.style, input.style.as_deref(), update_all);
    apply_num(&mut nav.parent_id, input.parent_id, update_all);
    apply_num(&mut nav.nav_type, input.nav_type, update_all);
    apply_num(&mut nav.type_id, input.type_id, update_all);
    apply_num(&mut nav.sort, input.sort, update_all);
    apply_num(&mut nav.status, input.status, update_all);
    if let Some(page_id) = input.page_id {
        if update_all || page_id != 0 {
            nav.page_id = page_id;
        }
    }

    let now = chrono::Utc::now().timestamp();
    if is_new {
        nav.created_time = Some(now);
        nav.updated_time = Some(now);
        let result = if nav.id > 0 {
            sqlx::query(
                "INSERT INTO navs (id, created_time, updated_time, title, sub_title, description, \
                    parent_id, nav_type, page_id, type_id, link, logo, style, sort, status) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(nav.id)
            .bind(nav.created_time)
            .bind(nav.updated_time)
            .bind(&nav.title)
            .bind(&nav.sub_title)
            .bind(&nav.description)
            .bind(nav.parent_id)
            .bind(nav.nav_type)
            .bind(nav.page_id)
            .bind(nav.type_id)
            .bind(&nav.link)
            .bind(&nav.logo)
            .bind(&nav.style)
            .bind(nav.sort)
            .bind(nav.status)
            .execute(pool)
            .await?
        } else {
            sqlx::query(
                "INSERT INTO navs (created_time, updated_time, title, sub_title, description, \
                    parent_id, nav_type, page_id, type_id, link, logo, style, sort, status) \
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
            )
            .bind(nav.created_time)
            .bind(nav.updated_time)
            .bind(&nav.title)
            .bind(&nav.sub_title)
            .bind(&nav.description)
            .bind(nav.parent_id)
            .bind(nav.nav_type)
            .bind(nav.page_id)
            .bind(nav.type_id)
            .bind(&nav.link)
            .bind(&nav.logo)
            .bind(&nav.style)
            .bind(nav.sort)
            .bind(nav.status)
            .execute(pool)
            .await?
        };
        nav.id = result.last_insert_id() as u32;
    } else {
        nav.updated_time = Some(now);
        sqlx::query(
            "UPDATE navs SET updated_time = ?, title = ?, sub_title = ?, description = ?, \
                parent_id = ?, nav_type = ?, page_id = ?, type_id = ?, link = ?, logo = ?, \
                style = ?, sort = ?, status = ? WHERE id = ?",
        )
        .bind(nav.updated_time)
        .bind(&nav.title)
        .bind(&nav.sub_title)
        .bind(&nav.description)
        .bind(nav.parent_id)
        .bind(nav.nav_type)
        .bind(nav.page_id)
        .bind(nav.type_id)
        .bind(&nav.link)
        .bind(&nav.logo)
        .bind(&nav.style)
        .bind(nav.sort)
        .bind(nav.status)
        .bind(nav.id)
        .execute(pool)
        .await?;
    }
    Ok(nav)
}

/// 删除导航，返回是否真的删掉。
pub async fn delete(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM navs WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// 全部分组（按 ID 升序）。
pub async fn list_types(pool: &MySqlPool) -> anyhow::Result<Vec<NavType>> {
    Ok(
        sqlx::query_as::<_, NavType>("SELECT * FROM nav_types ORDER BY id ASC")
            .fetch_all(pool)
            .await?,
    )
}

/// 按 ID 取分组。
pub async fn get_type(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<NavType>> {
    Ok(
        sqlx::query_as::<_, NavType>("SELECT * FROM nav_types WHERE id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?,
    )
}

/// 新建 / 更新分组。
pub async fn save_type(pool: &MySqlPool, input: &NavTypeInput) -> Result<NavType, ModelError> {
    let now = chrono::Utc::now().timestamp();
    if input.id > 0 && get_type(pool, input.id).await?.is_some() {
        let title = input.title.clone().unwrap_or_default();
        sqlx::query("UPDATE nav_types SET title = ?, updated_time = ? WHERE id = ?")
            .bind(&title)
            .bind(now)
            .bind(input.id)
            .execute(pool)
            .await?;
        return Ok(get_type(pool, input.id).await?.expect("刚更新过"));
    }

    let title = input.title.clone().unwrap_or_default();
    let result = if input.id > 0 {
        sqlx::query(
            "INSERT INTO nav_types (id, created_time, updated_time, title) VALUES (?, ?, ?, ?)",
        )
        .bind(input.id)
        .bind(now)
        .bind(now)
        .bind(&title)
        .execute(pool)
        .await?
    } else {
        sqlx::query("INSERT INTO nav_types (created_time, updated_time, title) VALUES (?, ?, ?)")
            .bind(now)
            .bind(now)
            .bind(&title)
            .execute(pool)
            .await?
    };
    let id = result.last_insert_id() as u32;
    Ok(get_type(pool, id).await?.expect("刚插入过"))
}

/// 删除分组（1 号默认分组不允许删），连带清掉组内导航。
pub async fn delete_type(pool: &MySqlPool, id: u32) -> Result<bool, ModelError> {
    if id == 1 {
        return Err(ModelError::invalid("不允许删除默认分类"));
    }
    if get_type(pool, id).await?.is_none() {
        return Ok(false);
    }
    sqlx::query("DELETE FROM navs WHERE type_id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    sqlx::query("DELETE FROM nav_types WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{db_lock, test_pool};

    #[test]
    fn builds_nav_tree() {
        let nav = |id: u32, parent: u32| Nav {
            id,
            created_time: None,
            updated_time: None,
            title: format!("导航{id}"),
            sub_title: String::new(),
            description: String::new(),
            parent_id: parent,
            nav_type: 0,
            page_id: 0,
            type_id: 1,
            link: String::new(),
            logo: String::new(),
            style: String::new(),
            sort: 99,
            status: 1,
        };
        let list = vec![nav(1, 0), nav(2, 1), nav(3, 1), nav(4, 2)];
        let tree = tree(&list);
        assert_eq!(tree.len(), 1);
        assert_eq!(tree[0].nav_list.len(), 2);
        assert_eq!(tree[0].nav_list[0].nav_list[0].nav.id, 4);
    }

    #[tokio::test]
    async fn reads_real_navs() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };
        let types = list_types(&pool).await.unwrap();
        assert!(!types.is_empty());
        let list = list_by_type(&pool, types[0].id).await.unwrap();
        assert!(list.iter().all(|item| item.type_id == types[0].id));
        if let Some(first) = list.first() {
            let found = get(&pool, first.id).await.unwrap().expect("导航应存在");
            assert_eq!(found.title, first.title);
        }
        assert!(get_type(&pool, types[0].id).await.unwrap().is_some());
    }
}
