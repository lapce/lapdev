pub mod auto_start_stop;
pub mod enterprise;
pub mod license;
pub mod quota;
pub mod usage;

#[cfg(test)]
pub mod tests {
    use anyhow::Result;
    use lapdev_db::tests::prepare_db;

    use crate::enterprise::Enterprise;

    pub async fn prepare_enterprise() -> Result<Enterprise> {
        let db = prepare_db().await?;
        let enterprise = Enterprise::new(db.clone()).await?;
        Ok(enterprise)
    }
}
