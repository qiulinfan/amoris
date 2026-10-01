# A Linux machine to build and test Pocket on (docs/build-system.md, Linux): Ubuntu 26.04 with clang,
# libstdc++, CMake, Ninja, Rust, WebKitGTK's JavaScriptCore, SDL3's build dependencies, and Mesa's
# software Vulkan (lavapipe), so the renderer runs headless without a GPU.
FROM ubuntu:26.04
ENV DEBIAN_FRONTEND=noninteractive
RUN apt-get update && apt-get install -y --no-install-recommends \
        clang lld libclang-rt-21-dev libstdc++-15-dev cmake ninja-build pkg-config make curl ca-certificates git python3 xz-utils unzip rsync file \
        libjavascriptcoregtk-4.1-dev \
        mesa-vulkan-drivers libvulkan1 vulkan-tools \
        libx11-dev libxext-dev libxrandr-dev libxcursor-dev libxi-dev libxss-dev libxfixes-dev libxtst-dev libxkbcommon-dev libxrender-dev libxinerama-dev \
        libwayland-dev wayland-protocols libdecor-0-dev libasound2-dev libpulse-dev libudev-dev libdbus-1-dev libdrm-dev libgbm-dev libegl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable
ENV PATH=/root/.cargo/bin:$PATH CC=clang CXX=clang++
WORKDIR /work
