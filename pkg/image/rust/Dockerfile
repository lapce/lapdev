FROM mcr.microsoft.com/devcontainers/rust:latest

USER root
COPY ../../../target/release/lapdev-guest-agent /lapdev-guest-agent
RUN chmod +x /lapdev-guest-agent
COPY ../../../lapdev-ws/scripts/install_guest_agent.sh /install_guest_agent.sh
RUN sh /install_guest_agent.sh
RUN rm /install_guest_agent.sh