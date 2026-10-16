FROM ubuntu:24.04 AS development

ARG DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
    build-essential clang lld cmake pkg-config curl ca-certificates git git-lfs python3 ripgrep \
    libgtk-3-dev libwebkit2gtk-4.1-dev libayatana-appindicator3-dev librsvg2-dev patchelf \
    libasound2-dev libasound2-plugins libudev-dev libdbus-1-dev libpulse-dev \
    libx11-dev libxcursor-dev libxi-dev libxrandr-dev libxss-dev libxtst-dev \
    libwayland-dev libxkbcommon-dev xvfb xauth xdotool x11-xserver-utils openbox weston dbus-x11 \
    pulseaudio pulseaudio-utils alsa-utils imagemagick fonts-dejavu-core \
    zenity at-spi2-core python3-pyatspi xdg-desktop-portal xdg-desktop-portal-gtk \
    && git lfs install --system --skip-repo \
    && rm -rf /var/lib/apt/lists/*

# Match the bind-mounted checkout's owner without running WebKit as root.
ARG CREST_UID=1000
RUN userdel -r ubuntu && useradd --create-home --uid "$CREST_UID" crest \
    && mkdir -p /opt/cargo/registry /opt/cargo/git /opt/rustup /workspace \
    && chown -R crest:crest /opt/cargo /opt/rustup /workspace
ENV CARGO_HOME=/opt/cargo RUSTUP_HOME=/opt/rustup
ENV PATH=/opt/cargo/bin:$PATH
USER crest
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/rustup-init.sh \
    && sh /tmp/rustup-init.sh -y --profile minimal --default-toolchain 1.96.0 --component clippy,rustfmt \
    && rm /tmp/rustup-init.sh
WORKDIR /workspace
CMD ["scripts/linux/with-desktop.sh", "scripts/linux/check.sh"]

# The visible desktop is optional; ordinary builds and automated witnesses do
# not need a remote desktop server.
FROM development AS desktop
USER root
RUN apt-get update && apt-get install -y --no-install-recommends \
    xpra xterm libjs-jquery python3-pil python3-lz4 python3-gst-1.0 gstreamer1.0-plugins-good \
    && rm -rf /var/lib/apt/lists/*
USER crest

FROM development AS default
