FROM debian:12

RUN apt update
RUN apt install -y curl build-essential git vim podman pkg-config libssl-dev libz-dev fuse-overlayfs s3cmd

RUN curl https://sh.rustup.rs -sSf | bash -s -- -y
RUN $HOME/.cargo/bin/rustup target add wasm32-unknown-unknown
RUN $HOME/.cargo/bin/rustup target add x86_64-unknown-linux-musl
RUN $HOME/.cargo/bin/cargo install trunk && rm -rf $HOME/.cargo/registry/

COPY ../../../target/x86_64-unknown-linux-musl/release/lapdev-guest-agent /lapdev-guest-agent
RUN chmod +x /lapdev-guest-agent
COPY ../../../lapdev-ws/scripts/install_guest_agent.sh /install_guest_agent.sh
RUN sh /install_guest_agent.sh && rm /install_guest_agent.sh

EXPOSE 8080
