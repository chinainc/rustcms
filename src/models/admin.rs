//! 后台管理员（`admins` 表）。

use serde::ser::SerializeStruct;
use serde::{Deserialize, Serialize, Serializer};
use sqlx::mysql::{MySql, MySqlPool};
use sqlx::{FromRow, QueryBuilder};

/// 管理员查询用的列（`password` 要查出来给改密校验，但序列化时会被跳过）。
const ADMIN_COLUMNS: &str = "SELECT id, created_time, updated_time, user_name, password, status, \
                             login_time, group_id FROM admins WHERE 1 = 1";

/// 管理员账号。密码哈希不参与序列化。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct Admin {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub user_name: String,
    #[serde(skip_serializing)]
    pub password: String,
    pub status: u8,
    pub login_time: Option<i64>,
    pub group_id: u32,
}

/// 按用户名查管理员。
pub async fn get_by_name(pool: &MySqlPool, user_name: &str) -> anyhow::Result<Option<Admin>> {
    let row = sqlx::query_as::<_, Admin>(
        "SELECT id, created_time, updated_time, user_name, password, status, login_time, group_id \
         FROM admins WHERE user_name = ? LIMIT 1",
    )
    .bind(user_name)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// 按 ID 查管理员。
pub async fn get(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<Admin>> {
    let row = sqlx::query_as::<_, Admin>(
        "SELECT id, created_time, updated_time, user_name, password, status, login_time, group_id \
         FROM admins WHERE id = ?",
    )
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row)
}

/// 记录登录时间。
pub async fn touch_login(pool: &MySqlPool, id: u32) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query("UPDATE admins SET login_time = ?, updated_time = ? WHERE id = ?")
        .bind(now)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// 更新管理员资料（用户名 / 状态 / 用户组）。
///
/// `password` 为 `None` 时保留原密码，与 Go 版 `AdminDetailForm` 的
/// 「`Password` 为空就不改密」一致。
pub async fn update_profile(
    pool: &MySqlPool,
    id: u32,
    user_name: &str,
    status: u8,
    group_id: u32,
    password: Option<&str>,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    sqlx::query(
        "UPDATE admins SET user_name = ?, status = ?, group_id = ?, updated_time = ? WHERE id = ?",
    )
    .bind(user_name)
    .bind(status)
    .bind(group_id)
    .bind(now)
    .bind(id)
    .execute(pool)
    .await?;

    if let Some(hash) = password {
        sqlx::query("UPDATE admins SET password = ?, updated_time = ? WHERE id = ?")
            .bind(hash)
            .bind(now)
            .bind(id)
            .execute(pool)
            .await?;
    }
    Ok(())
}

/// 新建管理员，返回自增 ID。
pub async fn create(
    pool: &MySqlPool,
    user_name: &str,
    password_hash: &str,
    status: u8,
    group_id: u32,
) -> anyhow::Result<u32> {
    let now = chrono::Utc::now().timestamp();
    let result = sqlx::query(
        "INSERT INTO admins (created_time, updated_time, user_name, password, status, login_time, \
            group_id) \
         VALUES (?, ?, ?, ?, ?, 0, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(user_name)
    .bind(password_hash)
    .bind(status)
    .bind(group_id)
    .execute(pool)
    .await?;
    Ok(result.last_insert_id() as u32)
}

/// 写一条后台登录日志（`admin_login_logs`，对应 Go 版 `model.AdminLoginLog`）。
///
/// `admin_id` 为 0 表示用户名不存在；`status` 1=成功、0=失败。
///
/// 与 Go 版的有意差异：失败日志**不落库明文密码**（Go 会把用户输入原样写进
/// `password` 列），这里一律写空串。
pub async fn record_login(
    pool: &MySqlPool,
    admin_id: u32,
    ip: &str,
    status: u8,
    user_name: &str,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    // ip 列是 varchar(32)，超长会被严格模式拒绝
    let ip: String = ip.chars().take(32).collect();
    let user_name: String = user_name.chars().take(32).collect();
    sqlx::query(
        "INSERT INTO admin_login_logs \
             (created_time, updated_time, admin_id, ip, status, user_name, password) \
         VALUES (?, ?, ?, ?, ?, ?, '')",
    )
    .bind(now)
    .bind(now)
    .bind(admin_id)
    .bind(ip)
    .bind(status)
    .bind(user_name)
    .execute(pool)
    .await?;
    Ok(())
}

/// 管理员列表过滤条件（对应 Go 版 `AdminList` 的 id / group_id / user_name 三个参数）。
#[derive(Debug, Default, Clone)]
pub struct AdminFilter {
    /// 精确匹配 ID。
    pub id: Option<u32>,
    /// 精确匹配用户组。
    pub group_id: Option<u32>,
    /// 用户名模糊匹配（`LIKE %x%`，空串视为不过滤）。
    pub user_name: Option<String>,
}

fn push_admin_filter(builder: &mut QueryBuilder<'_, MySql>, filter: &AdminFilter) {
    if let Some(id) = filter.id {
        builder.push(" AND id = ").push_bind(id);
    }
    if let Some(group_id) = filter.group_id {
        builder.push(" AND group_id = ").push_bind(group_id);
    }
    if let Some(name) = filter
        .user_name
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        builder
            .push(" AND user_name LIKE ")
            .push_bind(format!("%{name}%"));
    }
}

/// 分页查管理员（按 `id DESC`），返回 `(总数, 本页记录)`。
pub async fn list_page(
    pool: &MySqlPool,
    filter: &AdminFilter,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<Admin>)> {
    let page = page.max(1);
    // 未传（≤0）按 20 条，上限 100 条
    let page_size = if page_size < 1 {
        20
    } else {
        page_size.min(100)
    };
    let offset = (page - 1) * page_size;

    let mut counter = QueryBuilder::<MySql>::new("SELECT COUNT(*) FROM admins WHERE 1 = 1");
    push_admin_filter(&mut counter, filter);
    let total: i64 = counter.build_query_scalar().fetch_one(pool).await?;

    let mut query = QueryBuilder::<MySql>::new(ADMIN_COLUMNS);
    push_admin_filter(&mut query, filter);
    query
        .push(" ORDER BY id DESC LIMIT ")
        .push_bind(page_size)
        .push(" OFFSET ")
        .push_bind(offset);
    let rows = query.build_query_as::<Admin>().fetch_all(pool).await?;

    Ok((total, rows))
}

/// 删除管理员，返回是否真的删掉了一行。
///
/// 登录 / 操作日志刻意保留：审计信息不该因为账号被删就消失（Go 版也只删 `admins` 行）。
pub async fn delete(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM admins WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

/// 后台登录日志（`admin_login_logs`）。
///
/// 有意不返回 `password` 列：Go 版会把登录时提交的明文密码原样存进日志，
/// 这里历来写空串，就没必要再往回吐。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct AdminLoginLog {
    pub id: u32,
    pub created_time: Option<i64>,
    pub admin_id: u32,
    pub ip: String,
    pub status: u8,
    pub user_name: String,
}

/// 分页读取登录日志（按下发时间倒序），返回 `(总数, 本页记录)`。
///
/// `admin_id` 给 `None` 表示不过滤（超管视角）。
pub async fn list_login_logs(
    pool: &MySqlPool,
    admin_id: Option<u32>,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<AdminLoginLog>)> {
    let page = page.max(1);
    // 未传（≤0）按 20 条，上限 100 条
    let page_size = if page_size < 1 {
        20
    } else {
        page_size.min(100)
    };
    let offset = (page - 1) * page_size;

    let total: i64 = match admin_id {
        Some(id) => {
            sqlx::query_scalar("SELECT COUNT(*) FROM admin_login_logs WHERE admin_id = ?")
                .bind(id)
                .fetch_one(pool)
                .await?
        }
        None => {
            sqlx::query_scalar("SELECT COUNT(*) FROM admin_login_logs")
                .fetch_one(pool)
                .await?
        }
    };

    let rows =
        match admin_id {
            Some(id) => sqlx::query_as::<_, AdminLoginLog>(
                "SELECT id, created_time, admin_id, ip, status, user_name FROM admin_login_logs \
                 WHERE admin_id = ? ORDER BY id DESC LIMIT ? OFFSET ?",
            )
            .bind(id)
            .bind(page_size)
            .bind(offset)
            .fetch_all(pool)
            .await?,
            None => sqlx::query_as::<_, AdminLoginLog>(
                "SELECT id, created_time, admin_id, ip, status, user_name FROM admin_login_logs \
                 ORDER BY id DESC LIMIT ? OFFSET ?",
            )
            .bind(page_size)
            .bind(offset)
            .fetch_all(pool)
            .await?,
        };

    Ok((total, rows))
}

/// 管理员用户组查询用的列。
const GROUP_COLUMNS: &str = "SELECT id, created_time, updated_time, title, description, status, \
                             setting FROM admin_groups";

/// 管理员用户组（`admin_groups` 表，对应 Go 版 `model.AdminGroup`）。
#[derive(Debug, Clone, FromRow)]
pub struct AdminGroup {
    pub id: u32,
    pub created_time: Option<i64>,
    pub updated_time: Option<i64>,
    pub title: String,
    pub description: String,
    pub status: i8,
    /// `setting` 是 TEXT 列，存 `{"permissions":[...]}`。
    pub setting: Option<String>,
}

/// 用户组的权限配置（Go 版 `model.GroupSetting`）。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct GroupSetting {
    #[serde(default)]
    pub permissions: Vec<String>,
}

