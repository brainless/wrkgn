# Epic 06: VPS hosting behind Cloudflare and production cutover

Status: Setup/deployment/publication tooling and documentation implemented through four delegated tasks. Bash syntax checks passed; ShellCheck was unavailable. No review or test-suite/live infrastructure runs were performed, as requested. The maintainer subsequently ran staging and live acceptance checks and cut production over on 2026-11-18. Deferred performance/memory measurements and stable-period Astro/JSON cleanup (no earlier than 2026-12-02) remain. See [implementation record](../docs/implementation/epic06.md) and [hosting runbook](../docs/hosting.md).

Depends on: [Epic 05](05-rust-web-server.md).

## Outcome

`wrkgn-web` runs on the maintainer's Debian VPS behind Cloudflare. A wrkgn-owned Cloudflare Tunnel connects the server to Cloudflare, so the VPS opens no public HTTP ports for wrkgn. The maintainer builds release binaries locally and deploys them with one script. A second script publishes approved snapshots. Cloudflare caches pages at the edge, and publishing purges stale entries. After verifying the setup on a staging hostname, `wrkgn.com` moves from Cloudflare Pages to the tunnel. The Astro site and the tracked snapshot data are then removed from the repository.

All server management scripts and infrastructure configuration belong to this epic.

## Environment

- **Server:** Debian, 2 vCPU, 6 GB RAM.
- **Shared with Prompt Together.** That project's `~/Projects/promptogether/scripts/setup-vps.sh` already:
  - created the `pixlie` admin account with key-only SSH and passwordless sudo;
  - disabled root and password SSH logins;
  - enabled unattended security upgrades;
  - installed Nginx, with UFW allowing SSH, 80, and 443.
- **Coexistence requirement:** wrkgn setup must leave Prompt Together working. It must never reset UFW, remove existing firewall rules, or change existing Nginx sites, certificates, or users. Nginx serves only Prompt Together; wrkgn traffic never passes through it.
- **More projects to come.** The maintainer plans to host other Rust backends on this VPS using the same pattern. wrkgn must not claim any machine-wide resource another project would also need: the default `cloudflared` config path and service, a shared tunnel, or a loopback port without recording it.
- **Local build machine:** an Apple Silicon Mac (`arm64`). The VPS architecture is detected during setup.

## Product decisions

- **Cloudflare Tunnel from the start, with no reverse proxy on the VPS.** The tunnel client makes outbound connections only and forwards requests directly to `wrkgn-web`.
  - The origin's IP address is not in DNS, and nothing can bypass Cloudflare to reach the origin.
  - TLS ends at Cloudflare. No origin certificate, Certbot setup, or Nginx site is needed for wrkgn.
  - `wrkgn-web` listens on `127.0.0.1` only.
  - Every request reaches `wrkgn-web` from loopback. Later features that need the visitor's address read the `CF-Connecting-IP` header.
- **One tunnel per project.** wrkgn has its own named tunnel, its own `cloudflared` process, its own service account, and its own systemd unit. Other projects on the VPS get their own tunnels. Restarting or misconfiguring one project's tunnel never affects another, and each repository owns its tunnel configuration completely.
- **Configuration in the repository.** Use a locally managed named tunnel with its ingress configuration in the repository. The tunnel credentials file is created on the maintainer's computer and uploaded by the setup script. It is never committed.
- **One source for host settings.** A committed `deploy/wrkgn.conf` holds the non-secret values the scripts share: SSH host alias, loopback port, production and staging hostnames, and tunnel name. The ingress config, the systemd unit, health checks, and the purge list are rendered from it.
- **The edge cache is an accelerator, not a host.** Cache misses go to the origin, through Tiered Cache. The origin must serve every URL quickly on its own.
- **Moderation removals must leave the edge quickly**, so purging is part of every publish.
- **Purge by hostname from the start.** Purges name wrkgn's hostnames rather than the whole zone, so the same scripts stay correct if other hostnames in the zone are added later.
- **Nothing irreplaceable lives on the VPS in this epic.** The binary and public data are reproducible from the maintainer's computer. Database backups (Litestream to R2) arrive with the first epic that stores community data.

## Scope

### Server setup: `scripts/setup-vps.sh`

