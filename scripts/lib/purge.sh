#!/usr/bin/env bash
# Source this file; invoke wrkgn_purge_hosts REPOSITORY_ROOT locally only.
# API: https://developers.cloudflare.com/api/resources/cache/methods/purge/
wrkgn_purge_hosts() {
  # A caller's shell tracing must never print credentials.
  set +x
  WRKGN_PURGE_VERIFIED=0
  local result
  if result=$(python3 - "$1" "$WRKGN_PRODUCTION_HOST" "$WRKGN_STAGING_HOST" <<'PY'
import json, os, pathlib, re, sys, urllib.error, urllib.request

values = {}
dotenv = pathlib.Path(sys.argv[1]) / '.env'
if dotenv.is_file():
    for line in dotenv.read_text().splitlines():
        match = re.fullmatch(r'\s*(?:export\s+)?(CLOUDFLARE_API_TOKEN|CLOUDFLARE_ZONE_ID)\s*=\s*(.*?)\s*', line)
        if not match:
            continue
        key, value = match.groups()
        if value[:1] in ('"', "'"):
            quote = value[0]
            end = value.find(quote, 1)
            if end < 0 or not re.fullmatch(r'\s*(?:#.*)?', value[end+1:]):
                sys.exit('Invalid Cloudflare credential assignment in .env (use literal quoted values).')
            value = value[1:end]
        else:
            value = re.split(r'\s+#', value, 1)[0].strip()
        values[key] = value
token = os.environ.get('CLOUDFLARE_API_TOKEN', values.get('CLOUDFLARE_API_TOKEN', ''))
zone = os.environ.get('CLOUDFLARE_ZONE_ID', values.get('CLOUDFLARE_ZONE_ID', ''))
if not token:
    print('WARNING: no CLOUDFLARE_API_TOKEN; cached pages remain stale until a hostname purge. Run scripts/publish-data.sh --purge-only after configuring credentials.', file=sys.stderr)
    print('skipped')
    sys.exit(0)
if not re.fullmatch(r'[a-fA-F0-9]{32}', zone):
    sys.exit('Set CLOUDFLARE_ZONE_ID to the 32-character zone ID; hostname purge was not sent.')
if '\r' in token or '\n' in token:
    sys.exit('Invalid Cloudflare token format.')
hosts = list(dict.fromkeys(sys.argv[2:]))
if any(not re.fullmatch(r'[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?', h) for h in hosts):
    sys.exit('Invalid purge hostname in deploy/wrkgn.conf.')
request = urllib.request.Request(
    f'https://api.cloudflare.com/client/v4/zones/{zone}/purge_cache',
    data=json.dumps({'hosts': hosts}).encode(), method='POST',
    headers={'Authorization': f'Bearer {token}', 'Content-Type': 'application/json'})
try:
    with urllib.request.urlopen(request, timeout=30) as response:
        payload = json.load(response)
    if payload.get('success') is not True:
        codes = [e.get('code') for e in payload.get('errors', []) if isinstance(e, dict)]
        sys.exit(f'Hostname purge rejected (error codes {codes}). Check zone permission and hostname-purge support; no zone-wide fallback was attempted. Retry --purge-only.')
except urllib.error.HTTPError as error:
    sys.exit(f'Hostname purge rejected (HTTP {error.code}). Check token, zone permission and plan support; no zone-wide fallback was attempted. Retry --purge-only.')
except (urllib.error.URLError, TimeoutError, ValueError, OSError):
    sys.exit('Hostname purge failed: network error or invalid API response. Retry --purge-only. No zone-wide fallback was attempted.')
print('verified')
print('Purged configured production and staging hostnames.', file=sys.stderr)
PY
  ); then
    if [[ $result == verified ]]; then WRKGN_PURGE_VERIFIED=1; fi
  else
    return 1
  fi
}
