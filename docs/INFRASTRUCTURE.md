# Infrastructure

Operating notes for the CoA infrastructure node. Current measured state: [VPS_BASELINE.md](VPS_BASELINE.md).
This file never contains private keys, passwords, DB credentials, transfer PINs, API secrets or raw tokens.
`<VPS_IP>` stands for the public IPv4, which is intentionally not stored in git.

## Role

`coa-infra-01` (OVHcloud VPS, Ubuntu 24.04 LTS) is the shared infrastructure node for the planned services:

```
Registry API   Relay   Coordinator   PostgreSQL   Caddy (ingress)   monitoring
```

Phase -1 prepared the host. **Phase 10 deployed the Registry and PostgreSQL** (see [REGISTRY_DEPLOYMENT.md](REGISTRY_DEPLOYMENT.md)); **Phase 12 deployed the Coordinator**; the **Phases 11-12 TLS gate** enabled production HTTPS/WSS ingress; and **Phase 13 deployed the Game Relay**.
The VPS is not the source of truth for any player data.

Public domain is `coa-manager.duckdns.org` pointing to `coa-infra-01`. Caddy automatically obtains and renews trusted Let's Encrypt TLS certificates via ACME. HTTP on port 80 redirects to HTTPS on port 443.

## Service architecture

```
Internet
   |
 22/tcp -> sshd
 80,443/tcp (80 redirects to 443; ACME TLS via Let's Encrypt)
   v
 Caddy  (container, network coa-ingress)
   |
   +--> Registry API   (coa-registry:8080, networks coa-ingress + coa-db, no published port)
   |        |
   |        +--> PostgreSQL (coa-postgres:5432, network coa-db only, --internal, no published port)
   |
   +--> Coordinator    (coa-coordinator:8081, networks coa-ingress + coa-db, no published port)
   |
   +--> Game Relay     (coa-relay:8082, networks coa-ingress + coa-db, published 40000-43999/tcp)
```

Rules:

- PostgreSQL must never be internet-exposed: no `ports:` entry, only on the `coa-db` internal network, no UFW rule for 5432.
- Registry (8080), Coordinator (8081), and Relay control HTTP/WS (8082) have no published host ports; they are reverse-proxied exclusively through Caddy.
- Game Relay publishes the dedicated TCP port range `40000:43999/tcp` for relayed WoW client game streams (Auth & World).
- No admin dashboards or the Docker API on public interfaces. Caddy's admin API is disabled.

## Public ports

| Port | Proto | Service |
|---|---|---|
| 22 | tcp | SSH, key only |
| 80 | tcp | Caddy HTTP (redirects to HTTPS) |
| 443 | tcp | Caddy HTTPS / WSS |
| 40000-43999 | tcp | Game Relay WoW game traffic (Auth & World) |

## Filesystem layout

```
/opt/coa/
├── bin/          helper scripts (health.sh)
├── caddy/        compose.yaml, Caddyfile
├── postgres/     compose.yaml (running since Phase 10)
├── registry/     compose.yaml, .env, src/ (the build context of the running image)
├── coordinator/  compose.yaml, src/
├── relay/        compose.yaml, src/ (coa-relay container)
├── monitoring/   (empty)
├── backups/      0700, logical dumps and config archives
├── logs/
└── secrets/      0700 root:root, never in git
```

Config under `/opt/coa/*` is owned by `ubuntu`. Secrets live only in `/opt/coa/secrets` and are referenced from
compose files as Docker secrets/files, never inline.

## SSH procedure

```bash
ssh ubuntu@<VPS_IP>
```

- ED25519 key only; root login and password authentication are disabled (`/etc/ssh/sshd_config.d/00-coa-hardening.conf`).
- To change SSH settings: edit the drop-in, run `sudo sshd -t`, `sudo systemctl reload ssh`, and **test from a second
  terminal before closing the first one**.
- Lost key: use the OVH KVM console (or reinstall the VPS with a new key in the OVH reinstall dialog; this wipes data).

## Firewall policy

UFW, default deny incoming. Currently allowed: OpenSSH, 80/tcp, 443/tcp.

```bash
sudo ufw status verbose
```

Docker-published ports bypass UFW (see VPS_BASELINE.md). Check `sudo docker ps` and `sudo ss -tulnH` after every
deployment; the only public listeners must be 22, 80, 443.

fail2ban protects SSH (default `sshd` jail):

```bash
sudo fail2ban-client status sshd
```

## Update procedure

- Security updates install automatically (unattended-upgrades); no automatic reboot and no release upgrades.
- Manual full update (monthly, or after security advisories):

  ```bash
  sudo apt update && sudo apt full-upgrade -y
  test -f /var/run/reboot-required && echo "reboot needed"
  ```

  Reboot with `sudo systemctl reboot` and verify with `/opt/coa/bin/health.sh`.
- Containers: pin image tags, then `cd /opt/coa/<service> && sudo docker compose pull && sudo docker compose up -d`.
- Docker Engine updates come through the normal apt repository.

## Health check

```bash
/opt/coa/bin/health.sh
```

Shows uptime/load, disk, RAM, listening sockets, failed systemd units, UFW rules, fail2ban SSH bans and running
containers. No monitoring stack is installed on purpose (no Grafana/Prometheus until there is a real need).

## Deployment commands

```bash
# Caddy (running)
cd /opt/coa/caddy && sudo docker compose up -d
sudo docker compose logs --tail 50

# PostgreSQL and the Registry (Phase 10): see REGISTRY_DEPLOYMENT.md
cd /opt/coa/postgres && sudo docker compose up -d
cd /opt/coa/registry && sudo docker compose up -d

# Coordinator (Phase 12)
cd /opt/coa/coordinator && sudo docker compose up -d
```

## Backups

- Provider backup is not a substitute for application-level backups. What OVH actually includes for this plan
  (automatic backup, snapshot, KVM/rescue) still has to be confirmed in the Control Panel.
- Planned (not implemented yet):
  1. PostgreSQL logical backup: `pg_dump` to `/opt/coa/backups`, copied off the node.
  2. Infrastructure config backup: `/opt/coa` except `secrets/` and volumes.
  3. Secrets are backed up separately and never together with the config archive.
- Portable Characters are not backed up here: the VPS is not their source of truth.

## Recovery

| Situation | Path |
|---|---|
| SSH lockout / bad firewall | OVH Control Panel -> VPS -> KVM console, log in locally, fix `ufw`/sshd drop-in |
| Broken OS | OVH rescue mode, or Reinstall (Ubuntu 24.04 + pre-installed ED25519 key), then re-apply this document |
| Lost VPS | New VPS, follow VPS_BASELINE.md "What was configured", restore config archive and PostgreSQL dump |

Reinstall destroys all data on the VPS.