1. Copy Prompt Together's `scripts/setup-vps.sh` as the starting point. Keep its structure: run locally over SSH, check each SSH change with a fresh login, use strict bash, and stay idempotent.
2. Adapt it for a shared server:
   - Read the SSH host alias from `deploy/wrkgn.conf`, defaulting to a `wrkgn` alias pointing at the same VPS.
   - Keep the base hardening steps. When a completed bootstrap is detected, from either project, they only verify and never repeat destructive steps.
   - Remove `ufw --force reset` and the Nginx/Certbot domain mode. Only ensure that the SSH rule exists.
   - Do not add firewall rules for wrkgn, and do not remove Prompt Together's 80/443 rules.
   - Use a wrkgn-specific bootstrap marker.
3. Install and configure the wrkgn services:
   - **Users:** two non-login system users, `wrkgn` for the web server and `wrkgn-tunnel` for its tunnel.
   - **Directories:**
     - `/opt/wrkgn/releases/<release-id>/` holds the binaries, with a `current` symlink.
     - `/var/lib/wrkgn/snapshots/<version>/` holds `public.sqlite`, with a `current` pointer.
     - `/etc/wrkgn/` holds non-secret server configuration.
     - `/etc/cloudflared/wrkgn/` holds the rendered tunnel config `config.yml` and the credentials file, owned by `wrkgn-tunnel`. The directory is mode `0700` and the credentials file is mode `0600`.
   - **`cloudflared`** from Cloudflare's signed apt repository, so unattended upgrades keep it updated. Installing the package is shared and idempotent. Setup never uses `cloudflared service install`, never writes `/etc/cloudflared/config.yml`, and never enables the package's default `cloudflared.service`.
   - **`wrkgn-web.service`:**
     - restart on failure and `ExecReload` sending `SIGHUP`;
     - hardening: `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`, `PrivateTmp`, and read-only access to the release and snapshot directories;
     - `MemoryMax` and CPU quota limits that leave headroom for Prompt Together and future projects;
     - listens on the loopback port from `deploy/wrkgn.conf`. Setup fails if another process already listens on that port.
   - **`cloudflared-wrkgn.service`:** a wrkgn-owned unit that runs `cloudflared tunnel --config /etc/cloudflared/wrkgn/config.yml run` as `wrkgn-tunnel`, restarts on failure, starts after the network is online, and uses the same hardening options where `cloudflared` supports them.
   - **journald retention limits**, so logs cannot fill the disk. Setup changes them only through a drop-in file and leaves any stricter existing limits in place.
4. Print a summary at the end: detected architecture, Debian version, glibc version, service states, listening sockets, and UFW rules. Fail clearly if Debian, the `pixlie` account, or the SSH alias is missing.

### Tunnel

5. Commit a tunnel ingress template in `deploy/cloudflared/`, rendered from `deploy/wrkgn.conf`, that maps:
   - `wrkgn.com` to `http://127.0.0.1:<port>`;
   - the staging hostname, `next.wrkgn.com`, to the same port;
   - everything else to `http_status:404`.
   The rendered config names only wrkgn's tunnel and hostnames.
6. Document the one-time tunnel commands, run on the maintainer's computer:
   - `cloudflared tunnel login` and `cloudflared tunnel create wrkgn`;
   - DNS routing for the staging hostname to the `wrkgn` tunnel.
   The setup script uploads the credentials file and the rendered config to `/etc/cloudflared/wrkgn/`, then restarts `cloudflared-wrkgn`. Credentials never enter git, logs, or script output.
7. With the tunnel running:
   - the VPS's public IP serves nothing for wrkgn on any port;
   - Prompt Together continues to answer on its own domain;
   - the tunnel reconnects after a `cloudflared-wrkgn` restart, a `wrkgn-web` restart, and a reboot;
   - `systemctl status cloudflared` shows the package's default unit inactive or absent, unless something other than wrkgn enabled it.

### Deploy: `scripts/deploy.sh`

8. Build and deploy a release from the maintainer's computer:
   - Record the git commit, and refuse to deploy with uncommitted changes unless `--allow-dirty` is passed. Mark such builds in the release ID.
   - Build TypeScript and CSS assets, then cross-compile `wrkgn-web` in release mode for the detected VPS target with `cargo zigbuild`. Pin a glibc version no newer than the VPS's Debian release, for example `x86_64-unknown-linux-gnu.2.36`.
   - Release builds use LLVM. The Cranelift settings apply only to dev and test.
   - Run `wrkgn-web check` locally against the current export before uploading.
   - Upload to a new release directory under a temporary name, verify the SHA-256 checksum on the server, then rename.
   - Switch the `current` symlink, restart `wrkgn-web`, and poll `/healthz` on the loopback port until it reports the new build ID.
   - On failure, restore the previous symlink, restart, and exit nonzero with the health output.
   - Keep the last five releases.
   - Do not purge the edge cache by default. Rendering changes can use `--purge`.
