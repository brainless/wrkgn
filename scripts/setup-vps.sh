#!/usr/bin/env bash
# Run locally; shared-host adaptation of promptogether/scripts/setup-vps.sh.
set -euo pipefail
# shellcheck source=lib/host.sh
source "$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)/lib/host.sh"
credentials=
while (($#)); do
  case "$1" in
    --credentials) (($# >= 2)) || wrkgn_die 'Missing --credentials path'; credentials=$2; shift 2 ;;
    --help|-h) echo 'Usage: scripts/setup-vps.sh --credentials ~/.cloudflared/<tunnel-uuid>.json'; exit 0 ;;
    *) wrkgn_die "Unknown argument: $1" ;;
  esac
done
[[ -n $credentials && -r $credentials ]] || wrkgn_die 'Supply the locally created tunnel credentials JSON with --credentials PATH.'
command -v python3 >/dev/null || wrkgn_die 'python3 is required locally.'
tunnel_id=$(python3 - "$credentials" <<'PY'
import json, re, sys
try:
    data = json.load(open(sys.argv[1]))
    tunnel = data['TunnelID']
    assert re.fullmatch(r'[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}', tunnel)
    assert isinstance(data['TunnelSecret'], str) and data['TunnelSecret']
    assert isinstance(data['AccountTag'], str) and data['AccountTag']
    print(tunnel)
except (OSError, ValueError, KeyError, AssertionError, TypeError):
    sys.exit('Invalid locally managed tunnel credentials JSON (contents withheld).')
PY
)
wrkgn_check_ssh_alias
ssh_port=$(ssh -G "$WRKGN_SSH_HOST" 2>/dev/null | awk '$1 == "port" {print $2; exit}')
[[ $ssh_port =~ ^[0-9]+$ ]] && ((ssh_port > 0 && ssh_port <= 65535)) || wrkgn_die 'Invalid SSH port.'

# Fail before changing packages or uploading secrets on an unexpected host.
wrkgn_ssh 'sudo -n bash -s' <<'PREFLIGHT'
set -euo pipefail
[[ $(. /etc/os-release; printf '%s' "$ID") == debian ]] || { echo 'Debian is required.' >&2; exit 1; }
id pixlie >/dev/null || { echo 'Existing pixlie account is required.' >&2; exit 1; }
PREFLIGHT

# This is also an explicit real-zone capability probe. Never send the token to SSH.
purge_status="unverified; run scripts/publish-data.sh --purge-only with local Cloudflare credentials"
if [[ -f $WRKGN_REPO_ROOT/scripts/lib/purge.sh ]]; then
  # shellcheck source=lib/purge.sh
  source "$WRKGN_REPO_ROOT/scripts/lib/purge.sh"
  wrkgn_purge_hosts "$WRKGN_REPO_ROOT"
  if [[ ${WRKGN_PURGE_VERIFIED:-0} == 1 ]]; then
    purge_status="verified by successful hostname purge at $(date -u +%Y-%m-%dT%H:%M:%SZ)"
  fi
fi

work_dir=$(mktemp -d)
remote_dir=
cleanup() {
  rm -rf -- "$work_dir"
  if [[ -n $remote_dir ]]; then wrkgn_ssh "rm -rf -- '$remote_dir'" >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT
chmod 700 "$work_dir"
python3 - "$WRKGN_REPO_ROOT" "$work_dir" "$tunnel_id" "$WRKGN_PORT" "$WRKGN_PRODUCTION_HOST" "$WRKGN_STAGING_HOST" "$purge_status" <<'PY'
from pathlib import Path
import sys
root, out, tunnel, port, production, staging, purge = sys.argv[1:]
root, out = Path(root), Path(out)
values = {'@TUNNEL_ID@':tunnel, '@PORT@':port, '@PRODUCTION_HOST@':production, '@STAGING_HOST@':staging}
text = (root/'deploy/cloudflared/config.yml.in').read_text()
for key, value in values.items(): text = text.replace(key, value)
(out/'config.yml').write_text(text)
for name in ('wrkgn-web', 'cloudflared-wrkgn'):
    (out/(name+'.service')).write_text((root/('deploy/systemd/'+name+'.service.in')).read_text())
(out/'web.env').write_text(f'WRKGN_WEB_BIND=127.0.0.1:{port}\nWRKGN_WEB_SNAPSHOT=/var/lib/wrkgn/current/public.sqlite\nWRKGN_WEB_PUBLIC_ORIGIN=https://{production}\n')
(out/'hostname-purge-status').write_text(purge+'\n')
PY
cp -- "$credentials" "$work_dir/credentials.json"
chmod 600 "$work_dir/credentials.json"
remote_dir=$(wrkgn_ssh 'umask 077; mktemp -d /tmp/wrkgn-setup.XXXXXXXX')
[[ $remote_dir =~ ^/tmp/wrkgn-setup\.[A-Za-z0-9]+$ ]] || wrkgn_die 'Unexpected remote temporary directory.'
scp "${WRKGN_SSH_OPTIONS[@]}" "$work_dir/"* "pixlie@$WRKGN_SSH_HOST:$remote_dir/" >/dev/null

wrkgn_ssh "sudo -n bash -s -- '$remote_dir' '$WRKGN_PORT' '$ssh_port'" <<'REMOTE_SETUP'
set -euo pipefail
stage=$1
port=$2
ssh_port=$3
exec 9>/var/lock/wrkgn-operation.lock
flock -n 9 || { echo 'Another wrkgn operation is running.' >&2; exit 1; }
export DEBIAN_FRONTEND=noninteractive
apt-get update
apt-get install -y ca-certificates curl gnupg ufw unattended-upgrades python3 util-linux iproute2
completed=false
if [[ -e /var/lib/wrkgn-vps/bootstrap-complete || -e /var/lib/promptogether-vps/bootstrap-complete ]]; then completed=true; fi
# No firewall resets, policy changes, HTTP rules, Nginx or certificate operations.
ufw allow "$ssh_port/tcp"
if [[ $completed == true ]]; then
  ufw status | grep -Fq 'Status: active' || { echo 'Existing bootstrap has inactive UFW; repair it deliberately.' >&2; exit 1; }
else
  ufw --force enable
  cat > /etc/apt/apt.conf.d/20wrkgn-auto-upgrades <<'APT'
APT::Periodic::Update-Package-Lists "1";
APT::Periodic::Unattended-Upgrade "1";
APT
fi
REMOTE_SETUP
[[ $(wrkgn_ssh 'id -un') == pixlie ]] && wrkgn_ssh 'sudo -n true' || wrkgn_die 'Fresh login failed after firewall setup; SSH configuration was not changed.'
wrkgn_ssh "sudo -n bash -s -- '$remote_dir' '$WRKGN_PORT'" <<'REMOTE_SETUP'
set -euo pipefail
stage=$1
port=$2
exec 9>/var/lock/wrkgn-operation.lock
flock -n 9 || { echo 'Another wrkgn operation is running.' >&2; exit 1; }
export DEBIAN_FRONTEND=noninteractive
completed=false
if [[ -e /var/lib/wrkgn-vps/bootstrap-complete || -e /var/lib/promptogether-vps/bootstrap-complete ]]; then completed=true; fi
# Verify existing hardening rather than overwriting the other project's config.
if [[ $completed == false ]]; then
  ssh_config=/etc/ssh/sshd_config.d/000-wrkgn-hardening.conf
  had_ssh_config=false
  if [[ -e $ssh_config ]]; then cp -p "$ssh_config" "$stage/ssh-backup"; had_ssh_config=true; fi
  restore_ssh() {
    if [[ $had_ssh_config == true ]]; then cp -p "$stage/ssh-backup" "$ssh_config"; else rm -f "$ssh_config"; fi
    sshd -t && systemctl reload ssh || true
  }
  trap restore_ssh ERR
  cat > /etc/ssh/sshd_config.d/000-wrkgn-hardening.conf <<'SSHD'
PermitRootLogin no
PubkeyAuthentication yes
PasswordAuthentication no
KbdInteractiveAuthentication no
PermitEmptyPasswords no
X11Forwarding no
MaxAuthTries 3
SSHD
  chmod 644 /etc/ssh/sshd_config.d/000-wrkgn-hardening.conf
fi
sshd -t
for user in root pixlie; do
  effective=$(sshd -T -C "user=$user,host=localhost,addr=127.0.0.1")
  grep -Fxq 'passwordauthentication no' <<< "$effective"
  grep -Fxq 'kbdinteractiveauthentication no' <<< "$effective"
  grep -Fxq 'pubkeyauthentication yes' <<< "$effective"
done
grep -Fxq 'permitrootlogin no' <<< "$(sshd -T -C 'user=root,host=localhost,addr=127.0.0.1')"
if [[ $completed == false ]]; then systemctl reload ssh; fi
# A completed bootstrap must retain working security update configuration.
apt-config dump | grep -Fq 'APT::Periodic::Unattended-Upgrade "1"' || { echo 'Unattended upgrades must be enabled.' >&2; exit 1; }

trap - ERR
REMOTE_SETUP
[[ $(wrkgn_ssh 'id -un') == pixlie ]] && wrkgn_ssh 'sudo -n true' || wrkgn_die 'Fresh login failed after SSH verification.'
wrkgn_ssh "sudo -n bash -s -- '$remote_dir' '$WRKGN_PORT'" <<'REMOTE_SETUP'
set -euo pipefail
stage=$1
port=$2
exec 9>/var/lock/wrkgn-operation.lock
flock -n 9 || { echo 'Another wrkgn operation is running.' >&2; exit 1; }
export DEBIAN_FRONTEND=noninteractive
# Refuse a foreign listener, but permit this service on idempotent reruns.
main_pid=$(systemctl show wrkgn-web -p MainPID --value 2>/dev/null || true)
while IFS= read -r listener; do
  [[ -z $listener ]] && continue
  [[ $main_pid =~ ^[1-9][0-9]*$ && $listener == *"pid=$main_pid,"* && $listener == *'127.0.0.1:'* ]] || {
    echo "Configured wrkgn port $port is already occupied by another listener." >&2; exit 1;
  }
done < <(ss -H -ltnp "sport = :$port")
for user in wrkgn wrkgn-tunnel; do
  if ! id "$user" >/dev/null 2>&1; then useradd --system --user-group --no-create-home --shell /usr/sbin/nologin "$user"; fi
  [[ $(getent passwd "$user" | cut -d: -f7) == /usr/sbin/nologin ]] || { echo "Unexpected login shell for $user" >&2; exit 1; }
done
install -d -m 755 /opt/wrkgn/releases /var/lib/wrkgn/snapshots /etc/wrkgn /var/lib/wrkgn-vps
install -d -m 700 -o wrkgn-tunnel -g wrkgn-tunnel /etc/cloudflared/wrkgn
install -m 600 -o wrkgn-tunnel -g wrkgn-tunnel "$stage/credentials.json" /etc/cloudflared/wrkgn/credentials.json
install -m 600 -o wrkgn-tunnel -g wrkgn-tunnel "$stage/config.yml" /etc/cloudflared/wrkgn/config.yml
install -m 644 "$stage/web.env" /etc/wrkgn/web.env
install -m 644 "$stage/hostname-purge-status" /etc/wrkgn/hostname-purge-status
install -m 644 "$stage/wrkgn-web.service" /etc/systemd/system/wrkgn-web.service
install -m 644 "$stage/cloudflared-wrkgn.service" /etc/systemd/system/cloudflared-wrkgn.service

# Official signed apt repository: https://pkg.cloudflare.com/
# Use a project-owned source/key file, avoiding modification of another project's files.
install -d -m 755 /usr/share/keyrings
curl --fail --silent --show-error --location https://pkg.cloudflare.com/cloudflare-main.gpg -o /usr/share/keyrings/wrkgn-cloudflare-main.gpg
chmod 644 /usr/share/keyrings/wrkgn-cloudflare-main.gpg
if ! grep -RqsE '^deb .*https://pkg\.cloudflare\.com/cloudflared (any|bookworm|trixie) main|^URIs: https://pkg\.cloudflare\.com/cloudflared' /etc/apt/sources.list /etc/apt/sources.list.d; then
  printf '%s\n' 'deb [signed-by=/usr/share/keyrings/wrkgn-cloudflare-main.gpg] https://pkg.cloudflare.com/cloudflared any main' > /etc/apt/sources.list.d/wrkgn-cloudflared.list
fi
apt-get update
apt-get install -y cloudflared
# Include only this signed origin in unattended upgrades, preserving existing origins.
cat > /etc/apt/apt.conf.d/52wrkgn-cloudflared-upgrades <<'APT'
Unattended-Upgrade::Origins-Pattern { "site=pkg.cloudflare.com"; };
APT
runuser -u wrkgn-tunnel -- /usr/bin/cloudflared tunnel --config /etc/cloudflared/wrkgn/config.yml ingress validate

# Bound journald with a drop-in. Minima across existing declarations preserve stricter
# retention values even if an administrator has layered several configurations.
systemd-analyze cat-config systemd/journald.conf > "$stage/journald-effective.conf"
python3 - "$stage/journald-effective.conf" <<'PY'
import pathlib, re, sys
text = pathlib.Path(sys.argv[1]).read_text()
limits = {'SystemMaxUse':268435456, 'RuntimeMaxUse':67108864, 'MaxRetentionSec':604800}
units = {'':1, 'K':1024, 'M':1024**2, 'G':1024**3, 'T':1024**4,
         's':1, 'sec':1, 'min':60, 'h':3600, 'hour':3600, 'd':86400, 'day':86400, 'week':604800}
lines=['[Journal]']
for key, cap in limits.items():
    values=[]
    for value in re.findall(r'^\s*'+key+r'\s*=\s*([^#\n]*)', text, re.M):
        value=value.strip()
        if not value: continue
        match=re.fullmatch(r'([0-9]+)\s*([A-Za-z]*)', value)
        if not match or match[2] not in units:
            print(f'Leaving existing {key} unchanged: nontrivial value.', file=sys.stderr)
            break
        number=int(match[1])*units[match[2]]
        if number: values.append(number)
    else:
        lines.append(f'{key}={min([cap]+values)}'+('s' if key=='MaxRetentionSec' else ''))
path=pathlib.Path('/etc/systemd/journald.conf.d/99-wrkgn-retention.conf')
path.parent.mkdir(mode=0o755, parents=True, exist_ok=True)
path.write_text('\n'.join(lines)+'\n')
PY
systemctl restart systemd-journald
systemctl daemon-reload
systemctl enable wrkgn-web cloudflared-wrkgn
systemctl restart cloudflared-wrkgn
if [[ -x /opt/wrkgn/current/wrkgn-web && -r /var/lib/wrkgn/current/public.sqlite ]]; then
  systemctl restart wrkgn-web
else
  echo 'Web service enabled; first deploy supplies the release and snapshot before startup.'
fi
trap - ERR
REMOTE_SETUP

# New SSH processes verify both firewall and SSH changes before recording bootstrap.
[[ $(wrkgn_ssh 'id -un') == pixlie ]] && wrkgn_ssh 'sudo -n true' || wrkgn_die 'Fresh SSH login failed after setup.'
wrkgn_ssh 'sudo -n touch /var/lib/wrkgn-vps/bootstrap-complete'
wrkgn_ssh 'sudo -n bash -s' <<'SUMMARY'
set -euo pipefail
printf '\nArchitecture: '; dpkg --print-architecture
printf 'Debian: '; cat /etc/debian_version
printf 'glibc: '; getconf GNU_LIBC_VERSION
printf '\nServices\n'
systemctl --no-pager --full status wrkgn-web cloudflared-wrkgn || true
printf '\nDefault cloudflared unit (never enabled by wrkgn)\n'
systemctl is-active cloudflared || true
printf '\nListening sockets\n'; ss -ltnp
printf '\nFirewall\n'; ufw status verbose
printf '\nHostname purge capability\n'; cat /etc/wrkgn/hostname-purge-status
SUMMARY
printf '\nSetup complete. Run scripts/deploy.sh, then scripts/publish-data.sh; test staging before DNS cutover.\n'
