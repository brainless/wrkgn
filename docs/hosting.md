# VPS hosting and Cloudflare edge

Epic 06 tooling is implemented. The maintainer completed setup, staging checks and the production DNS cutover on 2026-11-18; results are in [the implementation record](implementation/epic06.md). The legacy Astro site and its Cloudflare Pages deployment are retired; the Rust server is the only production path. Rollback is by `scripts/deploy.sh --rollback` (release) and `scripts/publish-data.sh --rollback` (data). An UptimeRobot monitor checks production health.

## Shared VPS resources

The host is Debian with 2 vCPU and 6 GB RAM. Prompt Together already supplies `pixlie` key-only SSH, passwordless sudo, unattended security updates, Nginx, and UFW rules for SSH/80/443. wrkgn never changes its Nginx sites/certificates, resets UFW, or removes its users/rules. wrkgn uses outbound Cloudflare Tunnel connections and opens no public HTTP port.

| Project | Ports | Units | Users | Tunnel | Directories |
| --- | --- | --- | --- | --- | --- |
| Prompt Together | Public 80/443; existing SSH port | Existing Nginx/SSH/security services | `pixlie` admin; existing service users | Existing hosting unchanged | Existing `/var/www/promptogether.com`, Nginx and certificate configuration |
| wrkgn | Reserved `127.0.0.1:3000` | `wrkgn-web`, `cloudflared-wrkgn` | `wrkgn`, `wrkgn-tunnel` (non-login) | Locally managed `wrkgn` | `/opt/wrkgn`, `/var/lib/wrkgn`, `/etc/wrkgn`, `/etc/cloudflared/wrkgn`, `/var/lib/wrkgn-vps` |

`deploy/wrkgn.conf` is the committed non-secret source for SSH alias, loopback port, hostnames and tunnel name. Record port changes here too. `/opt/wrkgn/releases/<release-id>/wrkgn-web` holds immutable binaries; `current` and `previous` under `/opt/wrkgn` select releases. `/var/lib/wrkgn/snapshots/<version>/public.sqlite` holds immutable public databases; `current` and `previous` under `/var/lib/wrkgn` select directories. Five versions of each are retained, protecting selected versions. `/etc/wrkgn/web.env` sets the loopback bind, snapshot file and production canonical origin. The shared operation lock is `/var/lock/wrkgn-operation.lock`.

The tunnel directory is owned by `wrkgn-tunnel`, mode 0700; its credentials are mode 0600. Setup renders `deploy/cloudflared/config.yml.in` with only the two configured hostnames, forwarding to loopback, and a final 404 ingress. Never use `cloudflared service install`, `/etc/cloudflared/config.yml`, or the default `cloudflared.service` for wrkgn. An unrelated project may already use the default unit; leave it alone.