9. `scripts/deploy.sh --rollback` switches to the previous release and verifies health in the same way.
10. Deploys restart the process. Cloudflare serves cached or stale pages during the brief gap. Zero-downtime socket handoff is out of scope.

### Publish data: `scripts/publish-data.sh`

11. Publish the current export to the VPS:
    - Read `local-data/public-export/current.json` and resolve that snapshot's `public.sqlite`. Validate it locally with `wrkgn-web check`.
    - Upload it to `/var/lib/wrkgn/snapshots/<version>/` under a temporary name, verify its checksum, then rename. Uploading a version that already exists is a no-op.
    - Switch the server's snapshot pointer, run `systemctl reload wrkgn-web`, and poll `/healthz` until it reports the new snapshot version.
    - If the server rejects the snapshot, restore the previous pointer and exit nonzero without purging.
    - After a verified switch, purge the Cloudflare cache by hostname, using the production and staging hostnames from `deploy/wrkgn.conf` in one purge request. Purge is the final step; if it fails, report the failure so the maintainer can rerun the purge alone with `--purge-only`.
    - Keep the last five snapshots. `--rollback` switches to the previous one, then reloads, verifies, and purges.
12. Cloudflare credentials:
    - Use an API token limited to cache purge on the `wrkgn.com` zone, with the zone ID, read from local `.env` or the environment.
    - Never store it on the VPS, and never print it.
    - The script works without the token and then warns that cached pages stay stale until a purge.
13. Purge scope:
    - Every purge names wrkgn's hostnames and never purges the whole zone, so other hostnames later added to the zone keep their caches.
    - Confirm during setup that the zone's plan supports purge by hostname, and record the result. If the API rejects a hostname purge, the script fails clearly and does not fall back to purging everything.
    - Purging specific URLs for individual moderation actions is deferred to the moderation epic. Purging wrkgn's hostnames is correct for now, because publishing is the only way content changes.

### Cloudflare zone configuration

14. Document the dashboard configuration in `docs/hosting.md`. Note which features the current plan includes, and verify each after it is applied.
    - **DNS:** proxied tunnel records for `next.wrkgn.com`, and at cutover for `wrkgn.com`. Redirect `www` to the apex if it is used.
    - **SSL/TLS:** Always Use HTTPS and minimum TLS 1.2. Defer HSTS until the setup has been stable for a while.
    - **Caching:**
      - Enable Smart Tiered Cache.
      - Add a cache rule that makes HTML on the wrkgn hostnames eligible for caching and respects the origin's caching headers from Epic 05.
      - Exclude `/healthz` and reserved future paths such as `/api/*` and `/account/*`.
      - Keep the default query-string cache key, because the origin canonicalizes URLs.
    - **Security:**
      - Enable the free managed WAF rules.
      - Add a rate-limiting rule for query-string list and search requests, if the plan includes one.
      - Configure bot settings, and check that they do not block ordinary visitors or the tunnel health checks.
    - **Analytics:** optionally enable Cloudflare Web Analytics, which uses no cookies.
15. Verify through Cloudflare with `curl` on the staging hostname:
    - `cf-cache-status` is `MISS`, then `HIT`, for project pages, list pages, filtered pages, and assets;
    - `BYPASS` or `DYNAMIC` for `/healthz`;
    - `stale-while-revalidate` behavior;
    - a hostname purge produces `MISS` on the next request for that hostname;
    - non-canonical URLs redirect before caching.

### Cutover and cleanup

16. **Before cutover:** do not push large snapshot updates to `main`, because the Pages build fails above 20,000 files. The last successful Pages deployment remains live.
17. **Staging first:**
    - deploy and publish to `next.wrkgn.com`;
    - complete the maintainer's manual browser checks;
    - check caching behavior, uptime across restarts and a reboot, and Prompt Together's availability.
18. **Cutover:**
    - remove `wrkgn.com` from the Pages project's custom domains;
    - point the apex at the tunnel;
    - re-verify headers, redirects, and direct project URLs on `wrkgn.com`.
    **Rollback:** reattach the domain to the Pages project, which serves its last deployment. Keep the Pages project for at least two weeks after cutover.
