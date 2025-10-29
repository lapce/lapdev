#!/usr/bin/env bash

# Builds the kube service images and pushes them to GitHub Container Registry.

set -o errexit
set -o nounset
set -o pipefail

usage() {
  cat <<'EOF'
Usage: build_and_push.sh [--tag <tag>] [--owner <github-owner>] [--registry <registry>]

Optional:
  --tag       Override the image tag (must match the source constant)
  --owner     GitHub owner/namespace (defaults to source constant)
  --registry  Container registry host (defaults to source constant)

Environment:
  Requires prior `podman login` to the target registry.
EOF
}

TAG_OVERRIDE=""
REGISTRY="${REGISTRY:-}"
OWNER="${OWNER:-}"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --tag)
      TAG_OVERRIDE="${2:-}"
      shift 2
      ;;
    --owner)
      OWNER="${2:-}"
      shift 2
      ;;
    --registry)
      REGISTRY="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown argument: $1" >&2
      usage
      exit 1
      ;;
  esac
done

if [[ -z "$OWNER" ]]; then
  if [[ -n "${GITHUB_REPOSITORY:-}" ]]; then
    OWNER="${GITHUB_REPOSITORY%%/*}"
  elif git_remote=$(git config --get remote.origin.url 2>/dev/null || true); then
    case "$git_remote" in
      git@github.com:*)
        OWNER="${git_remote#git@github.com:}"
        OWNER="${OWNER%%/*}"
        ;;
      https://github.com/*)
        OWNER="${git_remote#https://github.com/}"
        OWNER="${OWNER%%/*}"
        ;;
      *)
        OWNER=""
        ;;
    esac
  fi
fi

if ! command -v podman >/dev/null 2>&1; then
  echo "Error: podman CLI not found in PATH." >&2
  exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
if ! REPO_ROOT="$(git -C "$SCRIPT_DIR" rev-parse --show-toplevel 2>/dev/null)"; then
  REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
fi

cd "$REPO_ROOT"

IMAGE_CONST_FILE="crates/api/src/kube_controller/container_images.rs"
if [[ ! -f "$IMAGE_CONST_FILE" ]]; then
  echo "Error: unable to locate ${IMAGE_CONST_FILE}" >&2
  exit 1
fi

extract_const_value() {
  local name="$1"
  local value
  value=$(grep -E "const ${name}: &str" "$IMAGE_CONST_FILE" | head -n1 | sed -E "s/.*const ${name}: &str = \"([^\"]+)\".*/\\1/")
  printf '%s' "$value"
}

parse_repo_components() {
  local repo="$1"
  local registry="${repo%%/*}"
  if [[ "$registry" == "$repo" ]]; then
    echo "Error: repo '${repo}' is missing registry" >&2
    exit 1
  fi
  local remainder="${repo#${registry}/}"
  local owner="${remainder%%/*}"
  if [[ "$owner" == "$remainder" ]]; then
    echo "Error: repo '${repo}' is missing owner" >&2
    exit 1
  fi
  local image_name="${remainder#${owner}/}"
  if [[ -z "$image_name" || "$image_name" == "$owner" ]]; then
    echo "Error: repo '${repo}' is missing image name" >&2
    exit 1
  fi
  printf '%s|%s|%s' "$registry" "$owner" "$image_name"
}

API_REPO=$(extract_const_value "API_IMAGE_REPO")
SIDECAR_REPO=$(extract_const_value "SIDECAR_PROXY_IMAGE_REPO")
KUBE_MANAGER_REPO=$(extract_const_value "KUBE_MANAGER_IMAGE_REPO")
DEVBOX_REPO=$(extract_const_value "DEVBOX_PROXY_IMAGE_REPO")
CONTAINER_IMAGE_TAG=$(extract_const_value "CONTAINER_IMAGE_TAG")

if [[ -z "$API_REPO" || -z "$SIDECAR_REPO" || -z "$KUBE_MANAGER_REPO" || -z "$DEVBOX_REPO" || -z "$CONTAINER_IMAGE_TAG" ]]; then
  echo "Error: failed to parse image constants from ${IMAGE_CONST_FILE}" >&2
  exit 1
