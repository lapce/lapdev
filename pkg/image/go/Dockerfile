FROM mcr.microsoft.com/devcontainers/go:latest

USER root
COPY ../../../target/x86_64-unknown-linux-musl/release/lapdev-guest-agent /lapdev-guest-agent
RUN chmod +x /lapdev-guest-agent
COPY ../../../lapdev-ws/scripts/install_guest_agent.sh /install_guest_agent.sh
RUN sh /install_guest_agent.sh
RUN rm /install_guest_agent.sh