#!/bin/sh
set -eu

main() {
  OS=${OS:-$(os)}
  ARCH=${ARCH:-$(arch)}
  DISTRO=${DISTRO:-$(distro)}
  install_ssh
  install_code_server
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

install_ssh() {
  case $DISTRO in
    debian) install_deb ;;
    fedora | opensuse) install_rpm ;;
    arch) install_aur ;;
    *)
        echo "Unsupported package manager." ;;
  esac
  mkdir -p /run/sshd
}

install_deb() {
  apt update
  apt install -y openssh-server curl
}

install_rpm() {
  dnf install -y openssh-server curl
}

install_aur() {
  pacman -Sy
  pacman -Sy openssh curl
}

install_code_server() {
  curl -fsSL https://code-server.dev/install.sh | sh 
}

main "$@"
