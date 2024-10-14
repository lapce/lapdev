FROM debian:12

COPY ./pkg/image/vnc/install.sh /tmp/install.sh
RUN chmod +x /tmp/install.sh
RUN /tmp/install.sh
ENV DISPLAY=:1
ENTRYPOINT /usr/local/share/desktop-init.sh
