//! 数据库结构自动迁移（AutoMigrate）。
//!
//! 对齐 Go 版 GORM `AutoMigrate` 的语义：
//!
//! * 表不存在 → `CREATE TABLE`；
//! * 列缺失 → `ALTER TABLE ... ADD COLUMN`；
//! * 列类型不一致 → `ALTER TABLE ... MODIFY COLUMN`；
//! * 索引缺失 → `CREATE INDEX`；
//! * **不删列、不改数据**（与 GORM 一致）。
//!
//! 服务于 `src/routes.rs` 中「数据库自动迁移尚未实现」这个原本直接失败的端点。

use anyhow::{bail, Context, Result};
use sqlx::mysql::MySqlPoolOptions;
use sqlx::MySqlPool;

/// 列定义。
#[derive(Debug, Clone)]
pub struct ColumnDef {
    pub name: String,
    /// 完整 SQL 类型（如 `varchar(255)`、`int unsigned`、`datetime`）。
    pub sql_type: String,
    pub nullable: bool,
    /// 默认值（`None` 表示不写 DEFAULT 子句）。
    pub default_value: Option<String>,
}

impl ColumnDef {
    /// 新建列定义（可空、无默认值）。
    pub fn new(name: impl Into<String>, sql_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            sql_type: sql_type.into(),
            nullable: true,
            default_value: None,
        }
    }

    /// 设为 NOT NULL。
    pub fn not_null(mut self) -> Self {
        self.nullable = false;
        self
    }

    /// 设置默认值。
    pub fn with_default(mut self, value: impl Into<String>) -> Self {
        self.default_value = Some(value.into());
        self
    }

    /// `CREATE TABLE` / `ALTER TABLE` 里的列子句。
    fn clause(&self) -> String {
        let mut sql = format!("`{}` {}", self.name, self.sql_type);
        if !self.nullable {
            sql.push_str(" NOT NULL");
        }
        if let Some(default) = &self.default_value {
            sql.push_str(&format!(" DEFAULT {default}"));
        }
        sql
    }
}

/// 索引定义。
#[derive(Debug, Clone)]
pub struct IndexDef {
    pub name: String,
    pub columns: Vec<String>,
    pub unique: bool,
}

impl IndexDef {
    /// 普通索引。
    pub fn new(name: impl Into<String>, columns: Vec<String>) -> Self {
        Self {
            name: name.into(),
            columns,
            unique: false,
        }
    }

    /// 唯一索引。
    pub fn unique(name: impl Into<String>, columns: Vec<String>) -> Self {
        Self {
            name: name.into(),
            columns,
            unique: true,
        }
    }
}

/// 表定义。
#[derive(Debug, Clone)]
pub struct TableDef {
    pub name: String,
    pub columns: Vec<ColumnDef>,
    pub indexes: Vec<IndexDef>,
    /// 主键列（为空表示无主键）。
    pub primary_key: Vec<String>,
    pub engine: String,
    pub charset: String,
}

impl TableDef {
    /// 新建表定义（InnoDB + utf8mb4）。
    pub fn new(name: impl Into<String>, columns: Vec<ColumnDef>) -> Self {
        Self {
            name: name.into(),
            columns,
            indexes: Vec::new(),
            primary_key: Vec::new(),
            engine: "InnoDB".to_string(),
            charset: "utf8mb4".to_string(),
        }
    }

    /// 追加索引。
    pub fn with_index(mut self, index: IndexDef) -> Self {
        self.indexes.push(index);
        self
    }

    /// 设置主键。
    pub fn with_primary_key(mut self, columns: Vec<String>) -> Self {
        self.primary_key = columns;
        self
    }
}

/// 迁移报告。
#[derive(Debug, Default, Clone)]
pub struct MigrationReport {
    /// 新建的表。
    pub created_tables: Vec<String>,
    /// 新增的列（`表.列`）。
    pub added_columns: Vec<String>,
    /// 修改类型的列（`表.列`）。
    pub modified_columns: Vec<String>,
    /// 新建的索引（`表.索引`）。
    pub created_indexes: Vec<String>,
}

impl MigrationReport {
    /// 是否没有任何改动（幂等执行时为 true）。
    pub fn is_noop(&self) -> bool {
        self.created_tables.is_empty()
            && self.added_columns.is_empty()
            && self.modified_columns.is_empty()
            && self.created_indexes.is_empty()
    }
}

