# Epic 06 implementation record

Four agents each received one task. No review pass was performed, as requested. The maintainer will test the delivered work. No live setup, deploy, publish, Cloudflare mutation, DNS cutover, reboot, model call or data refresh occurred during implementation.

| Task | Delivered |
| --- | --- |
| 1 | Shared `deploy/wrkgn.conf`, VPS setup/status commands, host helpers, project-owned cloudflared ingress and hardened systemd templates |
| 2 | Mac-native snapshot validation and LLVM Linux release build, checksum uploads, first-deploy snapshot seed, build/snapshot health verification, recovery, rollback and retention |
| 3 | Exact public snapshot validation/publication, immutable checksum installs, reload health checks and recovery, rollback/retention, local hostname-only purge and purge-only retry |
| 4 | Hosting guide, operational/development/product/agent workflow updates, credential examples and explicit staging/cutover/cleanup handoff |

## Checks actually reported

The three script agents each reported `bash -n` passing on their scripts/helpers. ShellCheck was not installed, so it was not run. No full test suites, Rust builds, cross-builds, script execution against a VPS, browser checks, performance/memory tools or live Cloudflare API checks were run. Documentation was written without a review pass or test execution.

Official documentation was consulted for Cloudflare's signed package repository, tunnel commands, hostname purging, cache settings and WAF availability. Hostname purge is documented on all plans; no actual zone plan, token or API capability has been verified. Setup records success only after a real hostname-only purge, or records unverified when credentials are absent. No script falls back to whole-zone purge.

## Maintainer work remaining

Follow [hosting.md](../hosting.md) in order: install prerequisites/configure SSH, create the named tunnel and staging DNS locally, configure local cache-purge credentials, run setup twice, deploy the initial code/snapshot, configure the zone and validate staging. Record detected architecture/Debian/glibc, tunnel/unit health and the real purge result.

Run ShellCheck, successful/failed deploy and recovery, binary rollback, valid/invalid publish, snapshot rollback and purge-only. Check Prompt Together HTTP/HTTPS before and after infrastructure steps, foreign port exclusion, outside public-IP access, service restarts and reboot. Verify actual origin memory/latency and two-snapshot reload peaks before accepting provisional web 1 GiB/75% CPU and tunnel 256 MiB/25% CPU limits. Epic 05 performance/parity/browser checks remain relevant; its filtered ordering difference remains deferred.

Through staging, verify cache eligibility, MISS/HIT, health bypass/no-store, redirects, stale behavior and purge effects on lists/projects/filtered pages/assets. Perform the browser checklist without automation. Only then move the apex from Pages to the tunnel, repeat production checks and configure an uptime monitor (currently none).

Retain the last Pages deployment and repository integration as documented until cutover stabilizes; do not push large snapshot updates to main meanwhile. Keep the Pages project for at least two weeks. After the stable period, disconnect integration and make the explicit cleanup change removing Astro/tracked public data, unused JSON export generation and dependent tooling/contracts. These removals are deferred, so live cutover and the complete epic acceptance criteria are not yet established.

## Live results

### 2026-11-16: staging status and changed-snapshot publication

The maintainer reports local prerequisites installed and setup/deploy completed. This session independently ran:

- `scripts/status.sh`: both `wrkgn-web` and `cloudflared-wrkgn` enabled and active; no recent warning/error journal entries. Selected build `20261116T083842Z-daff6497e8dd-dirty-11434`, initial snapshot `9086b9a28e419548`. Only one release and one snapshot were retained before publication. Web service memory was 38.3 MiB and tunnel memory 23.3 MiB; these are idle observations, not full/reload peak measurements. Setup's hostname-purge record still said unverified.
- `target/release/wrkgn-web check local-data/public-export`: valid schema 6, version `eaf547688b75f209`.
- `scripts/publish-data.sh`: exited successfully, verified active snapshot `eaf547688b75f209`, and reported successful purge of configured production/staging hostnames. This tested changed-data publication, not repeat publication. The setup purge record is not rewritten by this command.
- `curl --max-time 20 -sS -D - https://next.wrkgn.com/healthz`, before and after publication: HTTP 200, `Cache-Control: no-store`, `CF-Cache-Status: DYNAMIC`; build unchanged and final snapshot `eaf547688b75f209`.
- Two GETs of `https://next.wrkgn.com/`: HTTP 200 and `Cache-Control: public, max-age=60`, but both reported `CF-Cache-Status: DYNAMIC`. HTML edge caching is not yet verified; inspect the zone cache rules before cutover. Returned HTML identified the newly published snapshot.
- `curl --max-time 20 -sS -D - -o /dev/null https://promptogether.com`: HTTP 200 during publication. This does not establish an origin-specific coexistence test or HTTP/reboot availability.

### 2026-11-17: second deployment from a clean commit

The maintainer ran these commands and reported their final output lines:

- `scripts/deploy.sh` at commit `b66edfc` (clean tree, no `--purge`): validated schema 6 snapshot `eaf547688b75f209` and reported healthy release `20261117T120205Z-b66edfc7d784-2097` with snapshot `eaf547688b75f209`.
- `scripts/status.sh`: retained releases `20261116T083842Z-daff6497e8dd-dirty-11434` and `20261117T120205Z-b66edfc7d784-2097`; retained snapshots `9086b9a28e419548` and `eaf547688b75f209`. The setup hostname-purge record still said unverified. Service status, pointer and memory lines were not reported.
- `curl -sS -D - https://next.wrkgn.com/healthz`: served through Cloudflare, body `{"build":"20261117T120205Z-b66edfc7d784-2097","snapshot":"eaf547688b75f209"}`. Status and cache headers were not reported.

This establishes a successful second deployment of a traceable, non-dirty build, with the earlier dirty release retained as the rollback target. It does not exercise binary rollback, failed activation or recovery.

### 2026-11-17: zone cache rules and staging cache behavior

The maintainer applied these dashboard settings. The zone had no prior cache rules.

- Cache rule `wrkgn public cache`: `(http.host in {"wrkgn.com" "next.wrkgn.com"})`, eligible for cache, edge TTL uses cache-control when present and bypasses otherwise, browser TTL respects origin, default cache key and stale serving.
- Cache rule `wrkgn bypass`, ordered after it: same hosts and path `/healthz`, `/api`, `/api/*`, `/account` or `/account/*`; bypass cache.
- Caching Level Standard; Smart Tiered Cache enabled.

The maintainer reports the zone is on the Free plan. SSL/TLS, WAF, rate limiting and bot settings were not inspected or reported.

This session then ran repeated `curl` GETs against `https://next.wrkgn.com`:

- `/`, `/?q=tool`, `/projects/mib-8e3bc2382f/` and the `/projects/` → `/` redirect: MISS, then HIT.
- `/assets/global-FA4ILTJY.css`: HIT before the rules existed, from Cloudflare's default static caching.
- `/healthz` (`no-store`): DYNAMIC on every request. `/api` and `/api/x` (404 with `public, max-age=30`): DYNAMIC on every request, so the bypass rule applies.
- `/?q=tool&x=1`: 301 to `/?q=tool`, then a cached redirect HIT.
- `/` at Age 101–107 s: still HIT. This is expected, because edge lifetime comes from `Cloudflare-CDN-Cache-Control` (7 days for public HTML, plus 1 day stale-while-revalidate). Browser `max-age=60` does not bound edge lifetime. Expired-content and stale-while-revalidate behavior remain unobserved because of the 7-day TTL.
- After the maintainer ran `scripts/publish-data.sh --purge-only` ("Purged configured production and staging hostnames"), every URL above, including the CSS asset, returned MISS once and then HIT. `/healthz` remained DYNAMIC. Lower-tier versus upper-tier fills were not distinguished.

### 2026-11-17: remaining zone settings, repeat setup, third deployment and reboot

The maintainer reports that Always Use HTTPS, minimum TLS 1.2 and the Free Managed Ruleset were checked or applied. Bot Fight Mode is off and no rate-limiting rule is configured. The maintainer then ran `scripts/setup-vps.sh --credentials ...` (a repeat setup on the configured host), `scripts/deploy.sh`, `scripts/publish-data.sh` and a deliberate VPS reboot. Their output was not reported. This session then ran:

- `scripts/status.sh`: boot at 14:21:20 UTC. `wrkgn-web` and `cloudflared-wrkgn` were enabled and active from 14:21:38 without intervention, with no recent warning/error entries. Release `20261117T134535Z-ef750ef7cc65-7428` and snapshot `de09f5cc862da924` were selected, and loopback health matched. Three releases and three snapshots were retained. Web memory was 52.4 MiB (limit 1.0 GiB) and tunnel memory 62.8 MiB (limit 256 MiB). These are post-boot idle observations. The repeat setup rewrote the purge record to "verified by successful hostname purge at 2026-11-17T13:12:15Z".
- `target/release/wrkgn-web check local-data/public-export`: version `de09f5cc862da924`, matching the active snapshot.
- `https://next.wrkgn.com/healthz`: the same build and snapshot. `http://next.wrkgn.com/`: 301 to HTTPS. A TLS 1.1-capped client failed to connect, while TLS 1.2 succeeded.
- Over SSH: `wrkgn-web` listens only on `127.0.0.1:3000`, and only Nginx listens on 80/443. `cloudflared.service` has no unit file and is inactive. Nginx is active.
- From the maintainer's network: `nc` to the VPS public IP on port 3000 was closed/filtered, and HTTP to `:3000/healthz` failed.
- `https://promptogether.com`: 200 after the reboot. `http://promptogether.com`: 301 to HTTPS. Prompt Together was not checked before or between the maintainer's steps.

