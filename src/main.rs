#[tokio::main]
pub async fn main() {
    lapdev_api::server::start(None).await;
}
