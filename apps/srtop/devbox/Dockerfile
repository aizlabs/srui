# Linux dev box for the process explorer: builds srtop and the SSH bridge from a
# source tree exported by devbox.sh, then serves them over a real OpenSSH
# subsystem so the unmodified macOS client can connect to a live Linux /proc.
#
# Build context is produced by `devbox.sh build` (git archive of a revision plus
# the files in this directory). Never build it from a working tree directly: the
# context would carry keys and target directories into the image.
#
# PX-026-G01: both base images are pinned by digest, so the R1 devbox is rebuilt from the same
# toolchain and the same Debian userland it was tested with. The build stage is
# rust:1-bookworm as of 2026-11-05 (rustc 1.99.1 58cf8f9c3, cargo 1.99.1); the runtime stage is
# debian:bookworm-slim as of 2026-11-04 (Debian 12.15). The apt packages installed below
# (openssh-server, procps) are whatever bookworm serves at build time and are not pinned; the R1
# live checks record the sshd version they ran against.
#
# PX-027: every binary is a release build (the showcase publishes release numbers), the image also
# carries PX-025's bounded demonstration workers (`srtop-demo`, `srtop-demo-worker`) for the
# measured workloads, and htop for the matched comparison, pinned to bookworm's 3.2.2-2.
FROM rust:1-bookworm@sha256:e6f2cff35d0f309acf8433b4f05878ed77092602c5065cc81801177da0bb3a9c AS build
WORKDIR /src
COPY . /src
ENV CARGO_TARGET_DIR=/target
RUN cargo build --release --locked --manifest-path apps/srtop/Cargo.toml \
 && cargo build --release --locked --manifest-path apps/srtop/demo/Cargo.toml \
 && cargo build --release --locked --manifest-path server-rust/Cargo.toml -p srui-ssh-bridge

FROM debian:bookworm-slim@sha256:734a72b3d6142a71993b4bc580ca01d8ca687f77a2c422d96a961d3376b179bc
RUN apt-get update \
 && apt-get install -y --no-install-recommends openssh-server procps ca-certificates htop=3.2.2-2 \
 && rm -rf /var/lib/apt/lists/*
# `usermod -p '*'` leaves the account passwordless but unlocked; a plain useradd
# writes `!` into shadow, which sshd refuses with "account is locked" even for a
# valid public key.
RUN useradd -m -s /bin/bash srui && usermod -p '*' srui && mkdir -p /run/sshd
COPY --from=build /target/release/srtop /usr/local/bin/srtop
COPY --from=build /target/release/srui-ssh-bridge /usr/local/bin/srui-ssh-bridge
COPY --from=build /target/release/srtop-demo /usr/local/bin/srtop-demo
COPY --from=build /target/release/srtop-demo-worker /usr/local/bin/srtop-demo-worker
COPY devbox/sshd_config /etc/ssh/sshd_config.srui
COPY devbox/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
EXPOSE 22
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
