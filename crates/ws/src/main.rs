#[tokio::main]
pub async fn main() {
    lapdev_ws::server::start().await;
}
