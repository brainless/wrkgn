#!/usr/bin/env bash
# Build on the maintainer's Mac; only the finalized Linux binary/public snapshot leave it.
set -euo pipefail
repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
# shellcheck source=../deploy/wrkgn.conf
source "$repo_root/deploy/wrkgn.conf"
allow_dirty=false
rollback=false
purge=false
export_path="$repo_root/local-data/public-export"
usage() {
  echo 'Usage: scripts/deploy.sh [--allow-dirty] [--export PATH] [--purge]'
  echo '       scripts/deploy.sh --rollback [--purge]'
}
while (($#)); do
  case $1 in
    --allow-dirty) allow_dirty=true; shift ;;
    --rollback) rollback=true; shift ;;
    --purge) purge=true; shift ;;
    --export) [[ $# -ge 2 ]] || { usage >&2; exit 1; }; export_path=$2; shift 2 ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 1 ;;
  esac
done
[[ $WRKGN_SSH_HOST =~ ^[a-zA-Z0-9][a-zA-Z0-9._-]*$ ]] || { echo 'Invalid SSH alias.' >&2; exit 1; }
[[ $WRKGN_PORT =~ ^[0-9]+$ ]] && ((WRKGN_PORT >= 1024 && WRKGN_PORT <= 65535)) || { echo 'Invalid loopback port.' >&2; exit 1; }
ssh_options=(-o BatchMode=yes -o ConnectTimeout=10 -o ServerAliveInterval=15 -o ServerAliveCountMax=3 -l pixlie)
remote() { ssh "${ssh_options[@]}" "$WRKGN_SSH_HOST" "$@"; }
if $rollback; then
  $allow_dirty && { echo '--allow-dirty does not apply to rollback.' >&2; exit 1; }
  remote "sudo -n bash -s -- rollback '$WRKGN_PORT'" < "$repo_root/scripts/lib/deploy-remote.sh"
else
  cd "$repo_root"
  commit=$(git rev-parse HEAD)
  dirty_suffix=
  if [[ -n $(git status --porcelain) ]]; then
    $allow_dirty || { echo 'Working tree is dirty; commit changes or explicitly pass --allow-dirty.' >&2; exit 1; }
    dirty_suffix=-dirty
  fi
  release_id="$(date -u +%Y%m%dT%H%M%SZ)-${commit:0:12}${dirty_suffix}-$$"
  for tool in npm cargo python3 shasum scp; do
    command -v "$tool" >/dev/null || { echo "Missing local tool: $tool" >&2; exit 1; }
  done
  cargo zigbuild --version >/dev/null || { echo 'Install cargo-zigbuild and Zig; see docs/hosting.md.' >&2; exit 1; }
  # Probe the actual host, rather than assuming its architecture or Debian release.
  host_info=$(remote "bash -s" <<'PROBE'
set -euo pipefail
. /etc/os-release
[[ $ID == debian ]] || { echo 'Deployment requires Debian.' >&2; exit 1; }
printf '%s %s %s\n' "$(uname -m)" "$VERSION_ID" "$(getconf GNU_LIBC_VERSION | awk '{print $2}')"
PROBE
  )
  read -r architecture debian_version host_glibc <<< "$host_info"
  case $architecture in
    x86_64) target=x86_64-unknown-linux-gnu ;;
    aarch64|arm64) target=aarch64-unknown-linux-gnu ;;
    *) echo "Unsupported VPS architecture: $architecture" >&2; exit 1 ;;
  esac
  [[ $host_glibc =~ ^[0-9]+\.[0-9]+$ ]] || { echo 'Cannot determine VPS glibc.' >&2; exit 1; }
  # 2.31 is supported by Zig and is compatible with Debian 11 and newer.
  glibc=2.31
  python3 - "$host_glibc" "$glibc" <<'GLIBC'
import sys
if tuple(map(int, sys.argv[1].split('.'))) < tuple(map(int, sys.argv[2].split('.'))):
    raise SystemExit('VPS glibc predates supported deployment baseline 2.31')
GLIBC
  echo "Building $release_id for Debian $debian_version / $target / glibc $glibc (host $host_glibc)."
  npm ci --prefix crates/wrkgn-web/assets
  npm run build --prefix crates/wrkgn-web/assets
  # A native binary validates locally; never try to execute the Linux build on macOS.
  env -u CARGO_BUILD_TARGET WRKGN_BUILD_ID="$release_id" CARGO_PROFILE_RELEASE_CODEGEN_BACKEND=llvm \
    cargo build --locked --release -p wrkgn-web --bin wrkgn-web --target-dir "$repo_root/target"
  "$repo_root/target/release/wrkgn-web" check "$export_path"
  snapshot_info=$(python3 - "$export_path" <<'SNAPSHOT'
import json, pathlib, re, sys
root = pathlib.Path(sys.argv[1]).resolve()
pointer = json.loads((root / 'current.json').read_text())
version = pointer['version']
if not isinstance(version, str) or not re.fullmatch(r'[a-f0-9]{16}', version):
    raise SystemExit('Invalid snapshot version')
if pointer['snapshot'] != f'snapshots/{version}/manifest.json':
    raise SystemExit('Invalid snapshot pointer')
database = (root / 'snapshots' / version / 'public.sqlite').resolve()
if root not in database.parents or not database.is_file():
    raise SystemExit('Snapshot database escapes export or is missing')
print(version)
print(database)
SNAPSHOT
  )
  snapshot_version=${snapshot_info%%$'\n'*}
  snapshot_file=${snapshot_info#*$'\n'}
  "$repo_root/target/release/wrkgn-web" check "$snapshot_file"
  env WRKGN_BUILD_ID="$release_id" CARGO_PROFILE_RELEASE_CODEGEN_BACKEND=llvm \
    cargo zigbuild --locked --release -p wrkgn-web --bin wrkgn-web \
    --target "$target.$glibc" --target-dir "$repo_root/target"
  binary="$repo_root/target/$target/release/wrkgn-web"
  binary_sha=$(shasum -a 256 "$binary" | awk '{print $1}')
  snapshot_sha=$(shasum -a 256 "$snapshot_file" | awk '{print $1}')
  upload_dir=$(remote 'mktemp -d /tmp/wrkgn-deploy.XXXXXXXX')
  [[ $upload_dir =~ ^/tmp/wrkgn-deploy\.[a-zA-Z0-9]+$ ]] || { echo 'Invalid remote upload directory.' >&2; exit 1; }
  cleanup() { remote "rm -rf -- '$upload_dir'" >/dev/null 2>&1 || true; }
  trap cleanup EXIT
  scp -q -o BatchMode=yes "$binary" "pixlie@$WRKGN_SSH_HOST:$upload_dir/wrkgn-web"
  scp -q -o BatchMode=yes "$snapshot_file" "pixlie@$WRKGN_SSH_HOST:$upload_dir/public.sqlite"
  remote "sudo -n bash -s -- deploy '$WRKGN_PORT' '$release_id' '$upload_dir' '$binary_sha' '$commit' '$snapshot_version' '$snapshot_sha'" \
    < "$repo_root/scripts/lib/deploy-remote.sh"
fi
if $purge; then
  # shellcheck source=lib/purge.sh
  source "$repo_root/scripts/lib/purge.sh"
  wrkgn_purge_hosts "$repo_root"
fi
