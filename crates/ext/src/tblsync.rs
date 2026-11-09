//! 数据库结构自动迁移。
//!
//! 迁移语义：
//!
//! * 表不存在 → `CREATE TABLE`；
//! * 列缺失 → `ALTER TABLE ... ADD COLUMN`；
//! * 列类型不一致 → `ALTER TABLE ... MODIFY COLUMN`；
//! * 索引缺失 → `CREATE INDEX`；
//! * **不删列、不改数据**（只做增量变更）。
//!
//! 服务于 `src/routes.rs` 的数据库自动迁移端点，以及 `models::setting::migrate_db` /
//! `models::module::migrate` 的「只补缺失列」需求（`Migrator::ensure_columns`）。

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
    /// 是否自增（只有整型主键用得上）。
    pub auto_increment: bool,
    /// `ADD COLUMN` 时插到哪一列之后（`None` 表示 MySQL 默认追加到末尾）。
    pub after: Option<String>,
}

impl ColumnDef {
    /// 新建列定义（可空、无默认值）。
    pub fn new(name: impl Into<String>, sql_type: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            sql_type: sql_type.into(),
            nullable: true,
            default_value: None,
            auto_increment: false,
            after: None,
        }
    }

    /// 用逻辑类型构造（见 [`SqlType`]）。
    pub fn typed(name: impl Into<String>, sql_type: SqlType) -> Self {
        Self::new(name, sql_type.to_sql())
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

    /// 设为自增列（`AUTO_INCREMENT`）。
    pub fn auto_increment(mut self) -> Self {
        self.auto_increment = true;
        self
    }

    /// 指定补列位置（`ALTER TABLE ... ADD COLUMN ... AFTER x`）。
    pub fn after(mut self, column: impl Into<String>) -> Self {
        self.after = Some(column.into());
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
        if self.auto_increment {
            sql.push_str(" AUTO_INCREMENT");
        }
        sql
    }
}

/// 逻辑类型到 MySQL 类型的映射（写迁移定义时用，省得手拼类型串）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SqlType {
    BigInt { unsigned: bool },
    Int { unsigned: bool },
    SmallInt { unsigned: bool },
    TinyInt { unsigned: bool },
    Varchar(usize),
    Char(usize),
    Text,
    MediumText,
    LongText,
    DateTime,
    Timestamp,
    Date,
    Decimal { precision: u8, scale: u8 },
    Double,
    Float,
    Json,
    Blob,
    LongBlob,
}

impl SqlType {
    /// 转成 DDL 里的类型片段。
    pub fn to_sql(&self) -> String {
        match self {
            SqlType::BigInt { unsigned } => unsigned_suffix("bigint", *unsigned),
            SqlType::Int { unsigned } => unsigned_suffix("int", *unsigned),
            SqlType::SmallInt { unsigned } => unsigned_suffix("smallint", *unsigned),
            SqlType::TinyInt { unsigned } => unsigned_suffix("tinyint", *unsigned),
            SqlType::Varchar(length) => format!("varchar({length})"),
            SqlType::Char(length) => format!("char({length})"),
            SqlType::Text => "text".to_string(),
            SqlType::MediumText => "mediumtext".to_string(),
            SqlType::LongText => "longtext".to_string(),
            SqlType::DateTime => "datetime".to_string(),
            SqlType::Timestamp => "timestamp".to_string(),
            SqlType::Date => "date".to_string(),
            SqlType::Decimal { precision, scale } => format!("decimal({precision},{scale})"),
            SqlType::Double => "double".to_string(),
            SqlType::Float => "float".to_string(),
            SqlType::Json => "json".to_string(),
            SqlType::Blob => "blob".to_string(),
            SqlType::LongBlob => "longblob".to_string(),
        }
    }
}