impl AdminGroup {
    /// 把 `setting` 列解析成结构体；空值 / 坏 JSON 一律当「没有任何权限」。
    pub fn setting_value(&self) -> GroupSetting {
        self.setting
            .as_deref()
            .map(str::trim)
            .filter(|raw| !raw.is_empty())
            .and_then(|raw| serde_json::from_str::<GroupSetting>(raw).ok())
            .unwrap_or_default()
    }
}

/// 序列化成 API 形状：`setting` 列输出成对象（与 Go 版 `json:"setting"` 一致）。
impl Serialize for AdminGroup {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut out = serializer.serialize_struct("AdminGroup", 7)?;
        out.serialize_field("id", &self.id)?;
        out.serialize_field("created_time", &self.created_time)?;
        out.serialize_field("updated_time", &self.updated_time)?;
        out.serialize_field("title", &self.title)?;
        out.serialize_field("description", &self.description)?;
        out.serialize_field("status", &self.status)?;
        out.serialize_field("setting", &self.setting_value())?;
        out.end()
    }
}

/// 全部用户组（按 ID 升序，与 Go 版 `GetAdminGroups` 的主键顺序一致）。
pub async fn list_groups(pool: &MySqlPool) -> anyhow::Result<Vec<AdminGroup>> {
    let rows = sqlx::query_as::<_, AdminGroup>(&format!("{GROUP_COLUMNS} ORDER BY id ASC"))
        .fetch_all(pool)
        .await?;
    Ok(rows)
}

