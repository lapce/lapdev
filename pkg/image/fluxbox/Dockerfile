FROM debian:12

USER root
COPY ../../../target/release/lapdev-guest-agent /lapdev-guest-agent
RUN chmod +x /lapdev-guest-agent
COPY ../../../lapdev-ws/scripts/install_guest_agent.sh /install_guest_agent.sh
RUN sh /install_guest_agent.sh
RUN rm /install_guest_agent.sh

COPY ./pkg/image/fluxbox/install.sh /tmp/install.sh
RUN chmod +x /tmp/install.sh
RUN /tmp/install.sh
RUN rm /tmp/install.sh
ENV DISPLAY=:1
ENTRYPOINT /usr/local/share/desktop-init.sh