/// 拼接整型类型与 `unsigned` 后缀。
fn unsigned_suffix(base: &str, unsigned: bool) -> String {
    if unsigned {
        format!("{base} unsigned")
    } else {
        base.to_string()
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
    /// 数据库里有、定义里没有的列（`表.列`）。
    ///
    /// 与 GORM 一致：**只报告不删除**，方便发现历史遗留列。
    pub extra_columns: Vec<String>,
}

impl MigrationReport {
    /// 是否没有任何改动（幂等执行时为 true）。
    ///
    /// 仅多出历史列时仍算 no-op（没有执行任何 DDL）。
    pub fn is_noop(&self) -> bool {
        self.created_tables.is_empty()
            && self.added_columns.is_empty()
            && self.modified_columns.is_empty()
            && self.created_indexes.is_empty()
    }
}

/// 归一化 SQL 类型（忽略整型显示宽度、大小写与多余空白）。
///
/// MySQL 的 `COLUMN_TYPE` 会带上显示宽度（`bigint(20) unsigned`），
/// 而定义里通常写 `bigint unsigned`，不归一化会产生大量无意义的 `MODIFY COLUMN`。
pub fn normalize_type(raw: &str) -> String {
    let mut value = collapse_spaces(&raw.trim().to_ascii_lowercase());

    // 去掉整型显示宽度；先长后短，避免 "int" 抢到 "bigint" 的前缀。
    for base in ["tinyint", "smallint", "mediumint", "bigint", "int"] {
        let Some(rest) = value.strip_prefix(base) else {
            continue;
        };
        let rest = rest.trim_start();
        if let Some(inner) = rest.strip_prefix('(') {
            if let Some(end) = inner.find(')') {
                let digits = &inner[..end];
                if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
                    let tail = inner[end + 1..].trim();
                    value = if tail.is_empty() {
                        base.to_string()
                    } else {
                        format!("{base} {tail}")
                    };
                }
            }
        }
        break;
    }
    value
}

/// 把连续空白折叠成单个空格并去掉首尾空白。
fn collapse_spaces(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut last_space = false;
    for ch in raw.chars() {
        if ch.is_whitespace() {
            if !last_space {
                out.push(' ');
            }
            last_space = true;
        } else {
            out.push(ch);
            last_space = false;
        }
    }
    out.trim().to_string()
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

/// 生成 `ALTER TABLE ... ADD COLUMN` 语句（支持 `AFTER` 指定位置）。
pub fn add_column_sql(table: &str, column: &ColumnDef) -> String {
    let mut sql = format!("ALTER TABLE `{table}` ADD COLUMN {}", column.clause());
    if let Some(after) = &column.after {
        sql.push_str(&format!(" AFTER `{after}`"));
    }
    sql
}

/// 生成 `ALTER TABLE ... MODIFY COLUMN` 语句。
pub fn modify_column_sql(table: &str, column: &ColumnDef) -> String {
    format!("ALTER TABLE `{table}` MODIFY COLUMN {}", column.clause())
}

/// 生成 `CREATE INDEX` / `CREATE UNIQUE INDEX` 语句。
pub fn create_index_sql(table: &str, index: &IndexDef) -> String {
    let columns = index
        .columns
        .iter()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "CREATE {}INDEX `{}` ON `{}` ({columns})",
        if index.unique { "UNIQUE " } else { "" },
        index.name,
        table
    )
}

/// 数据库里现有的列（纯逻辑对比用，不含连接）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingColumnInfo {
    pub name: String,
    /// `information_schema.COLUMNS.COLUMN_TYPE`，如 `bigint(20) unsigned`。
    pub column_type: String,
    pub nullable: bool,
}

/// 对比期望列与现有列：返回（缺失的列, 需要改类型/可空性的列）。
pub fn diff_columns<'a>(
    wanted: &'a [ColumnDef],
    existing: &[ExistingColumnInfo],
) -> (Vec<&'a ColumnDef>, Vec<&'a ColumnDef>) {
    let mut missing = Vec::new();
    let mut changed = Vec::new();
    for column in wanted {
        match existing
            .iter()
            .find(|item| item.name.eq_ignore_ascii_case(&column.name))
        {
            None => missing.push(column),
            Some(current) => {
                if normalize_type(&column.sql_type) != normalize_type(&current.column_type)
                    || current.nullable != column.nullable
                {
                    changed.push(column);
                }
            }
        }
    }
    (missing, changed)
}

/// 数据库里有、定义里没有的列名（GORM 不删列，这里只报告）。
pub fn extra_columns(wanted: &[ColumnDef], existing: &[ExistingColumnInfo]) -> Vec<String> {
    let mut list: Vec<String> = existing
        .iter()
        .filter(|item| {
            !wanted
                .iter()
                .any(|column| column.name.eq_ignore_ascii_case(&item.name))
        })
        .map(|item| item.name.clone())
        .collect();
    list.sort();
    list
}

/// 数据库里现有的索引（纯逻辑对比用，不含连接）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExistingIndexInfo {
    pub name: String,
    /// 按 `SEQ_IN_INDEX` 排好序的列名。
    pub columns: Vec<String>,
    pub unique: bool,
}

