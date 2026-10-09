# VPS baseline (Phase -1)

Snapshot of the infrastructure node as configured at the end of Phase -1. No secrets, no IP addresses
(the public IP is `<VPS_IP>` throughout; it is kept out of git on purpose).

Date configured: 2026-10-06 (UTC)

## Inventory

| Item | Value |
|---|---|
| Provider | OVHcloud VPS |
| Hostname | `coa-infra-01` |
| Datacenter / region | _TODO: fill from OVH Control Panel (not visible from inside the VM)_ |
| Plan / service ID | _TODO: from OVH Control Panel_ |
| OS | Ubuntu 24.04.5 LTS (noble), kernel 6.8.0-146-generic |
| CPU | 2 vCPU, Intel Haswell-class (KVM guest) |
| RAM | 3.7 GiB |
| Disk | 38 GB root (`/dev/sda1`), ~2.9 GB used after setup |
| IPv4 | 1 public address, `<VPS_IP>` |
| IPv6 | 1 public /128 address on `ens3` |
| Timezone | UTC |
| Network limit | _TODO: from OVH plan_ |
| Provider backup | _TODO: see "Open items"_ |

## What was configured

- OS fully upgraded (`apt full-upgrade`), rebooted onto the new kernel.
- Base packages: curl, wget, git, ca-certificates, gnupg, jq, unzip, htop, ufw, fail2ban, unattended-upgrades.
- Hostname `coa-infra-01`, timezone UTC.
- SSH: key-only, no root login (details below).
- UFW: default deny incoming, allow outgoing; only 22/tcp, 80/tcp, 443/tcp allowed.
- fail2ban: default `sshd` jail only.
- unattended-upgrades: enabled for the `-security` pockets (Ubuntu, ESM apps, ESM infra); no automatic reboot,
  no release upgrades.
- Docker Engine from the official Docker apt repository (docker-ce, docker-ce-cli, containerd.io,
  docker-buildx-plugin, docker-compose-plugin).
- `/opt/coa` layout, Caddy running as the future ingress, PostgreSQL definition prepared but not started.

## Public ports (verified from an external machine)

| Port | Proto | Service |
|---|---|---|
| 22 | tcp | sshd (key only) |
| 80 | tcp | Caddy (redirects HTTP to HTTPS on 443) |
| 443 | tcp | Caddy (Let's Encrypt TLS: `/registry/*` and `/coord/*`) |

Everything else is closed, including 5432, 3306/3307, 2375/2376 (Docker API), 2019 (Caddy admin; admin API is disabled).

## SSH

- Login: `ssh ubuntu@<VPS_IP>` with an ED25519 key. The key is pre-installed via the OVH reinstall dialog.
- OVH sets the `ubuntu` password as expired on first install; it had to be changed once interactively
  (the emailed one-time password). The password is only used for `sudo` fallback; `ubuntu` also has passwordless sudo
  from the cloud image.
- Hardening drop-in: `/etc/ssh/sshd_config.d/00-coa-hardening.conf`:

  ```
  PermitRootLogin no
  PasswordAuthentication no
  KbdInteractiveAuthentication no
  PubkeyAuthentication yes
  ```

  **The file is `00-` and not `99-` on purpose.** sshd uses the first value it finds, and the cloud image ships
  `50-cloud-init.conf` with `PasswordAuthentication yes`; a `99-` file would be silently ignored.
- Ubuntu 24.04 starts sshd via socket activation (`ssh.socket`); that is expected.
- The SSH port is unchanged (22).
- Gate checked from a fresh session: key login works, `sudo` works, `root` and password login are refused.

## Firewall

```
Default: deny (incoming), allow (outgoing)
22/tcp   OpenSSH   ALLOW IN (v4+v6)
80/tcp             ALLOW IN (v4+v6)
443/tcp            ALLOW IN (v4+v6)
```

**Docker caveat:** ports published by Docker (`ports:` in a compose file) are inserted into iptables ahead of UFW and
are reachable from the internet even if UFW has no rule for them. Rule for this host: never publish a port other than
Caddy's 80/443; services talk to each other over Docker networks, and anything that must be reachable only locally is
published as `127.0.0.1:<port>:<port>`.

## Docker

- Docker Engine 29.8.2, Compose v5.6.0.
- The `ubuntu` user is **not** in the `docker` group; use `sudo docker ...`. (Membership in `docker` is
  root-equivalent, so it is deliberately not granted.)
- The Docker socket is not published anywhere; no TCP daemon listener.
- Networks:
  - `coa-ingress` (bridge): Caddy and future Registry API/Relay-facing HTTP services.
  - `coa-db` (`--internal`): PostgreSQL only; has no route to or from the outside.

## Resource baseline (right after setup, Caddy running)

- Load average ~0.6 shortly after boot, then idle.
- RAM: ~0.5 GiB used of 3.7 GiB (3.2 GiB available).
- Disk: 2.9 GB of 38 GB used.

## Phase 10 addendum

PostgreSQL 17 and the Registry now run (containers `coa-postgres-postgres-1`, `coa-registry-registry-1`); Caddy proxies `/registry/*` to the Registry. Public ports are unchanged (22, 80; 443 is
allowed and published but has no listener until TLS). Details and the gate: [REGISTRY_DEPLOYMENT.md](REGISTRY_DEPLOYMENT.md).

## Phase 12 & TLS gate addendum

The Coordinator (`coa-coordinator-coordinator-1`) runs behind Caddy on `coa-ingress` with no published port. Domain `coa-manager.duckdns.org` is pointed to the VPS, and Caddy manages automated Let's Encrypt TLS certificates on port 443 with persistent `/data` storage across container restarts. Port 80 automatically redirects to HTTPS. Public endpoints are `https://coa-manager.duckdns.org/registry/*` and `wss://coa-manager.duckdns.org/coord/*`. Verified by 13-item external verification gate (see [REGISTRY_DEPLOYMENT.md](REGISTRY_DEPLOYMENT.md) and [CONTROL_PROTOCOL.md](CONTROL_PROTOCOL.md)).

## Open items

- Fill in region, plan/service ID, network limit from the OVH Control Panel.
- Find out what OVH backup/snapshot is actually enabled for this plan (see INFRASTRUCTURE.md, Backups).
- [RESOLVED] Decide on the public hostname/TLS strategy: configured `coa-manager.duckdns.org` with Caddy automated Let's Encrypt TLS on 443.