/// 归一化 SQL 类型（忽略整型显示宽度差异）。
pub fn normalize_type(raw: &str) -> String {
    let mut value = raw.trim().to_ascii_lowercase();
    for base in ["tinyint", "smallint", "mediumint", "int", "bigint"] {
        if let Some(rest) = value.strip_prefix(base) {
            let rest = rest.trim_start();
            if rest.starts_with('(')
                && rest.ends_with(')')
                && rest.len() > 2
                && rest[1..rest.len() - 1].chars().all(|c| c.is_ascii_digit())
            {
                value = base.to_string();
                break;
            }
        }
    }
    value
}

/// 生成 `CREATE TABLE` 语句。
pub fn create_table_sql(table: &TableDef) -> String {
    let mut parts: Vec<String> = table.columns.iter().map(|c| c.clause()).collect();
    if !table.primary_key.is_empty() {
        let columns = table
            .primary_key
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!("PRIMARY KEY ({columns})"));
    }
    for index in &table.indexes {
        let columns = index
            .columns
            .iter()
            .map(|name| format!("`{name}`"))
            .collect::<Vec<_>>()
            .join(", ");
        parts.push(format!(
            "{}KEY `{}` ({columns})",
            if index.unique { "UNIQUE " } else { "" },
            index.name
        ));
    }
    format!(
        "CREATE TABLE IF NOT EXISTS `{}` (\n  {}\n) ENGINE={} DEFAULT CHARSET={}",
        table.name,
        parts.join(",\n  "),
        table.engine,
        table.charset
    )
}

/// 结构迁移器。
#[derive(Debug, Clone)]
pub struct Migrator {
    pool: MySqlPool,
}

impl Migrator {
    /// 用已有连接池构造。
    pub fn new(pool: MySqlPool) -> Self {
        Self { pool }
    }

    /// 按连接串新建连接池。
    pub async fn connect(url: &str) -> Result<Self> {
        let pool = MySqlPoolOptions::new()
            .max_connections(4)
            .connect(url)
            .await
            .context("连接数据库失败")?;
        Ok(Self::new(pool))
    }

    /// 连接池引用。
    pub fn pool(&self) -> &MySqlPool {
        &self.pool
    }

    /// 迁移一批表。
    pub async fn migrate(&self, tables: &[TableDef]) -> Result<MigrationReport> {
        let mut report = MigrationReport::default();
        for table in tables {
            self.migrate_table(table, &mut report).await?;
        }
        Ok(report)
    }

    /// 迁移单表。
    pub async fn migrate_table(
        &self,
        table: &TableDef,
        report: &mut MigrationReport,
    ) -> Result<()> {
        if table.columns.is_empty() {
            bail!("表 {} 没有列定义", table.name);
        }
        if !self.table_exists(&table.name).await? {
            let sql = create_table_sql(table);
            sqlx::query(&sql)
                .execute(&self.pool)
                .await
                .with_context(|| format!("建表 {} 失败", table.name))?;
            report.created_tables.push(table.name.clone());
            return Ok(());
        }

        // 列对比：缺失则新增，类型不一致则修改。
        let existing = self.existing_columns(&table.name).await?;
        for column in &table.columns {
            match existing.get(&column.name.to_ascii_lowercase()) {
                None => {
                    let sql = format!(
                        "ALTER TABLE `{}` ADD COLUMN {}",
                        table.name,
                        column.clause()
                    );
                    sqlx::query(&sql)
                        .execute(&self.pool)
                        .await
                        .with_context(|| {
                            format!("给 {} 添加列 {} 失败", table.name, column.name)
                        })?;
                    report
                        .added_columns
                        .push(format!("{}.{}", table.name, column.name));
                }
                Some(current) => {
                    let wanted = normalize_type(&column.sql_type);
                    let actual = normalize_type(&current.column_type);
                    let nullability_changed = current.nullable != column.nullable;
                    if wanted != actual || nullability_changed {
                        let sql = format!(
                            "ALTER TABLE `{}` MODIFY COLUMN {}",
                            table.name,
                            column.clause()
                        );
                        sqlx::query(&sql)
                            .execute(&self.pool)
                            .await
                            .with_context(|| {
                                format!("修改 {}.{} 失败", table.name, column.name)
                            })?;
                        report
                            .modified_columns
                            .push(format!("{}.{}", table.name, column.name));
                    }
                }
            }
        }

        // 索引对比。
        let indexes = self.existing_indexes(&table.name).await?;
        for index in &table.indexes {
            if indexes.contains(&index.name.to_ascii_lowercase()) {
                continue;
            }
            let columns = index
                .columns
                .iter()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "CREATE {}INDEX `{}` ON `{}` ({columns})",
                if index.unique { "UNIQUE " } else { "" },
                index.name,
                table.name
            );
            sqlx::query(&sql)
                .execute(&self.pool)
                .await
                .with_context(|| {
                    format!("给 {} 创建索引 {} 失败", table.name, index.name)
                })?;
            report
                .created_indexes
                .push(format!("{}.{}", table.name, index.name));
        }
        Ok(())
    }

    /// 表是否存在。
    pub async fn table_exists(&self, table: &str) -> Result<bool> {
        let (count,): (i64,) = sqlx::query_as(
            "SELECT COUNT(*) FROM information_schema.TABLES \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?",
        )
        .bind(table)
        .fetch_one(&self.pool)
        .await
        .context("查询表是否存在失败")?;
        Ok(count > 0)
    }

    /// 读取现有列信息。
    async fn existing_columns(&self, table: &str) -> Result<std::collections::HashMap<String, ExistingColumn>> {
        use sqlx::Row;
        let rows = sqlx::query(
            "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE, COLUMN_DEFAULT \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?",
        )
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .context("读取列信息失败")?;
        let mut map = std::collections::HashMap::new();
        for row in rows {
            let name: String = row.try_get("COLUMN_NAME")?;
            let column_type: String = row.try_get("COLUMN_TYPE")?;
            let nullable_text: String = row.try_get("IS_NULLABLE")?;
            let default_value: Option<String> = row.try_get("COLUMN_DEFAULT").ok();
            map.insert(
                name.to_ascii_lowercase(),
                ExistingColumn {
                    column_type,
                    nullable: nullable_text.eq_ignore_ascii_case("YES"),
                    default_value,
                },
            );
        }
        Ok(map)
    }

    /// 读取现有索引名集合。
    async fn existing_indexes(&self, table: &str) -> Result<std::collections::HashSet<String>> {
        use sqlx::Row;
        let rows = sqlx::query(
            "SELECT DISTINCT INDEX_NAME FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ?",
        )
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .context("读取索引信息失败")?;
        let mut set = std::collections::HashSet::new();
        for row in rows {
            let name: String = row.try_get("INDEX_NAME")?;
            set.insert(name.to_ascii_lowercase());
        }
        Ok(set)
    }
}