/// 期望的索引是否已被某个现有索引覆盖（列与列序相同、唯一性相同）。
///
/// 用于避免「索引改名后又建一个同列索引」这种重复索引。
pub fn index_covered(existing: &[ExistingIndexInfo], wanted: &IndexDef) -> bool {
    existing.iter().any(|item| {
        item.unique == wanted.unique
            && item.columns.len() == wanted.columns.len()
            && item
                .columns
                .iter()
                .zip(wanted.columns.iter())
                .all(|(left, right)| left.eq_ignore_ascii_case(right))
    })
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

        // 列对比：缺失则新增，类型/可空性不一致则修改；多出来的列只报告不删。
        let existing = self.existing_columns(&table.name).await?;
        let (missing, changed) = diff_columns(&table.columns, &existing);
        for column in missing {
            let sql = add_column_sql(&table.name, column);
            sqlx::query(&sql)
                .execute(&self.pool)
                .await
                .with_context(|| format!("给 {} 添加列 {} 失败", table.name, column.name))?;
            report
                .added_columns
                .push(format!("{}.{}", table.name, column.name));
        }
        for column in changed {
            let sql = modify_column_sql(&table.name, column);
            sqlx::query(&sql)
                .execute(&self.pool)
                .await
                .with_context(|| format!("修改 {}.{} 失败", table.name, column.name))?;
            report
                .modified_columns
                .push(format!("{}.{}", table.name, column.name));
        }
        for name in extra_columns(&table.columns, &existing) {
            report.extra_columns.push(format!("{}.{}", table.name, name));
        }

        // 索引对比：按名字跳过；名字不同但列与唯一性完全一致的也跳过（避免重复索引）。
        let indexes = self.existing_indexes(&table.name).await?;
        let names: std::collections::HashSet<String> = indexes
            .iter()
            .map(|item| item.name.to_ascii_lowercase())
            .collect();
        for index in &table.indexes {
            if names.contains(&index.name.to_ascii_lowercase()) || index_covered(&indexes, index) {
                continue;
            }
            let sql = create_index_sql(&table.name, index);
            sqlx::query(&sql)
                .execute(&self.pool)
                .await
                .with_context(|| format!("给 {} 创建索引 {} 失败", table.name, index.name))?;
            report
                .created_indexes
                .push(format!("{}.{}", table.name, index.name));
        }
        Ok(())
    }

    /// 只补缺失列（不改已有列类型、不建表、不碰索引），返回新增的列名。
    ///
    /// 与 [`Migrator::migrate_table`] 的差别就是「不 MODIFY」：`migrate_table`
    /// 会把类型/可空性不一致的列改掉，而 `setting::migrate_db` 与
    /// `module::migrate` 的既有语义是「表已存在时只 ADD COLUMN」——
    /// 后台「一键迁移」不应该顺手改用户已有列的定义。列比较复用
    /// [`diff_columns`]，与完整迁移保持同一套类型归一规则。
    pub async fn ensure_columns(
        &self,
        table: &str,
        columns: &[ColumnDef],
    ) -> Result<Vec<String>> {
        if columns.is_empty() {
            return Ok(Vec::new());
        }
        let existing = self.existing_columns(table).await?;
        let (missing, _changed) = diff_columns(columns, &existing);
        let mut added = Vec::with_capacity(missing.len());
        for column in missing {
            let sql = add_column_sql(table, column);
            sqlx::query(&sql)
                .execute(&self.pool)
                .await
                .with_context(|| format!("给 {table} 添加列 {} 失败", column.name))?;
            added.push(column.name.clone());
        }
        Ok(added)
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
    async fn existing_columns(&self, table: &str) -> Result<Vec<ExistingColumnInfo>> {
        use sqlx::Row;
        let rows = sqlx::query(
            "SELECT COLUMN_NAME, COLUMN_TYPE, IS_NULLABLE \
             FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? \
             ORDER BY ORDINAL_POSITION",
        )
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .context("读取列信息失败")?;
        let mut list = Vec::with_capacity(rows.len());
        for row in rows {
            let nullable_text: String = row.try_get("IS_NULLABLE")?;
            list.push(ExistingColumnInfo {
                name: row.try_get("COLUMN_NAME")?,
                column_type: row.try_get("COLUMN_TYPE")?,
                nullable: nullable_text.eq_ignore_ascii_case("YES"),
            });
        }
        Ok(list)
    }

    /// 读取现有索引（按索引名聚合列，列按 `SEQ_IN_INDEX` 排序）。
    async fn existing_indexes(&self, table: &str) -> Result<Vec<ExistingIndexInfo>> {
        use sqlx::Row;
        let rows = sqlx::query(
            "SELECT INDEX_NAME, COLUMN_NAME, NON_UNIQUE, SEQ_IN_INDEX \
             FROM information_schema.STATISTICS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? \
             ORDER BY INDEX_NAME, SEQ_IN_INDEX",
        )
        .bind(table)
        .fetch_all(&self.pool)
        .await
        .context("读取索引信息失败")?;
        let mut list: Vec<ExistingIndexInfo> = Vec::new();
        for row in rows {
            let name: String = row.try_get("INDEX_NAME")?;
            let column: Option<String> = row.try_get("COLUMN_NAME").ok();
            let non_unique: i64 = row.try_get("NON_UNIQUE").unwrap_or(1);
            match list.iter_mut().find(|item| item.name == name) {
                Some(item) => {
                    if let Some(column) = column {
                        item.columns.push(column);
                    }
                }
                None => list.push(ExistingIndexInfo {
                    name,
                    columns: column.into_iter().collect(),
                    unique: non_unique == 0,
                }),
            }
        }
        Ok(list)
    }
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

    #[test]
    fn normalize_type_strips_display_width_with_unsigned() {
        // information_schema 给的是带显示宽度的形态，定义里通常不带。
        assert_eq!(normalize_type("bigint(20) unsigned"), "bigint unsigned");
        assert_eq!(normalize_type("int(10) unsigned"), "int unsigned");
        assert_eq!(normalize_type("tinyint(3)"), "tinyint");
        assert_eq!(normalize_type("int(10)"), "int");
        // 空白折叠与大小写。
        assert_eq!(normalize_type("  INT(11)   UNSIGNED "), "int unsigned");
        // 非整型保持原样（显示宽度对它们有意义）。
        assert_eq!(normalize_type("varchar(255)"), "varchar(255)");
        assert_eq!(normalize_type("decimal(10,2)"), "decimal(10,2)");
        // 名字里带 int 但不是整型，不能被误删。
        assert_eq!(normalize_type("interval_day"), "interval_day");
    }

    #[test]
    fn sql_type_maps_to_mysql_ddl() {
        assert_eq!(SqlType::BigInt { unsigned: true }.to_sql(), "bigint unsigned");
        assert_eq!(SqlType::Int { unsigned: false }.to_sql(), "int");
        assert_eq!(SqlType::TinyInt { unsigned: true }.to_sql(), "tinyint unsigned");
        assert_eq!(SqlType::Varchar(120).to_sql(), "varchar(120)");
        assert_eq!(SqlType::Char(16).to_sql(), "char(16)");
        assert_eq!(SqlType::LongText.to_sql(), "longtext");
        assert_eq!(
            SqlType::Decimal {
                precision: 10,
                scale: 2
            }
            .to_sql(),
            "decimal(10,2)"
        );
        let column = ColumnDef::typed("hits", SqlType::Int { unsigned: true })
            .not_null()
            .with_default("0");
        assert_eq!(column.sql_type, "int unsigned");
        assert!(!column.nullable);
    }

    #[test]
    fn add_column_sql_supports_after_and_auto_increment() {
        let plain = ColumnDef::new("body", "text");
        assert_eq!(
            add_column_sql("demo", &plain),
            "ALTER TABLE `demo` ADD COLUMN `body` text"
        );

        let placed = ColumnDef::new("title", "varchar(80)")
            .not_null()
            .with_default("''")
            .after("id");
        assert_eq!(
            add_column_sql("demo", &placed),
            "ALTER TABLE `demo` ADD COLUMN `title` varchar(80) NOT NULL DEFAULT '' AFTER `id`"
        );

        let auto = ColumnDef::typed("id", SqlType::Int { unsigned: true })
            .not_null()
            .auto_increment();
        assert_eq!(
            add_column_sql("demo", &auto),
            "ALTER TABLE `demo` ADD COLUMN `id` int unsigned NOT NULL AUTO_INCREMENT"
        );
    }

    #[test]
    fn modify_column_sql_rewrites_full_definition() {
        let column = ColumnDef::new("title", "varchar(255)").not_null();
        assert_eq!(
            modify_column_sql("demo", &column),
            "ALTER TABLE `demo` MODIFY COLUMN `title` varchar(255) NOT NULL"
        );
    }

    #[test]
    fn create_index_sql_marks_unique() {
        let normal = IndexDef::new("idx_time", vec!["created_time".into()]);
        assert_eq!(
            create_index_sql("demo", &normal),
            "CREATE INDEX `idx_time` ON `demo` (`created_time`)"
        );
        let unique = IndexDef::unique("uk_slug", vec!["slug".into(), "lang".into()]);
        assert_eq!(
            create_index_sql("demo", &unique),
            "CREATE UNIQUE INDEX `uk_slug` ON `demo` (`slug`, `lang`)"
        );
    }

    #[test]
    fn diff_columns_reports_missing_and_changed() {
        let wanted = vec![
            ColumnDef::typed("id", SqlType::BigInt { unsigned: true }).not_null(),
            ColumnDef::new("title", "varchar(255)"),
            ColumnDef::new("body", "text"),
            ColumnDef::new("hits", "int unsigned").not_null(),
        ];
        let existing = vec![
            // 显示宽度差异不算变化。
            ExistingColumnInfo {
                name: "ID".into(),
                column_type: "bigint(20) unsigned".into(),
                nullable: false,
            },
            // 长度变化要改。
            ExistingColumnInfo {
                name: "title".into(),
                column_type: "varchar(64)".into(),
                nullable: true,
            },
            // 可空性变化也要改。
            ExistingColumnInfo {
                name: "hits".into(),
                column_type: "int(10) unsigned".into(),
                nullable: true,
            },
            // 定义里没有的历史列。
            ExistingColumnInfo {
                name: "deleted_at".into(),
                column_type: "datetime".into(),
                nullable: true,
            },
        ];
        let (missing, changed) = diff_columns(&wanted, &existing);
        let missing: Vec<&str> = missing.iter().map(|c| c.name.as_str()).collect();
        let changed: Vec<&str> = changed.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(missing, vec!["body"]);
        assert_eq!(changed, vec!["title", "hits"]);
        assert_eq!(
            extra_columns(&wanted, &existing),
            vec!["deleted_at".to_string()]
        );
        // 历史列不进 is_noop 的判定。
        let mut report = MigrationReport::default();
        report.extra_columns.push("demo.deleted_at".into());
        assert!(report.is_noop());
    }

    #[test]
    fn index_covered_matches_columns_and_uniqueness() {
        let existing = vec![ExistingIndexInfo {
            name: "idx_old_name".into(),
            columns: vec!["created_time".into(), "id".into()],
            unique: false,
        }];
        // 改名但列与原索引一致 → 视为已覆盖。
        assert!(index_covered(
            &existing,
            &IndexDef::new("idx_new_name", vec!["created_time".into(), "id".into()])
        ));
        // 列序不同（联合索引左前缀不同）→ 需要新建。
        assert!(!index_covered(
            &existing,
            &IndexDef::new("idx_new_name", vec!["id".into(), "created_time".into()])
        ));
        // 唯一性不同 → 需要新建。
        assert!(!index_covered(
            &existing,
            &IndexDef::unique("uk_new_name", vec!["created_time".into(), "id".into()])
        ));
        // 列数不同 → 需要新建。
        assert!(!index_covered(
            &existing,
            &IndexDef::new("idx_new_name", vec!["created_time".into()])
        ));
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

    /// `ensure_columns` 是「只补不改」：缺的补上，已有列**绝不** MODIFY。
    #[tokio::test]
    async fn ensure_columns_only_adds_and_never_modifies() {
        let Some(pool) = test_pool().await else {
            eprintln!("跳过：未配置 RUSTCMS_TEST_DATABASE_URL");
            return;
        };
        let migrator = Migrator::new(pool.clone());
        let table_name = temp_table_name("ensurecol");
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

        // title 定义成更长的类型 + 新增两列：只应补两列，title 保持 varchar(32)。
        let added = migrator
            .ensure_columns(
                &table_name,
                &[
                    ColumnDef::new("title", "varchar(255)"),
                    ColumnDef::new("api_token", "varchar(64)")
                        .not_null()
                        .with_default("''"),
                    ColumnDef::new("body", "text").with_default("NULL"),
                ],
            )
            .await
            .expect("ensure_columns");
        assert_eq!(added, vec!["api_token".to_string(), "body".to_string()], "{added:?}");

        // 再跑一次必须幂等（返回空）。
        let again = migrator
            .ensure_columns(
                &table_name,
                &[ColumnDef::new("api_token", "varchar(64)").not_null()],
            )
            .await
            .expect("ensure_columns");
        assert!(again.is_empty(), "{again:?}");

        // 用 information_schema 复核：title 没被改长、被补的列确实存在。
        let title_type: String = sqlx::query_scalar(
            "SELECT COLUMN_TYPE FROM information_schema.COLUMNS \
             WHERE TABLE_SCHEMA = DATABASE() AND TABLE_NAME = ? AND COLUMN_NAME = 'title'",
        )
        .bind(&table_name)
        .fetch_one(&pool)
        .await
        .expect("read title type");
        assert_eq!(title_type, "varchar(32)");

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
