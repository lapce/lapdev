#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct HrpcError {
    pub error: String,
}

impl From<HrpcError> for anyhow::Error {
    fn from(err: HrpcError) -> Self {
        anyhow::anyhow!(err.error)
    }
}
