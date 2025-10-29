# Lapdev Kubernetes Service Images

This directory contains the multi-stage Dockerfile we use to build and publish
the service binaries that run inside customer clusters:

- `lapdev-api`
- `lapdev-kube-manager`
- `lapdev-kube-sidecar-proxy`

## Building images locally

The Dockerfile builds all binaries in a shared builder stage and exposes
named runtime targets. Select the service you want with `--target`:

```bash
# Lapdev API
podman build \
  -f pkg/kube-container/Dockerfile \
  --target lapdev-api \
  -t ghcr.io/lapdev/lapdev-api:0.1.0 \
  .

# Kube manager
podman build \
  -f pkg/kube-container/Dockerfile \
  --target kube-manager \
  -t ghcr.io/lapdev/lapdev-kube-manager:0.1.0 \
  .

# Sidecar proxy
podman build \
  -f pkg/kube-container/Dockerfile \
  --target kube-sidecar-proxy \
  -t ghcr.io/lapdev/lapdev-kube-sidecar-proxy:0.1.0 \
  .

```

We publish these images from CI; customers do not need to build them.

Alternatively, run the helper script to build all images, tag them, and
push to GitHub Container Registry in one shot. The script derives the registries
and tags from `crates/api/src/kube_controller/container_images.rs` so the
published images stay in lockstep with the API's hard-coded references:

```bash
./pkg/kube-container/build_and_push.sh
# optional overrides:
#   --owner lapdev        # must match the repo constants
#   --registry ghcr.io    # must match the repo constants
```

Make sure you have already authenticated via `podman login ghcr.io`.
When cutting a new release, update `CONTAINER_IMAGE_TAG` in
`crates/api/src/kube_controller/container_images.rs` before invoking the script.

## Runtime expectations

### `lapdev-api`

- Exposes HTTP on `8080`, SSH proxy on `2222`, and preview URL proxy on `8443`.
- Configured entirely via environment variables (see `.env.example`), notably
  `LAPDEV_DB_URL`, `LAPDEV_BIND_ADDR`, and the various `*_PORT` overrides.
- Persists data under `/var/lib/lapdev`; mount that path if you need stateful data.
- Emits structured logs to stdout/stderr; collect them via your container platform.

### `lapdev-kube-manager`

- Listens on TCP ports `5001` (sidecar RPC) and `7771` (devbox RPC).
- Requires the token and endpoint env vars defined in `lapdev_common::kube`, e.g.
  `LAPDEV_KUBE_CLUSTER_TOKEN`, `LAPDEV_KUBE_CLUSTER_URL`, and
  `LAPDEV_KUBE_CLUSTER_TUNNEL_URL`.

### `lapdev-kube-sidecar-proxy`

- Listens on port `8080` by default and expects iptables redirected traffic.
- Requires `LAPDEV_ENVIRONMENT_ID`, `LAPDEV_ENVIRONMENT_AUTH_TOKEN`, and
  `LAPDEV_SIDECAR_PROXY_MANAGER_ADDR` to bootstrap (`crates/kube-sidecar-proxy`).
- Runs as root because it needs low-level socket access for SO\_ORIGINAL\_DST.
