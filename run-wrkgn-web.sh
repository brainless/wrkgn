#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
assets="$repo_root/crates/wrkgn-web/assets"
export_root="$repo_root/local-data/public-export"

usage() {
  cat <<'HELP'
Usage: ./run-wrkgn-web.sh [--install] [-- SERVER-ARGS...]
Export approved data to local-data/public-export, build web assets, validate the
export, then run wrkgn-web locally. Asset dependencies are installed when
node_modules is missing, or always with --install.
Arguments after -- go to wrkgn-web (for example --bind, --public-origin).
Reload a new export without restarting: kill -HUP <wrkgn-web-pid>.
HELP
}
install=false
server_args=()
while (($#)); do
  case $1 in
    --install) install=true; shift ;;
    --) shift; server_args=("$@"); break ;;
    --help|-h) usage; exit 0 ;;
    *) usage >&2; exit 1 ;;
  esac
done

cd "$repo_root"
if $install || [[ ! -d $assets/node_modules ]]; then
  npm ci --prefix "$assets"
fi
npm run build --prefix "$assets"
cargo run --bin wrkgn -- export --export "$export_root"
cargo run -p wrkgn-web -- check "$export_root"
exec cargo run -p wrkgn-web -- --snapshot "$export_root" ${server_args[@]+"${server_args[@]}"}