/// 按 ID 查用户组。
pub async fn get_group(pool: &MySqlPool, id: u32) -> anyhow::Result<Option<AdminGroup>> {
    let row = sqlx::query_as::<_, AdminGroup>(&format!("{GROUP_COLUMNS} WHERE id = ? LIMIT 1"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row)
}

/// 新增 / 更新用户组，返回组 ID。
///
/// 与 Go 版 `SaveAdminGroupInfo` 一样，状态恒写 1（启用）。
pub async fn save_group(
    pool: &MySqlPool,
    id: u32,
    title: &str,
    description: &str,
    setting: &GroupSetting,
) -> anyhow::Result<u32> {
    let now = chrono::Utc::now().timestamp();
    let setting = serde_json::to_string(setting)?;

    if id > 0 {
        sqlx::query(
            "UPDATE admin_groups SET title = ?, description = ?, status = 1, setting = ?, \
                updated_time = ? WHERE id = ?",
        )
        .bind(title)
        .bind(description)
        .bind(setting)
        .bind(now)
        .bind(id)
        .execute(pool)
        .await?;
        return Ok(id);
    }

    let result = sqlx::query(
        "INSERT INTO admin_groups (created_time, updated_time, title, description, status, setting) \
         VALUES (?, ?, ?, ?, 1, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(title)
    .bind(description)
    .bind(setting)
    .execute(pool)
    .await?;
    Ok(result.last_insert_id() as u32)
}

/// 组下还有多少管理员（删组前检查用）。
pub async fn count_in_group(pool: &MySqlPool, group_id: u32) -> anyhow::Result<i64> {
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM admins WHERE group_id = ?")
        .bind(group_id)
        .fetch_one(pool)
        .await?;
    Ok(total)
}

/// 后台操作日志（`admin_logs` 表，对应 Go 版 `model.AdminLog`）。
#[derive(Debug, Clone, FromRow, Serialize)]
pub struct AdminLog {
    pub id: u32,
    pub created_time: Option<i64>,
    pub admin_id: u32,
    pub ip: String,
    pub log: String,
    pub user_name: String,
}

/// 写一条操作日志（Go 版 `AddAdminLog`：日志超过 250 字符会被截断）。
pub async fn record_action(
    pool: &MySqlPool,
    admin_id: u32,
    user_name: &str,
    ip: &str,
    log: &str,
) -> anyhow::Result<()> {
    let now = chrono::Utc::now().timestamp();
    // `log` 列是 varchar(250)、`ip`/`user_name` 是 varchar(32)，严格模式下超长会整条失败
    let log: String = log.chars().take(250).collect();
    let ip: String = ip.chars().take(32).collect();
    let user_name: String = user_name.chars().take(32).collect();

    sqlx::query(
        "INSERT INTO admin_logs (created_time, updated_time, admin_id, ip, `log`, user_name) \
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(now)
    .bind(now)
    .bind(admin_id)
    .bind(ip)
    .bind(log)
    .bind(user_name)
    .execute(pool)
    .await?;
    Ok(())
}

/// 分页读取操作日志（倒序），返回 `(总数, 本页记录)`。
pub async fn list_actions(
    pool: &MySqlPool,
    admin_id: Option<u32>,
    page: i64,
    page_size: i64,
) -> anyhow::Result<(i64, Vec<AdminLog>)> {
    let page = page.max(1);
    let page_size = if page_size < 1 {
        20
    } else {
        page_size.min(100)
    };
    let offset = (page - 1) * page_size;

    let total: i64 = match admin_id {
        Some(id) => {
            sqlx::query_scalar("SELECT COUNT(*) FROM admin_logs WHERE admin_id = ?")
                .bind(id)
                .fetch_one(pool)
                .await?
        }
        None => {
            sqlx::query_scalar("SELECT COUNT(*) FROM admin_logs")
                .fetch_one(pool)
                .await?
        }
    };

    let rows = match admin_id {
        Some(id) => {
            sqlx::query_as::<_, AdminLog>(
                "SELECT id, created_time, admin_id, ip, `log`, user_name FROM admin_logs \
                 WHERE admin_id = ? ORDER BY id DESC LIMIT ? OFFSET ?",
            )
            .bind(id)
            .bind(page_size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
        None => {
            sqlx::query_as::<_, AdminLog>(
                "SELECT id, created_time, admin_id, ip, `log`, user_name FROM admin_logs \
                 ORDER BY id DESC LIMIT ? OFFSET ?",
            )
            .bind(page_size)
            .bind(offset)
            .fetch_all(pool)
            .await?
        }
    };

    Ok((total, rows))
}

/// 删除用户组，返回是否真的删掉了一行。
pub async fn delete_group(pool: &MySqlPool, id: u32) -> anyhow::Result<bool> {
    let result = sqlx::query("DELETE FROM admin_groups WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::db_lock;
    use crate::models::test_pool;

    #[tokio::test]
    async fn reads_real_admins() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        let id: Option<u32> = sqlx::query_scalar("SELECT id FROM admins ORDER BY id LIMIT 1")
            .fetch_optional(&pool)
            .await
            .unwrap();
        let Some(id) = id else {
            return;
        };

        let admin = get(&pool, id).await.unwrap().expect("管理员应存在");
        assert_eq!(admin.id, id);
        // bcrypt 哈希形如 `$2a$12$...`
        assert!(admin.password.starts_with("$2"), "{}", admin.password);

        let by_name = get_by_name(&pool, &admin.user_name).await.unwrap();
        assert_eq!(by_name.map(|item| item.id), Some(id));

        assert!(get(&pool, u32::MAX).await.unwrap().is_none());
        assert!(get_by_name(&pool, "不存在的管理员")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn reads_real_admin_groups() {
        let _guard = db_lock().await;
        let Some(pool) = test_pool().await else {
            return;
        };

        let groups = list_groups(&pool).await.unwrap();
        let Some(first) = groups.first() else {
            return;
        };
        assert!(!first.title.is_empty());
        // ID 升序
        assert!(groups.windows(2).all(|pair| pair[0].id < pair[1].id));

        // `setting` 列解析成对象（不是原样吐 JSON 字符串）
        let value = serde_json::to_value(first).unwrap();
        assert!(value["setting"].is_object(), "{value}");
        assert!(
            value["setting"]["permissions"].is_array(),
            "权限列表应是数组：{value}"
        );

        let found = get_group(&pool, first.id).await.unwrap();
        assert_eq!(found.map(|item| item.id), Some(first.id));
        assert!(get_group(&pool, u32::MAX).await.unwrap().is_none());
    }
}