19. **After a stable period:**
    - disconnect the Pages repository integration;
    - delete the Astro application under `website/` and the tracked `website/data/public/`;
    - remove JSON discovery and detail generation from export if nothing else uses them;
    - update `.gitignore`.
    Old snapshots remain in git history. Rewriting history is out of scope.
20. **Documentation:**
    - Rewrite the publication sections of `MANAGE.md`: `export → publish-data.sh` replaces `stage → commit/push → Cloudflare`, and `deploy.sh` covers code releases.
    - Add `docs/hosting.md` covering server layout, services, tunnel setup, Cloudflare settings, and a troubleshooting guide. Include a VPS resource table listing each project's loopback port, service units, system users, tunnel name, and directories, and the steps for adding another project's tunnel without touching wrkgn's.
    - Update AGENTS.md and the PRD architecture and workflow sections.

### Operations

21. `scripts/status.sh` shows over SSH:
    - service states and the deployed release and snapshot versions;
    - disk and memory use;
    - recent `wrkgn-web` and `cloudflared-wrkgn` errors;
    - the snapshots and releases on disk.
22. Use a free external uptime check on `https://wrkgn.com/healthz`, or document that none is configured. A `/healthz` response through Cloudflare is never cached, so it shows that the origin itself is up.

## Maintainer workflow

```text
Code:  commit → scripts/deploy.sh            (build locally, upload, restart, verify)
Data:  collect → process → review → export → scripts/publish-data.sh   (upload, reload, verify, purge hostnames)
Check: scripts/status.sh
```

Website data no longer goes through git, and no build runs in Cloudflare.

## Acceptance criteria

- Running `setup-vps.sh` twice on the shared VPS succeeds without changing Prompt Together's firewall rules, Nginx sites, certificates, or availability.
- `wrkgn-web` and `cloudflared-wrkgn` run as hardened systemd services under their own users, restart on failure, and survive a reboot. No wrkgn port is reachable on the VPS's public IP.
- wrkgn uses no machine-wide tunnel resources: no `/etc/cloudflared/config.yml`, no default `cloudflared.service`, and no tunnel shared with another project. Its port and resources are recorded in `docs/hosting.md`.
- `deploy.sh` builds on the Mac and deploys a working Linux binary. A failed health check rolls back automatically, and `--rollback` restores the previous release.
- `publish-data.sh` validates, uploads, switches, verifies, and purges wrkgn's hostnames, in that order. An invalid snapshot never replaces the live one. Rollback works.
- Through Cloudflare, cacheable pages report `HIT` after warming. Hostname purges take effect, and no script purges the whole zone. `/healthz` is never served from cache. Non-canonical URLs redirect.
- `wrkgn.com` serves the Rust server with all existing public URLs working. The documented rollback to Pages stays available until cleanup.
- After cleanup, the repository contains no Astro site or tracked public snapshot data, and the documentation describes only the current workflow.
- No credentials appear in git, script output, server logs, or files readable outside their service accounts.

## Validation

- Run `shellcheck` on every script.
- Rerun the setup script to confirm idempotency on the real VPS.
- Before and after each infrastructure step, check that Prompt Together is still available over HTTP and HTTPS.
- From an outside network, scan or `curl` the VPS's public IP for wrkgn ports.
- Restart and reboot tests for both services.
- Deploy, failed-deploy (an intentionally broken health check), and rollback runs.
- Publish, invalid-publish, rollback, and purge-only runs.
- Header and cache-status checks through Cloudflare on staging, then on production after cutover.
- The maintainer's manual browser checks on staging and production; no browser automation.

Record the commands and results actually run, including any Cloudflare plan limitations found.

## Boundaries and handoff

This epic does not include:

- accounts, sessions, a community database, Litestream backups, R2 uploads, Turnstile, or per-URL moderation purges, which arrive with the community epics;
- Cloudflare Workers code;
- multi-server deployment or zero-downtime socket handoff;
- changes to Prompt Together's hosting, including moving it to its own tunnel or closing ports 80 and 443;
- hosting other projects. The epic only keeps wrkgn's resources separate and documents the pattern;
- Terraform or another infrastructure-as-code tool for Cloudflare. Dashboard settings are documented and verified manually.