/// 现有列信息。
#[derive(Debug, Clone)]
struct ExistingColumn {
    column_type: String,
    nullable: bool,
    #[allow(dead_code)]
    default_value: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_type_ignores_integer_display_width() {
        assert_eq!(normalize_type("int(11)"), "int");
        assert_eq!(normalize_type("BIGINT(20)"), "bigint");
        assert_eq!(normalize_type("varchar(255)"), "varchar(255)");
        assert_eq!(normalize_type("int unsigned"), "int unsigned");
        assert_eq!(normalize_type("  datetime  "), "datetime");
    }

    #[test]
    fn create_table_sql_contains_columns_and_indexes() {
        let table = TableDef::new(
            "demo",
            vec![
                ColumnDef::new("id", "bigint unsigned").not_null(),
                ColumnDef::new("name", "varchar(64)"),
                ColumnDef::new("hits", "int unsigned").with_default("0"),
            ],
        )
        .with_primary_key(vec!["id".to_string()])
        .with_index(IndexDef::unique("uk_name", vec!["name".to_string()]));
        let sql = create_table_sql(&table);
        assert!(sql.contains("CREATE TABLE IF NOT EXISTS `demo`"), "{sql}");
        assert!(sql.contains("`id` bigint unsigned NOT NULL"), "{sql}");
        assert!(sql.contains("`hits` int unsigned DEFAULT 0"), "{sql}");
        assert!(sql.contains("PRIMARY KEY (`id`)"), "{sql}");
        assert!(sql.contains("UNIQUE KEY `uk_name` (`name`)"), "{sql}");
        assert!(sql.contains("ENGINE=InnoDB"), "{sql}");
        assert!(sql.contains("CHARSET=utf8mb4"), "{sql}");
    }

    /// 取测试连接池；没有配置数据库时返回 `None`（跳过用例）。
    async fn test_pool() -> Option<MySqlPool> {
        let url = std::env::var("RUSTCMS_TEST_DATABASE_URL").ok()?;
        let pool = MySqlPoolOptions::new()
            .max_connections(2)
            .acquire_timeout(std::time::Duration::from_secs(5))
            .connect(&url)
            .await
            .ok()?;
        Some(pool)
    }

    fn temp_table_name(tag: &str) -> String {
        format!("rustcms_ext_{tag}_{}", std::process::id())
    }

