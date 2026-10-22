#!/bin/sh
# Build the Registry image on the node from the working tree and (re)start the stack.  Usage (from the repository root):
#   VPS=ubuntu@<VPS_IP> TAG=p10c sh services/coa-registry/deploy/deploy.sh
# Secrets are never touched here: /opt/coa/secrets/{postgres_password,registry_db_password} must already exist (see docs/REGISTRY_DEPLOYMENT.md).
set -eu
: "${VPS:?set VPS=user@host}" "${TAG:?set TAG=<image tag>}"
tar --exclude=target -czf /tmp/registry-src.tgz crates/coa-registry-proto services/coa-registry .dockerignore
scp -q /tmp/registry-src.tgz "$VPS:/tmp/registry-src.tgz"
ssh "$VPS" "set -e
sudo rm -rf /opt/coa/registry/src && mkdir -p /opt/coa/registry/src && tar -xzf /tmp/registry-src.tgz -C /opt/coa/registry/src
cd /opt/coa/registry/src && sudo docker build -q -f services/coa-registry/Dockerfile -t coa-registry:$TAG . >/dev/null
cd /opt/coa/registry && sudo sed -i 's/^REGISTRY_TAG=.*/REGISTRY_TAG=$TAG/' .env && sudo docker compose up -d
sleep 8 && sudo docker ps --format '{{.Names}} {{.Status}}'"
