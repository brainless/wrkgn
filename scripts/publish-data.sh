#!/usr/bin/env bash
set -euo pipefail
set +x

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=../deploy/wrkgn.conf
source "$repo_root/deploy/wrkgn.conf"
# shellcheck source=lib/purge.sh
source "$repo_root/scripts/lib/purge.sh"

usage() {
  cat <<'HELP'
Usage: scripts/publish-data.sh [--export PATH] [--checker PATH]
       scripts/publish-data.sh --rollback
       scripts/publish-data.sh --purge-only
Publish the approved public SQLite snapshot, verify reload, then purge hostnames.
Defaults: local-data/public-export; WRKGN_WEB_CHECK_BIN or target/release/wrkgn-web.
Cloudflare credentials: CLOUDFLARE_API_TOKEN and CLOUDFLARE_ZONE_ID in environment/.env.
HELP
}
mode=publish
export_root="$repo_root/local-data/public-export"
checker=${WRKGN_WEB_CHECK_BIN:-$repo_root/target/release/wrkgn-web}
while (($#)); do
  case $1 in
    --rollback|--purge-only)
      [[ $mode == publish ]] || { echo 'Choose only one operation.' >&2; exit 1; }
      mode=${1#--}; shift ;;
    --export|--checker)
      (($# >= 2)) || { usage >&2; exit 1; }
      if [[ $1 == --export ]]; then export_root=$2; else checker=$2; fi
      shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 1 ;;
  esac
done
[[ $WRKGN_SSH_HOST =~ ^[A-Za-z0-9][A-Za-z0-9._-]*$ && $WRKGN_PORT =~ ^[0-9]+$ ]] || {
  echo 'Invalid SSH alias or port in deploy/wrkgn.conf.' >&2; exit 1;
}
(( WRKGN_PORT >= 1024 && WRKGN_PORT <= 65535 )) || { echo 'Invalid loopback port.' >&2; exit 1; }
if [[ $mode == purge-only ]]; then
  wrkgn_purge_hosts "$repo_root"
  exit
fi
for command_name in ssh scp python3; do
  command -v "$command_name" >/dev/null || { echo "Missing command: $command_name" >&2; exit 1; }
done
ssh_options=(-o BatchMode=yes -o ConnectTimeout=10 -o PasswordAuthentication=no -o KbdInteractiveAuthentication=no)
remote() { ssh "${ssh_options[@]}" -l pixlie "$WRKGN_SSH_HOST" "$@"; }
scratch=$(mktemp -d "${TMPDIR:-/tmp}/wrkgn-publish.XXXXXX")
upload=
cleanup() {
  local result=$?
  trap - EXIT
  rm -rf -- "$scratch"
  if [[ -n $upload ]]; then remote "rm -rf -- '$upload'" >/dev/null 2>&1 || true; fi
  exit "$result"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP
version=
checksum=
if [[ $mode == publish ]]; then
  [[ -x $checker ]] || {
    echo 'Local wrkgn-web checker missing. Run scripts/deploy.sh first, or build assets then cargo build -p wrkgn-web --release; alternatively pass --checker PATH.' >&2
    exit 1
  }
  # Resolve exactly the schema-6 pointer contract and freeze a private local copy.
  version=$(python3 - "$export_root" "$scratch/public.sqlite" <<'PY'
import json, pathlib, re, shutil, sys
root = pathlib.Path(sys.argv[1]).resolve()
pointer = json.loads((root / 'current.json').read_text())
version = pointer.get('version', '')
if pointer.get('schema_version') != 6 or not isinstance(version, str) or not re.fullmatch('[0-9a-f]{16}', version):
    sys.exit('Unsupported public snapshot pointer.')
if pointer.get('snapshot') != f'snapshots/{version}/manifest.json':
    sys.exit('Unsafe public snapshot pointer.')
directory = root / 'snapshots' / version
manifest = json.loads((directory / 'manifest.json').read_text())
if manifest.get('schema_version') != 6 or manifest.get('version') != version or manifest.get('database') != 'public.sqlite':
    sys.exit('Public manifest does not match selected snapshot.')
source = directory / 'public.sqlite'
if source.resolve().parent != directory.resolve() or not directory.resolve().is_relative_to(root):
    sys.exit('Public snapshot path escapes its export directory.')
shutil.copyfile(source, sys.argv[2])
print(version)
PY
  )
  validation=$("$checker" check "$scratch/public.sqlite")
  [[ $validation == "valid public snapshot schema=6 version=$version" ]] || {
    echo 'Checker result does not match selected public snapshot version.' >&2; exit 1;
  }
  checksum=$(python3 - "$scratch/public.sqlite" <<'PY'
import hashlib, sys
with open(sys.argv[1], 'rb') as source:
    digest = hashlib.sha256()
    for block in iter(lambda: source.read(1024 * 1024), b''):
        digest.update(block)
    print(digest.hexdigest())
PY
  )
  upload=$(remote 'umask 077; mktemp -d /tmp/wrkgn-publish.XXXXXXXX')
  [[ $upload =~ ^/tmp/wrkgn-publish\.[A-Za-z0-9]+$ ]] || { echo 'Invalid remote upload path.' >&2; exit 1; }
  scp "${ssh_options[@]}" "$scratch/public.sqlite" "pixlie@$WRKGN_SSH_HOST:$upload/public.sqlite"
fi

# Values passed below are validated safe identifiers, never credentials.
remote "sudo -n bash -s -- '$mode' '$version' '$checksum' '$upload' '$WRKGN_PORT'" <<'REMOTE'
set -euo pipefail
mode=$1 version=$2 checksum=$3 upload=$4 port=$5
exec 9>/var/lock/wrkgn-operation.lock
flock -w 300 9 || { echo 'Another wrkgn operation holds the lock.' >&2; exit 1; }
base=/var/lib/wrkgn
binary=/opt/wrkgn/current/wrkgn-web
[[ -x $binary && -L $base/current ]] || {
  echo 'Deploy the first release (with its seed snapshot) using scripts/deploy.sh before publishing.' >&2; exit 1;
}
previous=$(readlink -f "$base/current")
[[ $previous =~ ^/var/lib/wrkgn/snapshots/[0-9a-f]{16}$ && -f $previous/public.sqlite ]] || {
  echo 'Existing snapshot selection is unsafe or missing.' >&2; exit 1;
}
previous_version=${previous##*/}
if [[ $mode == rollback ]]; then
  target=$(readlink -f "$base/previous" || true)
  [[ $target =~ ^/var/lib/wrkgn/snapshots/[0-9a-f]{16}$ && -f $target/public.sqlite && $target != "$previous" ]] || {
    echo 'No previous published snapshot is available.' >&2; exit 1;
  }
  version=${target##*/}
else
  target="$base/snapshots/$version"
  [[ $(sha256sum "$upload/public.sqlite" | cut -d ' ' -f1) == "$checksum" ]] || { echo 'Upload checksum mismatch.' >&2; exit 1; }
  if [[ -e $target || -L $target ]]; then
    [[ ! -L $target && -f $target/public.sqlite && $(sha256sum "$target/public.sqlite" | cut -d ' ' -f1) == "$checksum" ]] || {
      echo 'Immutable snapshot version already exists with different bytes or an unsafe path.' >&2; exit 1;
    }
    echo "Snapshot $version already exists; skipping upload installation."
  else
    incoming=$(mktemp -d "$base/snapshots/.incoming-$version.XXXXXXXX")
    install -m 0444 -o root -g root "$upload/public.sqlite" "$incoming/public.sqlite"
    chmod 0755 "$incoming"
    mv -T "$incoming" "$target"
  fi
fi
validation=$(runuser -u wrkgn -- "$binary" check "$target/public.sqlite")
[[ $validation == "valid public snapshot schema=6 version=$version" ]] || { echo 'Server validator rejected snapshot identity.' >&2; exit 1; }
health=
wait_health() {
  local expected=$1 attempt
  for attempt in $(seq 1 30); do
    health=$(curl --silent --show-error --fail --max-time 2 "http://127.0.0.1:$port/healthz" 2>/dev/null) || health=
    if python3 -c 'import json,sys; sys.exit(0 if json.loads(sys.argv[1]).get("snapshot") == sys.argv[2] else 1)' "$health" "$expected" 2>/dev/null; then return 0; fi
    sleep 1
  done
  return 1
}
select_snapshot() {
  local destination=$1 pointer=$2
  ln -s "$destination" "$base/.$pointer.$$"
  mv -Tf "$base/.$pointer.$$" "$base/$pointer"
}
switched=0
committed=0
recover() {
  local status=$?
  trap - EXIT INT TERM HUP
  if (( switched && !committed )); then
    echo "Snapshot switch failed; restoring $previous_version. Last health: $health" >&2
    select_snapshot "$previous" current
    systemctl reload-or-restart wrkgn-web || true
    if ! wait_health "$previous_version"; then echo "Rollback health failed: $health" >&2; fi
    status=1
  fi
  exit "$status"
}
trap recover EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP
switched=1
select_snapshot "$target" current
systemctl reload-or-restart wrkgn-web
wait_health "$version" || { echo "Snapshot health verification failed: $health" >&2; exit 1; }
if [[ $previous != "$target" ]]; then select_snapshot "$previous" previous; fi
committed=1
echo "Verified active public snapshot $version."
# Keep five immutable snapshots, always preserving current and previous.
python3 - "$base" <<'PY'
import pathlib, re, shutil, sys
base = pathlib.Path(sys.argv[1])
protected = {(base / name).resolve() for name in ('current', 'previous') if (base / name).exists()}
snapshots = sorted((p for p in (base / 'snapshots').iterdir() if p.is_dir() and not p.is_symlink() and re.fullmatch('[0-9a-f]{16}', p.name)), key=lambda p: p.stat().st_mtime_ns, reverse=True)
keep = set(protected)
for snapshot in snapshots:
    if len(keep) < 5:
        keep.add(snapshot)
for snapshot in snapshots:
    if snapshot not in keep:
        shutil.rmtree(snapshot)
PY
REMOTE

# The origin switch has been verified. An edge failure keeps the valid origin
# selected; retry the purge independently without another upload or reload.
if ! wrkgn_purge_hosts "$repo_root"; then
  echo 'Snapshot is active, but hostname purge failed. Retry scripts/publish-data.sh --purge-only.' >&2
  exit 1
fi
