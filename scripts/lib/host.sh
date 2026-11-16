#!/usr/bin/env bash
# Sourced by setup/status. No network activity until wrkgn_ssh is called.
WRKGN_REPO_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
# shellcheck source=../../deploy/wrkgn.conf
source "$WRKGN_REPO_ROOT/deploy/wrkgn.conf"
wrkgn_die() { echo "wrkgn: $*" >&2; exit 1; }
[[ $WRKGN_SSH_HOST =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ ]] || wrkgn_die 'Invalid SSH alias.'
[[ $WRKGN_PORT =~ ^[0-9]+$ ]] && ((WRKGN_PORT > 1024 && WRKGN_PORT <= 65535)) || wrkgn_die 'Port must be 1025..65535.'
for wrkgn_hostname in "$WRKGN_PRODUCTION_HOST" "$WRKGN_STAGING_HOST"; do
  [[ $wrkgn_hostname =~ ^[a-z0-9][a-z0-9.-]*[a-z0-9]$ ]] || wrkgn_die 'Invalid hostname.'
done
[[ $WRKGN_TUNNEL_NAME =~ ^[a-z0-9][a-z0-9-]*$ ]] || wrkgn_die 'Invalid tunnel name.'
WRKGN_SSH_OPTIONS=(-o BatchMode=yes -o ConnectTimeout=10 -o PasswordAuthentication=no -o KbdInteractiveAuthentication=no)
wrkgn_ssh() { ssh "${WRKGN_SSH_OPTIONS[@]}" -l pixlie "$WRKGN_SSH_HOST" "$@"; }
wrkgn_check_ssh_alias() {
  local resolved
  resolved=$(ssh -G "$WRKGN_SSH_HOST" 2>/dev/null | awk '$1 == "hostname" {print $2; exit}')
  [[ -n $resolved && $resolved != "$WRKGN_SSH_HOST" ]] || wrkgn_die "Configure Host $WRKGN_SSH_HOST with a VPS HostName in ~/.ssh/config first."
  [[ $(wrkgn_ssh 'id -un') == pixlie ]] && wrkgn_ssh 'sudo -n true' || wrkgn_die 'Existing pixlie key-only SSH and passwordless sudo are required.'
}