This covers setup idempotency on the host, a third successful deployment, publication of a further changed snapshot, reboot recovery of both units, the default-unit check and the outside-port check. Independent web/tunnel restarts were not separately exercised.

### 2026-11-17: restart, publication, rollback and failed-activation drills

The maintainer ran the following and reported success without pasting output. The expected results were the ones listed with each step:

- `sudo systemctl restart wrkgn-web`, then `sudo systemctl restart cloudflared-wrkgn`, with `/healthz` checked after each.
- A repeat `scripts/publish-data.sh` of unchanged snapshot `de09f5cc862da924`.
- `scripts/publish-data.sh --export <scratch copy>`, where the copy had the same pointer/manifest and the `evidence` table dropped. The local `wrkgn-web check` rejects that copy with "missing public snapshot tables", so publication should fail before upload.
- `scripts/publish-data.sh --rollback` (to `eaf547688b75f209`), then `scripts/publish-data.sh` to restore `de09f5cc862da924`.
- `scripts/deploy.sh --rollback` twice: rollback to the previous release, then swap back.

Failed activation: a temporary uncommitted change made `wrkgn-web` exit when serving, while `check` kept working. The change was reverted afterwards and never committed. `scripts/deploy.sh --allow-dirty` uploaded and selected release `20261117T155606Z-5a858e99fbd6-dirty-17852` and reported:

```text
Health verification failed: curl: (7) Failed to connect to 127.0.0.1 port 3000 after 0 ms: Couldn't connect to server
Release activation failed; restoring previous release. Last health: curl: (7) Failed to connect to 127.0.0.1 port 3000 after 0 ms: Couldn't connect to server
Previous release restored and healthy.
```

Afterwards, `scripts/status.sh` and SSH showed:

- `current` was `20261117T144913Z-5a858e99fbd6-12613`, the release selected before the drill, and `previous` was still `20261117T134535Z-ef750ef7cc65-7428`. The failed deployment did not change the rollback target.
- The snapshot was `de09f5cc862da924`, and staging health matched.
- The failed release directory remains retained and unselected.
- `wrkgn-web` was active from 15:57:59 UTC with `NRestarts=9`. The journal shows systemd restart failures at about 3-second intervals from 15:57:28 to 15:57:57, so staging origin was unavailable for roughly 30 seconds; cached pages may have covered part of that window.
- Prompt Together returned HTTPS 200 and HTTP 301 afterwards.

### 2026-11-17: staging browser checks and measurement deferral

The maintainer reports the [manual browser checklist](../development.md#manual-browser-checklist) passed on `https://next.wrkgn.com`. The maintainer chose to defer full/reload performance and memory measurements past cutover. The `wrkgn-web` 1 GiB and tunnel 256 MiB limits therefore remain provisional, and the p99 budget is unmeasured.

Before cutover, `wrkgn.com` resolved to Cloudflare proxy addresses served by Pages, and `/healthz` returned Pages HTML. `www.wrkgn.com` had no DNS record. The tunnel ingress already lists `wrkgn.com` and `next.wrkgn.com`.

### 2026-11-18: production cutover

The maintainer removed `wrkgn.com` from the Pages project's custom domains. No separate Pages DNS record was visible afterwards; only `next.wrkgn.com` remained. The maintainer then ran `cloudflared tunnel route dns wrkgn wrkgn.com`, confirmed a proxied CNAME to the `ed1dbb5c-a6ba-4b19-ae70-c7936df8d5dd` tunnel, and ran `scripts/publish-data.sh --purge-only`. The maintainer reports `https://wrkgn.com` shows the latest project count in a browser rather than the Pages data.

This session then checked `https://wrkgn.com` with `curl`:

- `/healthz`: 200, `no-store`, DYNAMIC on repeat, with body `{"build":"20261117T144913Z-5a858e99fbd6-12613","snapshot":"de09f5cc862da924"}`. Before cutover the same URL returned Pages HTML.
- `http://wrkgn.com/`: 301 to `https://wrkgn.com/`. A TLS 1.1-capped client failed. The canonical link is `https://wrkgn.com/`.
- `/?q=tool`, `/projects/mib-8e3bc2382f/` and the `/projects/` → `/` 301: MISS, then HIT. `/` was already HIT, warmed by the maintainer's browser visit after the purge. `/assets/global-FA4ILTJY.css`: HIT. `/api`: 404 and DYNAMIC on repeat.
- Content-Security-Policy, `X-Content-Type-Options: nosniff` and `Referrer-Policy` are present.
- `https://promptogether.com` returned 200, and `https://next.wrkgn.com/healthz` returned 200.

Rollback remains reattaching `wrkgn.com` to the Pages project after removing the tunnel CNAME. Keep the Pages project until stable-period cleanup, no earlier than 2026-12-02.

Still pending: external uptime monitor, full/reload performance and memory, rate-limit threshold, expired-content/stale behavior, Pages repository disconnection and stable-period cleanup.
