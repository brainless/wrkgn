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

Not yet executed. Add dates, exact commands, observed results, build IDs and snapshot versions here after maintainer testing, including any zone-plan limitations and the eventual cleanup date.
