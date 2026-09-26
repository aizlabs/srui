#!/bin/sh
# Runs as root inside the container: install the client's public key, start srtop
# as the unprivileged user, then hand the container over to sshd.
#
# SRTOP_ARGS  extra srtop flags (default: --live-source --refresh-interval-ms 1000)
set -eu

SRTOP_ARGS="${SRTOP_ARGS:---live-source --refresh-interval-ms 1000}"

mkdir -p /home/srui/.ssh /home/srui/run
cp /keys/id_ed25519.pub /home/srui/.ssh/authorized_keys
chown -R srui:srui /home/srui/.ssh /home/srui/run
chmod 700 /home/srui/.ssh /home/srui/run
chmod 600 /home/srui/.ssh/authorized_keys

ssh-keygen -A >/dev/null

# srtop refuses a socket directory that is not private to a non-root user, so it
# runs as srui and sshd hands each subsystem invocation to the same account.
# shellcheck disable=SC2086
setpriv --reuid=srui --regid=srui --init-groups \
  /usr/local/bin/srtop --socket /home/srui/run/srtop.sock $SRTOP_ARGS \
  >/var/log/srtop.log 2>&1 &

for _ in $(seq 1 50); do
  [ -S /home/srui/run/srtop.sock ] && break
  sleep 0.2
done
if [ ! -S /home/srui/run/srtop.sock ]; then
  echo 'srtop never bound its socket:' >&2
  cat /var/log/srtop.log >&2
  exit 1
fi

echo "srtop listening on /home/srui/run/srtop.sock (args: $SRTOP_ARGS)"
exec /usr/sbin/sshd -D -e -f /etc/ssh/sshd_config.srui