    #[tokio::test]
    async fn migrate_creates_table_and_is_idempotent() {
        let Some(pool) = test_pool().await else {
            eprintln!("跳过：未配置 RUSTCMS_TEST_DATABASE_URL");
            return;
        };
        let migrator = Migrator::new(pool.clone());
        let table_name = temp_table_name("create");
        let table = TableDef::new(
            &table_name,
            vec![
                ColumnDef::new("id", "bigint unsigned").not_null(),
                ColumnDef::new("title", "varchar(120)"),
            ],
        )
        .with_primary_key(vec!["id".to_string()]);

        let mut report = MigrationReport::default();
        migrator.migrate_table(&table, &mut report).await.expect("migrate");
        assert_eq!(report.created_tables, vec![table_name.clone()]);
        assert!(migrator.table_exists(&table_name).await.unwrap());

        // 再跑一次应当没有任何改动。
        let mut second = MigrationReport::default();
        migrator.migrate_table(&table, &mut second).await.expect("migrate");
        assert!(second.is_noop(), "{second:?}");

        sqlx::query(&format!("DROP TABLE IF EXISTS `{table_name}`"))
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn migrate_adds_missing_column() {
        let Some(pool) = test_pool().await else {
            eprintln!("跳过：未配置 RUSTCMS_TEST_DATABASE_URL");
            return;
        };
        let migrator = Migrator::new(pool.clone());
        let table_name = temp_table_name("addcol");
        let base = TableDef::new(
            &table_name,
            vec![ColumnDef::new("id", "bigint unsigned").not_null()],
        )
        .with_primary_key(vec!["id".to_string()]);
        let mut report = MigrationReport::default();
        migrator.migrate_table(&base, &mut report).await.expect("migrate");

        let extended = TableDef::new(
            &table_name,
            vec![
                ColumnDef::new("id", "bigint unsigned").not_null(),
                ColumnDef::new("body", "text"),
            ],
        )
        .with_primary_key(vec!["id".to_string()]);
        let mut report2 = MigrationReport::default();
        migrator
            .migrate_table(&extended, &mut report2)
            .await
            .expect("migrate");
        assert_eq!(
            report2.added_columns,
            vec![format!("{table_name}.body")],
            "{report2:?}"
        );

        sqlx::query(&format!("DROP TABLE IF EXISTS `{table_name}`"))
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn migrate_modifies_changed_column_type() {
        let Some(pool) = test_pool().await else {
            eprintln!("跳过：未配置 RUSTCMS_TEST_DATABASE_URL");
            return;
        };
        let migrator = Migrator::new(pool.clone());
        let table_name = temp_table_name("modcol");
        let base = TableDef::new(
            &table_name,
            vec![
                ColumnDef::new("id", "bigint unsigned").not_null(),
                ColumnDef::new("title", "varchar(32)"),
            ],
        )
        .with_primary_key(vec!["id".to_string()]);
        let mut report = MigrationReport::default();
        migrator.migrate_table(&base, &mut report).await.expect("migrate");

        let changed = TableDef::new(
            &table_name,
            vec![
                ColumnDef::new("id", "bigint unsigned").not_null(),
                ColumnDef::new("title", "varchar(255)"),
            ],
        )
        .with_primary_key(vec!["id".to_string()]);
        let mut report2 = MigrationReport::default();
        migrator
            .migrate_table(&changed, &mut report2)
            .await
            .expect("migrate");
        assert_eq!(
            report2.modified_columns,
            vec![format!("{table_name}.title")],
            "{report2:?}"
        );

        sqlx::query(&format!("DROP TABLE IF EXISTS `{table_name}`"))
            .execute(&pool)
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn migrate_creates_missing_index() {
        let Some(pool) = test_pool().await else {
            eprintln!("跳过：未配置 RUSTCMS_TEST_DATABASE_URL");
            return;
        };
        let migrator = Migrator::new(pool.clone());
        let table_name = temp_table_name("idx");
        let base = TableDef::new(
            &table_name,
            vec![
                ColumnDef::new("id", "bigint unsigned").not_null(),
                ColumnDef::new("slug", "varchar(64)"),
            ],
        )
        .with_primary_key(vec!["id".to_string()]);
        let mut report = MigrationReport::default();
        migrator.migrate_table(&base, &mut report).await.expect("migrate");

        let with_index = base.with_index(IndexDef::unique("uk_slug", vec!["slug".to_string()]));
        let mut report2 = MigrationReport::default();
        migrator
            .migrate_table(&with_index, &mut report2)
            .await
            .expect("migrate");
        assert_eq!(
            report2.created_indexes,
            vec![format!("{table_name}.uk_slug")],
            "{report2:?}"
        );

        sqlx::query(&format!("DROP TABLE IF EXISTS `{table_name}`"))
            .execute(&pool)
            .await
            .unwrap();
    }
}
