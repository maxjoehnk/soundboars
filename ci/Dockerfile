# Headless CI image: Rust toolchain plus every system dependency SoundBoars needs at build and test
# time (PipeWire + WirePlumber + D-Bus, X11/Xvfb, ALSA, ffmpeg, yt-dlp).
FROM rust:1-trixie

RUN rustup component add rustfmt clippy

RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config clang libclang-dev \
        libpipewire-0.3-dev libspa-0.2-dev pipewire pipewire-bin wireplumber \
        dbus dbus-bin \
        libasound2-dev alsa-utils \
        libx11-dev libxi-dev libxtst-dev libxrandr-dev libxcursor-dev libxkbcommon-dev \
        libxkbcommon-x11-dev libwayland-dev libgl1-mesa-dev mesa-vulkan-drivers \
        xvfb xdotool \
        ffmpeg yt-dlp python3 \
    && rm -rf /var/lib/apt/lists/*

# Containers run as the host user (`--user $(id -u):$(id -g)`) with CARGO_HOME pointing into the
# mounted workspace. Keep the image's rustup toolchain reachable and readable for any uid.
ENV RUSTUP_HOME=/usr/local/rustup \
    PATH=/usr/local/cargo/bin:$PATH
RUN chmod -R a+rX /usr/local/rustup /usr/local/cargo

# dbus-daemon refuses to start for a uid without a passwd entry; scripts/test-headless.sh adds one
# for the host user at runtime (containers are `--rm`, so this never persists).
RUN chmod a+w /etc/passwd
