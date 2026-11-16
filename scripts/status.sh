#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib/host.sh
source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/lib/host.sh"
[[ $# == 0 ]] || wrkgn_die 'Usage: scripts/status.sh'
wrkgn_check_ssh_alias
wrkgn_ssh "sudo -n bash -s -- $WRKGN_PORT" <<'REMOTE'
set -euo pipefail
port=$1
printf '\nService states\n'
systemctl --no-pager --full status wrkgn-web cloudflared-wrkgn || true
printf '\nSelected release and snapshot\n'
readlink -f /opt/wrkgn/current || true
readlink -f /var/lib/wrkgn/current || true
curl --silent --show-error --max-time 5 "http://127.0.0.1:$port/healthz" || true
printf '\nDisk and memory\n'
df -h / /opt/wrkgn /var/lib/wrkgn
free -h
printf '\nRecent service errors\n'
journalctl --no-pager -u wrkgn-web -u cloudflared-wrkgn -p warning -n 40
printf '\nReleases\n'
find /opt/wrkgn/releases -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort
printf '\nSnapshots\n'
find /var/lib/wrkgn/snapshots -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort
printf '\nRecorded hostname purge capability\n'
cat /etc/wrkgn/hostname-purge-status 2>/dev/null || true
REMOTE