Both services restart on failure and use systemd hardening, including no new privileges, strict filesystem protection, protected homes and private temporary directories. Web reload sends SIGHUP. The provisional limits are web 1 GiB/75% CPU and tunnel 256 MiB/25% CPU. These have **not** been validated against the full snapshot or concurrent reload peak. Run the [memory/performance tools](development.md#maintainer-performance-commands), record actual one/two-snapshot and reload peaks, and adjust with headroom before production. On this 2-vCPU host, systemd 100% CPU quota means one CPU. Journald retention is bounded by a drop-in, preserving stricter existing declarations where parsed: 256 MiB persistent, 64 MiB runtime, seven days.

## One-time setup

Run locally on the Apple Silicon Mac. Required local tools: Bash, SSH/SCP, Python 3, Node >=22.12, the pinned Rust toolchain, Zig, cargo-zigbuild, and cloudflared for one-time tunnel management. Install Zig/cloudflared with the maintainer's package manager, then install cargo-zigbuild (`cargo install --locked cargo-zigbuild`). Add the detected VPS target to the pinned Rust toolchain: `rustup target add x86_64-unknown-linux-gnu` for x86_64 or `rustup target add aarch64-unknown-linux-gnu` for arm64. The deploy script detects architecture, Debian version and glibc; it pins glibc 2.31 and rejects older hosts. Release builds explicitly use LLVM; dev/test keep Cranelift. Refer to [cargo-zigbuild](https://github.com/rust-cross/cargo-zigbuild) for the build prerequisites.

1. Add `Host wrkgn` in `~/.ssh/config`, using the existing VPS HostName, SSH port and key. The alias must resolve to an explicit HostName. Check `ssh -l pixlie wrkgn 'id -un'` and `ssh -l pixlie wrkgn 'sudo -n true'`. Setup requires Debian and the existing `pixlie` account; it will not create a replacement admin.
2. On the Mac, create a locally managed tunnel and route staging only:

   ```sh
   cloudflared tunnel login
   cloudflared tunnel create wrkgn
   cloudflared tunnel route dns wrkgn next.wrkgn.com
   ```

   The credentials file is `~/.cloudflared/<tunnel-uuid>.json`. Keep it and `cert.pem` outside the repository. Setup reads the UUID from that file. The certificate permits tunnel management and stays local; only tunnel-specific credentials are uploaded. See [Cloudflare's locally managed tunnel guide](https://developers.cloudflare.com/cloudflare-one/networks/connectors/cloudflare-tunnel/do-more-with-tunnels/local-management/create-local-tunnel/).
3. Create a local API token with Zone / Cache Purge permission, restricted to the wrkgn.com zone. Put `CLOUDFLARE_API_TOKEN` and `CLOUDFLARE_ZONE_ID` in local environment or `.env`; environment values win. Never put tokens in committed config or SSH command arguments. The helper parses only these literal assignments and does not execute `.env`.
4. Check Prompt Together over HTTP and HTTPS before making changes. Run:

   ```sh
   scripts/setup-vps.sh --credentials ~/.cloudflared/<tunnel-uuid>.json
   ```

   Setup checks fresh SSH logins after firewall/SSH steps, recognizes either project's completed-bootstrap marker, and verifies existing hardening. It ensures the SSH firewall rule without resetting policies or changing HTTP rules. It installs cloudflared from the [signed apt repository](https://pkg.cloudflare.com/) and permits unattended updates from that origin, creates the isolated users/directories/units, validates tunnel ingress and starts the tunnel. The web service is enabled but waits for the first release and snapshot. It fails on a foreign listener at the reserved port. A second run must preserve the other project's availability.
5. Setup attempts a real hostname-only purge locally when credentials exist, recording its verified timestamp or an unverified status in `/etc/wrkgn/hostname-purge-status`. Missing token warns and leaves it unverified. Rejected hostname purge fails; it never falls back to whole-zone purge. Cloudflare [documents hostname purge on all plans](https://developers.cloudflare.com/changelog/post/2025-04-01-purge-for-all/), with plan-dependent limits. The actual zone plan and successful API result remain unverified until this step runs. If credentials were added later, rerun setup to update that record; `--purge-only` proves purge operation but does not rewrite the setup record.
6. Inspect the printed architecture, Debian/glibc, service state, listeners and firewall summary. Check Prompt Together again over HTTP/HTTPS.

## Code and data publication

Build and inspect the schema-6 export locally first; see [MANAGE.md](../MANAGE.md). Then run:

```sh
scripts/deploy.sh
scripts/publish-data.sh
scripts/status.sh
```

Deploy refuses a dirty Git tree unless `--allow-dirty` is passed; such releases are marked dirty. It builds assets, builds a native Mac validator, checks the export, then cross-compiles the Linux release with a recorded commit/build ID. Uploads use temporary paths, SHA-256 verification and immutable final release directories. The first deployment seeds the validated export only when no server snapshot is selected, resolving the initial binary/data dependency. Later code releases preserve selected data. Activation switches the release pointer, restarts and polls loopback `/healthz` for the intended build and snapshot. Failure restores the prior selection and verifies recovery; first deployment has no prior working release. Code deployment does not purge by default; `--purge` evicts HTML after a rendering change.

Publishing defaults to `local-data/public-export` and native checker `target/release/wrkgn-web`, which deploy leaves available. Use `--export PATH`, `--checker PATH`, or `WRKGN_WEB_CHECK_BIN` as needed. It resolves the schema-6 `current.json` pointer and its `manifest.json`, freezes a local copy, validates its version, uploads only `public.sqlite`, checks the SHA-256 and validates remotely before switching. An existing immutable version must match its checksum. It reloads (or starts when necessary), polls health for the exact snapshot version, and restores the prior pointer on activation failure without purging. No raw payloads, working database, private history or model credentials go to the VPS.

After a verified snapshot switch, a single local API request purges both configured hostnames. Missing token permits publish with a warning; cached content can remain stale until purged. API failure exits nonzero after the verified publication; retry purge alone. Never use whole-zone purge.

```sh
scripts/deploy.sh --rollback              # previous release, restart, verify
scripts/deploy.sh --rollback --purge      # also evict old rendered HTML
scripts/publish-data.sh --rollback        # previous data, reload, verify, purge
scripts/publish-data.sh --purge-only      # no SSH or data change
```

Brief process restarts are expected; cached/stale Cloudflare responses may cover them, but cache misses can fail. Socket handoff is outside scope.

## Cloudflare dashboard configuration

The scripts do not configure zone settings. Record actual plan, enabled settings and observed behavior below after applying them. On 2026-11-17 the maintainer applied the public cache and bypass rules below and enabled Smart Tiered Cache; staging HIT/purge behavior is recorded in [the implementation record](implementation/epic06.md). The zone is on the Cloudflare **Free** plan (maintainer-reported). The maintainer reports Always Use HTTPS, minimum TLS 1.2 and the Free Managed Ruleset in place, Bot Fight Mode off and no rate-limiting rule; no dashboard settings have been applied by agents.

| Setting | Required action and verification |
| --- | --- |
| DNS | Proxied `next.wrkgn.com` tunnel CNAME first; apex only at cutover. `www` is a proxied A record to the placeholder `192.0.2.1` (no origin) plus a zone Single Redirect rule from the "Redirect from WWW to root" template: 301 to `https://wrkgn.com` with path and query preserved. It is not a tunnel hostname and needs no purge. Never publish the origin IP for wrkgn. |
| SSL/TLS | Enable Always Use HTTPS and minimum TLS 1.2. Verify HTTP redirect and HTTPS. Defer HSTS until stable. |
| Tiered Cache | Enable Smart Tiered Cache if offered; verify the setting in this zone. [Tiered Cache documentation](https://developers.cloudflare.com/cache/how-to/tiered-cache/). |
| Public cache rule | Match wrkgn.com and next.wrkgn.com, make public HTML/assets eligible, respect origin cache-control and TTL headers. Preserve the default query-string cache key. [Cache Rule settings](https://developers.cloudflare.com/cache/how-to/cache-rules/settings/) and [Origin Cache Control](https://developers.cloudflare.com/cache/concepts/cache-control/). |
| Bypass rule | Exclude `/healthz`, `/api`, `/api/*`, `/account`, `/account/*`; place a bypass rule after the broad eligible rule or exclude them directly in its expression. Verify health remains uncached. |
| WAF | Enable/verify the Free Managed Ruleset. Current [WAF availability](https://developers.cloudflare.com/waf/) lists it and one rate-limiting rule on Free; actual zone configuration remains unverified. |
| Rate limiting | If the zone offers the needed match fields, rate-limit query-string directory/search routes on wrkgn hostnames. Start with a documented threshold selected from real usage, and test ordinary search. Available fields/periods depend on plan; record unavailable options instead of claiming protection. [Rate limiting docs](https://developers.cloudflare.com/waf/rate-limiting-rules/). |
| Bots | Check available bot settings and verify they permit ordinary visitors and uncached health checks. |
| Analytics | Optional cookie-free Cloudflare Web Analytics; record if enabled and check CSP compatibility before adding its script. |

A cache-rule expression for public eligibility can start with the following; explicitly exclude the reserved paths above or use a later bypass rule:

```text
(http.host in {"wrkgn.com" "next.wrkgn.com"})
```

Do not override the origin's `no-store`, cache TTLs or security headers. Keep query strings in the cache key because distinct canonical searches/filters are distinct content. Non-canonical requests should retain their redirect behavior.

## Staging and cutover

1. Deploy/publish to the tunnel and inspect `https://next.wrkgn.com`. Canonical links intentionally point to production. Follow the [manual browser checklist](development.md#manual-browser-checklist), including JavaScript off/on, wide/narrow layout, direct URLs, pagination, filters/sort/search, modal history/focus and CSP.
2. Verify cache headers on real project, list, filtered-list and asset URLs. Repeat requests, inspect `CF-Cache-Status`, and distinguish a lower-tier miss satisfied by an upper tier from an origin fill. Purge, then repeat. Expect warmed HITs; a hostname purge should cause a cold fill. Origin headers set stale-while-revalidate; verify expired-content behavior, not just header presence. See [Cloudflare cache statuses](https://developers.cloudflare.com/cache/concepts/cache-responses/).

   ```sh
   curl -sS -D - -o /dev/null https://next.wrkgn.com/
   curl -sS -D - -o /dev/null 'https://next.wrkgn.com/?q=tool'
   curl -sS -D - https://next.wrkgn.com/healthz
   curl -sS -D - -o /dev/null https://next.wrkgn.com/projects/
   scripts/publish-data.sh --purge-only
   ```

   Use GET rather than relying only on HEAD cache behavior. Health must show the selected build/snapshot with `no-store` and BYPASS/DYNAMIC, never a cached HIT. Non-canonical URLs must redirect. Check actual project/hashed asset paths from returned HTML.
3. Run setup twice. Before and after each infrastructure step, check Prompt Together over HTTP/HTTPS. Test web and tunnel restarts independently, then reboot deliberately and check both projects again. Confirm the default cloudflared unit is inactive/absent unless another project owns it. From an outside network, verify the VPS's public IP cannot reach port 3000 and exposes nothing for wrkgn; existing Prompt Together 80/443 service is expected.
4. Exercise successful deploy, deliberately failed activation, automatic recovery and release rollback; exercise publish, invalid snapshot rejection, data rollback, and purge-only. Record command results and health IDs. Do not infer these from Bash syntax checks.
5. After staging passes, remove `wrkgn.com` from the Pages custom domains and remove its conflicting Pages DNS record. Route the apex to the tunnel:

   ```sh
   cloudflared tunnel route dns wrkgn wrkgn.com
   ```

   Verify the proxied record, HTTPS, cache/security headers, redirects and direct project URLs on production; repeat the browser checks.
6. Rollback is by the scripts: `scripts/deploy.sh --rollback` restores the previous release and `scripts/publish-data.sh --rollback` the previous snapshot, each verified against health. There is no Cloudflare Pages fallback; the legacy Astro site (`website/`), its tracked data and the JSON export were removed. The export is now `current.json`, `snapshots/<version>/manifest.json` and `snapshots/<version>/public.sqlite` only. If the VPS itself is unavailable, restore it with `scripts/setup-vps.sh` and `scripts/deploy.sh`; there is no alternative host.

## Operations and troubleshooting

`scripts/status.sh` reports service status, selected pointers/health, disk/memory, recent warnings/errors, retained versions and the setup purge record over SSH. Journald receives path-only request logs; query strings, cookies and client IPs are not logged. No pipeline processes or credentials belong on the VPS.

| Symptom | Check/action |
| --- | --- |
| SSH/setup fails | Confirm explicit alias, `pixlie`, key login, sudo and Debian. Repair existing hardening deliberately; do not reset shared firewall/SSH configuration. |
| Web service skipped | First deploy must supply both binary and valid snapshot; inspect current symlinks and status. |
| Tunnel 502 / origin down | Check loopback `/healthz`, `wrkgn-web` journal, current release/snapshot and configured port. A healthy tunnel does not imply a healthy web process. |
| Tunnel disconnected | Inspect `cloudflared-wrkgn` journal, credentials ownership/mode, UUID and outbound connectivity. Never install the default service to repair this project. |
| New snapshot rejected | Read reload errors; old in-memory data may still serve. Publish restores the selected pointer on failed verification. Validate exact public SQLite locally. |
| New content stale | Confirm health version, then `--purge-only`; inspect token scope/zone, cache rules and response headers. A successful data upload does not prove purge success. |
| Purge rejected | Verify local token/zone and actual API capability; no whole-zone fallback. Retry purge-only after fixing credentials. |
| Cross-build failure | Check Zig/cargo-zigbuild, installed Linux target, pinned nightly, bundled native dependency support and host glibc >=2.31. No binary is activated before build/upload verification. |
| Resource pressure | Inspect service memory/CPU and journal OOM/restart events; measure full/reload workload before adjusting limits, keeping headroom for other projects. |
| Operation already running | Let the owning setup/deploy/publish finish; inspect SSH/process state before intervening. Do not bypass the shared lock. |

On 2026-11-18 the maintainer configured UptimeRobot for `https://wrkgn.com/healthz`. Check response content as well as HTTP 200. Health is never cached and therefore checks origin availability through Cloudflare.

For another Rust backend, reserve a distinct loopback port in this registry first; create separate non-login application/tunnel users, release/data/config directories, named tunnel, ingress config and systemd units in its own repository. Reuse only the installed cloudflared package and existing admin access. Never append its ingress to wrkgn's config, reuse wrkgn's credentials, alter Prompt Together, or install a competing default tunnel service.

## Live validation record

Recorded through 2026-11-18: production cutover with production health/cache/redirect checks; Free plan, cache/bypass rules, Smart Tiered Cache, HTTPS/TLS and managed WAF settings, verified hostname purge, staging cache HIT/purge, repeat setup, three successful deployments, changed-snapshot publication, purge-only, reboot recovery, outside-port check, post-reboot Prompt Together availability, independent restarts, repeat publish, invalid snapshot rejection, data and release rollback, failed activation with automatic recovery, and staging browser checks. Full/reload memory and p99 measurements were deferred by the maintainer; service limits remain provisional. Pending maintainer execution: detected architecture/Debian/glibc, full/reload memory and p99 measurements and the rate-limit threshold. The Astro/Pages/JSON cleanup was performed on 2026-11-18 at the maintainer's direction (marketing began that day, so the planned stable period no longer applied); the maintainer then deleted the Cloudflare Pages project and confirmed no Pages DNS record remains. Record dates, commands, release/snapshot IDs and observed results in [the implementation record](implementation/epic06.md).