fi

IFS='|' read -r API_REGISTRY API_OWNER API_IMAGE_NAME <<< "$(parse_repo_components "$API_REPO")"
IFS='|' read -r SIDECAR_REGISTRY SIDECAR_OWNER SIDECAR_IMAGE_NAME <<< "$(parse_repo_components "$SIDECAR_REPO")"
IFS='|' read -r KUBE_MANAGER_REGISTRY KUBE_MANAGER_OWNER KUBE_MANAGER_IMAGE_NAME <<< "$(parse_repo_components "$KUBE_MANAGER_REPO")"
IFS='|' read -r DEVBOX_REGISTRY DEVBOX_OWNER DEVBOX_IMAGE_NAME <<< "$(parse_repo_components "$DEVBOX_REPO")"

if [[ "$API_REGISTRY" != "$SIDECAR_REGISTRY" || "$SIDECAR_REGISTRY" != "$KUBE_MANAGER_REGISTRY" || "$SIDECAR_REGISTRY" != "$DEVBOX_REGISTRY" ]]; then
  echo "Error: image registries differ between components" >&2
  exit 1
fi

if [[ "$API_OWNER" != "$SIDECAR_OWNER" || "$SIDECAR_OWNER" != "$KUBE_MANAGER_OWNER" || "$SIDECAR_OWNER" != "$DEVBOX_OWNER" ]]; then
  echo "Error: image owners differ between components" >&2
  exit 1
fi

if [[ -z "$REGISTRY" ]]; then
  REGISTRY="$SIDECAR_REGISTRY"
fi

if [[ -z "$OWNER" ]]; then
  OWNER="$SIDECAR_OWNER"
fi

if [[ "$REGISTRY" != "$SIDECAR_REGISTRY" ]]; then
  echo "Error: registry '${REGISTRY}' does not match source registry '${SIDECAR_REGISTRY}'." >&2
  exit 1
fi

if [[ "$OWNER" != "$SIDECAR_OWNER" ]]; then
  echo "Error: owner '${OWNER}' does not match source owner '${SIDECAR_OWNER}'." >&2
  exit 1
fi

TAG=""

if [[ -z "$TAG_OVERRIDE" ]]; then
  TAG="$CONTAINER_IMAGE_TAG"
else
  TAG="$TAG_OVERRIDE"
  if [[ "$TAG_OVERRIDE" != "$CONTAINER_IMAGE_TAG" ]]; then
    echo "Error: provided --tag '${TAG_OVERRIDE}' does not match source constant '${CONTAINER_IMAGE_TAG}'." >&2
    exit 1
  fi
fi

declare -a TARGETS=(
  "lapdev-api|${API_IMAGE_NAME}|${API_REPO}"
  "kube-manager|${KUBE_MANAGER_IMAGE_NAME}|${KUBE_MANAGER_REPO}"
  "kube-sidecar-proxy|${SIDECAR_IMAGE_NAME}|${SIDECAR_REPO}"
  "kube-devbox-proxy|${DEVBOX_IMAGE_NAME}|${DEVBOX_REPO}"
)

build_image() {
  local target="$1"
  local image="$2"
  local repo="$3"
  local tag="$4"

  local ref="${REGISTRY}/${OWNER}/${image}:${tag}"
  local expected_ref="${repo}:${tag}"

  if [[ "$ref" != "$expected_ref" ]]; then
    echo "Error: image ref '${ref}' does not match source constant '${expected_ref}'." >&2
    exit 1
  fi

  echo "Building ${ref} (target=${target})"
  podman build \
    -f pkg/kube-container/Dockerfile \
    --target "${target}" \
    -t "${ref}" \
    .

  echo "Pushing ${ref}"
  podman push "${ref}"
}

for entry in "${TARGETS[@]}"; do
  IFS='|' read -r target image repo <<< "${entry}"
  build_image "${target}" "${image}" "${repo}" "${TAG}"
done

echo "All images built and pushed with tag '${TAG}' to ${REGISTRY}/${OWNER}/"
