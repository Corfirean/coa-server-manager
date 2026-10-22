#!/bin/sh
# Build the Coordinator image on the node from the working tree and (re)start it.  Usage (from the repository root):
#   VPS=ubuntu@<VPS_IP> TAG=p12a sh services/coa-coordinator/deploy/deploy.sh
# The database role must exist (provision-db.sh) and /opt/coa/coordinator/compose.yaml must be in place (copied here on the first run).
set -eu
: "${VPS:?set VPS=user@host}" "${TAG:?set TAG=<image tag>}"
tar --exclude=target -czf /tmp/coordinator-src.tgz crates/coa-registry-proto crates/coa-control-proto services/coa-coordinator .dockerignore
scp -q /tmp/coordinator-src.tgz "$VPS:/tmp/coordinator-src.tgz"
scp -q services/coa-coordinator/deploy/coordinator.compose.yaml "$VPS:/tmp/coordinator.compose.yaml"
scp -q services/coa-coordinator/deploy/provision-db.sh "$VPS:/tmp/coordinator-provision-db.sh"
ssh "$VPS" "set -e
sudo mkdir -p /opt/coa/coordinator && sudo cp /tmp/coordinator.compose.yaml /opt/coa/coordinator/compose.yaml
sudo sh /tmp/coordinator-provision-db.sh
sudo rm -rf /opt/coa/coordinator/src && sudo mkdir -p /opt/coa/coordinator/src && sudo tar -xzf /tmp/coordinator-src.tgz -C /opt/coa/coordinator/src
cd /opt/coa/coordinator/src && sudo docker build -q -f services/coa-coordinator/Dockerfile -t coa-coordinator:$TAG . >/dev/null
cd /opt/coa/coordinator && echo COORDINATOR_TAG=$TAG | sudo tee .env >/dev/null && sudo docker compose up -d
sleep 8 && sudo docker ps --format '{{.Names}} {{.Status}}'"
