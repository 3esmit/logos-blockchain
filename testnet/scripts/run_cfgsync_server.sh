#!/bin/sh

set -e

export CFG_SERVER_MODE="run" \
       ENTROPY_FILE="${ENTROPY_FILE:-/etc/logos-blockchain/entropy}" \
       CFG_SERVER_STORAGE_PATH="/node-data/cfgsync/deployment-settings.yaml"

exec /usr/bin/logos-blockchain-cfgsync-server /etc/logos-blockchain/cfgsync.yaml
