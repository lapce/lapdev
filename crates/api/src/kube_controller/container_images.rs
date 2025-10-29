/// Shared image tag for Lapdev Kubernetes components.
pub const CONTAINER_IMAGE_TAG: &str = "0.1.0";

/// Registry/repository for the Lapdev API image.
pub(crate) const API_IMAGE_REPO: &str = "ghcr.io/lapce/lapdev-api";
/// Registry/repository for the kube-manager controller image.
pub(crate) const KUBE_MANAGER_IMAGE_REPO: &str = "ghcr.io/lapce/lapdev-kube-manager";
/// Registry/repository for the sidecar proxy image.
pub(crate) const SIDECAR_PROXY_IMAGE_REPO: &str = "ghcr.io/lapce/lapdev-kube-sidecar-proxy";
/// Registry/repository for the devbox proxy image.
pub(crate) const DEVBOX_PROXY_IMAGE_REPO: &str = "ghcr.io/lapce/lapdev-kube-devbox-proxy";

/// Helper returning the Lapdev API image reference with the shared tag.
#[allow(dead_code)]
pub(crate) fn api_image_reference() -> String {
    format!("{}:{}", API_IMAGE_REPO, CONTAINER_IMAGE_TAG)
}

/// Helper returning the kube-manager image reference with the shared tag.
#[allow(dead_code)]
pub(crate) fn kube_manager_image_reference() -> String {
    format!("{}:{}", KUBE_MANAGER_IMAGE_REPO, CONTAINER_IMAGE_TAG)
}

/// Helper returning the sidecar proxy image reference with the shared tag.
pub(crate) fn sidecar_proxy_image_reference() -> String {
    format!("{}:{}", SIDECAR_PROXY_IMAGE_REPO, CONTAINER_IMAGE_TAG)
}

/// Helper returning the devbox proxy image reference with the shared tag.
#[allow(dead_code)]
pub(crate) fn devbox_proxy_image_reference() -> String {
    format!("{}:{}", DEVBOX_PROXY_IMAGE_REPO, CONTAINER_IMAGE_TAG)
}
