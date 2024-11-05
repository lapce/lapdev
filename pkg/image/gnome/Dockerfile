FROM debian:12

ENV LANG en_US.UTF-8

# cleanup script for use after apt-get
RUN echo '#! /bin/sh\n\
env DEBIAN_FRONTEND=noninteractive apt-get autoremove -y\n\
apt-get clean\n\
find /var/lib/apt/lists -type f -delete\n\
find /var/cache -type f -delete\n\
find /var/log -type f -delete\n\
exit 0\n\
' > /cleanup && chmod +x /cleanup

# basics
RUN apt-get update && \
    env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      locales && \
      echo "$LANG UTF-8" >> /etc/locale.gen && \
      locale-gen && \
    env DEBIAN_FRONTEND=noninteractive apt-get install -y \
      at-spi2-core \
      dbus \
      dbus-x11 \
      libpulse0 \
      procps \
      psutils \
      systemd \
      x11-xserver-utils && \
    /cleanup

# Gnome 3
RUN apt-get update && \
    env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      gnome-session && \
    /cleanup

# Gnome 3 apps
RUN apt-get update && \
    env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      cheese \
      gedit \
      gnome-control-center \
      gnome-system-monitor \
      gnome-terminal \
      gnome-tweaks \
      nautilus && \
    /cleanup

# Gnome Shell extensions
RUN apt-get update && \
    env DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      gnome-shell-extension* && \
    /cleanup
    
USER root
COPY ../../../target/x86_64-unknown-linux-musl/release/lapdev-guest-agent /lapdev-guest-agent
RUN chmod +x /lapdev-guest-agent
COPY ../../../lapdev-ws/scripts/install_guest_agent.sh /install_guest_agent.sh
RUN sh /install_guest_agent.sh
RUN rm /install_guest_agent.sh

COPY ./pkg/image/gnome/install.sh /tmp/install.sh
RUN chmod +x /tmp/install.sh
RUN /tmp/install.sh
RUN rm /tmp/install.sh
ENV DISPLAY=:1
ENTRYPOINT /usr/local/share/desktop-init.sh
