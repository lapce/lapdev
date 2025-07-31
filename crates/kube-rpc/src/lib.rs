#[tarpc::service]
pub trait KubeService {
    async fn connect_cluster(token: String);
}
