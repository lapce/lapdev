#[tokio::main]
pub async fn main() {
    if let Err(e) = lapdev_kube_manager::manager::KubeManager::connect_cluster().await {
        println!("error: {e:?}");
    }
}
