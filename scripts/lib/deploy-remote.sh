#!/usr/bin/env bash
# Run only via deploy.sh on the Debian host with sudo; shares publication's lock.
set -Eeuo pipefail
mode=$1
port=$2
[[ $mode == deploy || $mode == rollback ]] && [[ $port =~ ^[0-9]+$ ]] || exit 1
exec 9>/var/lock/wrkgn-operation.lock
flock -w 120 9 || { echo 'Another wrkgn operation holds the lock.' >&2; exit 1; }
base=/opt/wrkgn
data=/var/lib/wrkgn
[[ -d $base/releases && -d $data/snapshots ]] || { echo 'Run scripts/setup-vps.sh first.' >&2; exit 1; }
old=$(readlink "$base/current" || true)
old_snapshot=$(readlink "$data/current" || true)
old_snapshot=${old_snapshot#"$data/"}
[[ -z $old || $old =~ ^releases/[a-zA-Z0-9._-]+$ ]] || { echo 'Unexpected current release pointer.' >&2; exit 1; }
[[ -z $old_snapshot || $old_snapshot =~ ^snapshots/[a-f0-9]{16}$ ]] || { echo 'Unexpected snapshot pointer.' >&2; exit 1; }
switched=false
seeded=false
health='No health response received.'
atomic_pointer() {
  local pointer=$1 destination=$2
  ln -s "$destination" "$pointer.next.$$"
  mv -Tf "$pointer.next.$$" "$pointer"
}
wait_healthy() {
  local expected_build=$1 expected_version=$2 attempt
  for ((attempt=0; attempt<30; attempt++)); do
    health=$(curl --silent --show-error --fail --connect-timeout 1 --max-time 2 "http://127.0.0.1:$port/healthz" 2>&1) || true
    if python3 - "$expected_build" "$expected_version" "$health" <<'HEALTH'
import json, sys
try:
    value = json.loads(sys.argv[3])
    good = value.get('build') == sys.argv[1] and value.get('snapshot') == sys.argv[2]
except (ValueError, AttributeError):
    good = False
sys.exit(0 if good else 1)
HEALTH
    then return 0; fi
    sleep 1
  done
  return 1
}
recover() {
  local status=$?
  trap - ERR INT TERM HUP
  if $switched; then
    echo "Release activation failed; restoring previous release. Last health: $health" >&2
    if [[ -n $old ]]; then
      atomic_pointer "$base/current" "$old"
      if systemctl restart wrkgn-web.service && wait_healthy "${old#releases/}" "${old_snapshot#snapshots/}"; then
        echo 'Previous release restored and healthy.' >&2
      else
        echo "Previous release selected, but recovery health verification failed: $health" >&2
      fi
    else
      systemctl stop wrkgn-web.service || true
      rm -f "$base/current"
    fi
    if $seeded && [[ -z $old_snapshot ]]; then rm -f "$data/current"; fi
  fi
  rm -f "$base/current.next.$$" "$base/previous.next.$$" "$data/current.next.$$"
  ((status != 0)) || status=1
  exit "$status"
}
trap recover ERR INT TERM HUP
if [[ $mode == rollback ]]; then
  destination=$(readlink "$base/previous" || true)
  [[ $destination =~ ^releases/[a-zA-Z0-9._-]+$ && $destination != "$old" ]] || { echo 'No previous release is available.' >&2; exit 1; }
  release_id=${destination#releases/}
  [[ -x $base/$destination/wrkgn-web ]] || { echo 'Previous binary is missing.' >&2; exit 1; }
  "$base/$destination/wrkgn-web" check "$data/current/public.sqlite"
else
  release_id=$3 upload=$4 binary_sha=$5 commit=$6 snapshot_version=$7 snapshot_sha=$8
  [[ $release_id =~ ^[a-zA-Z0-9._-]+$ && $upload =~ ^/tmp/wrkgn-deploy\.[a-zA-Z0-9]+$ ]] || exit 1
  [[ $binary_sha =~ ^[a-f0-9]{64}$ && $snapshot_sha =~ ^[a-f0-9]{64}$ && $commit =~ ^[a-f0-9]{40}$ && $snapshot_version =~ ^[a-f0-9]{16}$ ]] || exit 1
  destination="releases/$release_id"
  [[ ! -e $base/$destination ]] || { echo 'Release already exists; refusing overwrite.' >&2; exit 1; }
  temporary="$base/releases/.incoming-$release_id"
  [[ ! -e $temporary ]] || { echo 'Stale incoming release exists; inspect it before retrying.' >&2; exit 1; }
  install -d -m 755 "$temporary"
  install -m 755 -o root -g root "$upload/wrkgn-web" "$temporary/wrkgn-web"
  printf '%s  %s\n' "$binary_sha" "$temporary/wrkgn-web" | sha256sum --check --status
  printf '%s\n' "$commit" > "$temporary/git-commit"
  printf '%s\n' "$release_id" > "$temporary/build-id"
  printf '%s\n' "$binary_sha" > "$temporary/sha256"
  if [[ -z $old_snapshot ]]; then
    # Bootstrap only: subsequent deploys do not publish or alter snapshots.
    snapshot_dir="$data/snapshots/$snapshot_version"
    if [[ ! -e $snapshot_dir ]]; then
      incoming_snapshot="$data/snapshots/.incoming-deploy-$release_id"
      install -d -m 755 "$incoming_snapshot"
      install -m 444 -o root -g root "$upload/public.sqlite" "$incoming_snapshot/public.sqlite"
      printf '%s  %s\n' "$snapshot_sha" "$incoming_snapshot/public.sqlite" | sha256sum --check --status
      "$temporary/wrkgn-web" check "$incoming_snapshot/public.sqlite"
      mv -T "$incoming_snapshot" "$snapshot_dir"
    else
      printf '%s  %s\n' "$snapshot_sha" "$snapshot_dir/public.sqlite" | sha256sum --check --status
      "$temporary/wrkgn-web" check "$snapshot_dir/public.sqlite"
    fi
    seeded=true
    # Mark a transaction before changing either selected pointer.
    switched=true
    atomic_pointer "$data/current" "snapshots/$snapshot_version"
    echo "Seeded initial public snapshot $snapshot_version."
  else
    "$temporary/wrkgn-web" check "$data/current/public.sqlite"
  fi
  mv -T "$temporary" "$base/$destination"
fi
expected_snapshot=$(basename "$(readlink "$data/current")")
switched=true
atomic_pointer "$base/current" "$destination"
systemctl restart wrkgn-web.service
wait_healthy "$release_id" "$expected_snapshot" || { echo "Health verification failed: $health" >&2; false; }
if [[ -n $old && $old != "$destination" ]]; then atomic_pointer "$base/previous" "$old"; fi
switched=false
trap - ERR INT TERM HUP
echo "Healthy release $release_id; snapshot $expected_snapshot."
# Keep five, always preserving the selected and rollback targets. Release IDs sort by UTC creation time.
current=$(readlink "$base/current")
previous=$(readlink "$base/previous" || true)
extra_slots=4
if [[ -n $previous && $previous != "$current" ]]; then extra_slots=3; fi
kept=0
while IFS= read -r name; do
  [[ $name =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]+$ ]] || continue
  [[ -d $base/releases/$name ]] || continue
  if [[ releases/$name == "$current" || releases/$name == "$previous" ]]; then continue; fi
  kept=$((kept + 1))
  if ((kept > extra_slots)); then rm -rf -- "$base/releases/$name"; fi
done < <(find "$base/releases" -mindepth 1 -maxdepth 1 -type d -printf '%f\n' | sort -r)
