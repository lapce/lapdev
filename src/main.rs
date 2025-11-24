#[tokio::main]
pub async fn main() {
    let _ = dotenvy::dotenv();
    lapdev_api::server::start(None).await;
}
