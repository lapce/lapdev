#!/bin/sh
set -eu

VERSION=0.1.0

main() {
  OS=${OS:-$(os)}
  ARCH=${ARCH:-$(arch)}
  DISTRO=${DISTRO:-$(distro)}
  install
}

os() {
  uname="$(uname)"
  case $uname in
    Linux) echo linux ;;
    Darwin) echo macos ;;
    FreeBSD) echo freebsd ;;
    *) echo "$uname" ;;
  esac
}

distro() {
  if [ "$OS" = "macos" ] || [ "$OS" = "freebsd" ]; then
    echo "$OS"
    return
  fi

  if [ -f /etc/os-release ]; then
    (
      . /etc/os-release
      if [ "${ID_LIKE-}" ]; then
        for id_like in $ID_LIKE; do
          case "$id_like" in debian | fedora | opensuse | arch)
            echo "$id_like"
            return
            ;;
          esac
        done
      fi

      echo "$ID"
    )
    return
  fi
}

arch() {
  uname_m=$(uname -m)
  case $uname_m in
    aarch64) echo arm64 ;;
    x86_64) echo amd64 ;;
    *) echo "$uname_m" ;;
  esac
}

install() {
  case $DISTRO in
    debian) install_deb ;;
    fedora | opensuse) install_rpm ;;
    *)
        echo "Unsupported package manager." ;;
  esac
}

install_deb() {
  echo "Installing lapdev-ws package from GitHub."
  sudo apt update
  curl -sL -o /tmp/lapdev-ws_${VERSION}-1_amd64.deb https://github.com/lapce/lapdev/releases/download/v${VERSION}/lapdev-ws_${VERSION}-1_amd64.deb
  sudo apt install -y /tmp/lapdev-ws_${VERSION}-1_amd64.deb
  echo "Installing podman if it's not"
  sudo apt install -y fuse-overlayfs podman dbus-user-session golang-github-containernetworking-plugin-dnsname
  sudo loginctl enable-linger lapdev
}

install_rpm() {
  echo "Installing lapdev-ws package from GitHub."
  sudo yum install -y https://github.com/lapce/lapdev/releases/download/v${VERSION}/lapdev-ws-${VERSION}-1.x86_64.rpm
  echo "Installing podman if it's not"
  sudo yum install -y fuse-overlayfs podman
  sudo loginctl enable-linger lapdev
}

main "$@"
