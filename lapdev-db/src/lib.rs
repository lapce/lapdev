pub mod api;
pub mod entities;
pub mod links;
pub mod migration;
pub mod relations;

pub mod tests {
    use anyhow::Result;
    use sea_orm::Database;
    use sea_orm_migration::MigratorTrait;

    use crate::{api::DbApi, migration::Migrator};

    pub async fn prepare_db() -> Result<DbApi> {
        let conn = Database::connect("sqlite::memory:").await?;
        let db = DbApi { conn, pool: None };
        Migrator::up(&db.conn, None).await?;
        Ok(db)
    }
}
