use lapdev_db_migration::Migrator;

#[tokio::main]
pub async fn main() {
    sea_orm_migration::cli::run_cli(Migrator).await
}
