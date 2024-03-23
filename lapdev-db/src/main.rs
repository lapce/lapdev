use lapdev_db::migration::Migrator;
use sea_orm_migration::cli;

#[tokio::main]
pub async fn main() {
    cli::run_cli(Migrator).await;
}
