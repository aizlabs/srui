# Linux dev box for the process explorer: builds srtop and the SSH bridge from a
# source tree exported by devbox.sh, then serves them over a real OpenSSH
# subsystem so the unmodified macOS client can connect to a live Linux /proc.
#
# Build context is produced by `devbox.sh build` (git archive of a revision plus
# the files in this directory). Never build it from a working tree directly: the
# context would carry keys and target directories into the image.
FROM rust:latest AS build
WORKDIR /src
COPY . /src
RUN cargo build --locked --manifest-path apps/srtop/Cargo.toml \
 && cargo build --locked --manifest-path server-rust/Cargo.toml -p srui-ssh-bridge

FROM debian:bookworm-slim
RUN apt-get update \
 && apt-get install -y --no-install-recommends openssh-server procps ca-certificates \
 && rm -rf /var/lib/apt/lists/*
# `usermod -p '*'` leaves the account passwordless but unlocked; a plain useradd
# writes `!` into shadow, which sshd refuses with "account is locked" even for a
# valid public key.
RUN useradd -m -s /bin/bash srui && usermod -p '*' srui && mkdir -p /run/sshd
COPY --from=build /src/apps/srtop/target/debug/srtop /usr/local/bin/srtop
COPY --from=build /src/server-rust/target/debug/srui-ssh-bridge /usr/local/bin/srui-ssh-bridge
COPY devbox/sshd_config /etc/ssh/sshd_config.srui
COPY devbox/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh
EXPOSE 22
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
