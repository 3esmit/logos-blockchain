#!/bin/bash

# SQLite Zone Demo
# Runs sequencer and/or indexer without Docker (works on ARM Mac)
#
# Usage:
#   ./run-local.sh <service> --env-file /path/to/.env-local [--clean]
#
# Services:
#   sequencer  - Run only the sequencer
#   indexer   - Run only the indexer
#
# Examples:
#   ./run-local.sh sequencer --env-file ~/Eng/offsite-sequencer-env/.env-local
#   ./run-local.sh indexer --env-file ~/Eng/offsite-sequencer-env/.env-local
#
# Required env vars:
#   SEQUENCER_NODE_ENDPOINT      - LB node HTTP endpoint for sequencer
#   INDEXER_NODE_ENDPOINT       - LB node HTTP endpoint for indexer

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
DATA_DIR="$SCRIPT_DIR/data"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

# Parse service argument (first positional arg)
SERVICE="sequencer"
if [[ $# -gt 0 && ! "$1" =~ ^-- ]]; then
    SERVICE="$1"
    shift
fi

# Validate service
case $SERVICE in
    sequencer|indexer)
        ;;
    *)
        echo -e "${RED}Unknown service: $SERVICE${NC}"
        echo "Valid services: sequencer, indexer"
        exit 1
        ;;
esac

# Parse remaining arguments
ENV_FILE=""
CLEAN_START=false
while [[ $# -gt 0 ]]; do
    case $1 in
        --env-file)
            ENV_FILE="$2"
            shift 2
            ;;
        --clean)
            CLEAN_START=true
            shift
            ;;
        --sequencer-node-endpoint)
            SEQUENCER_NODE_ENDPOINT="$2"
            shift 2
            ;;
        --sequencer-node-auth-username)
            SEQUENCER_NODE_AUTH_USERNAME="$2"
            shift 2
            ;;
        --sequencer-node-auth-password)
            SEQUENCER_NODE_AUTH_PASSWORD="$2"
            shift 2
            ;;
        --sequencer-db-path)
            SEQUENCER_DB_PATH="$2"
            shift 2
            ;;
        --sequencer-signing-key-path)
            SEQUENCER_SIGNING_KEY_PATH="$2"
            shift 2
            ;;
        --queue-file)
            QUEUE_FILE="$2"
            shift 2
            ;;
        --checkpoint-path)
            CHECKPOINT_PATH="$2"
            shift 2
            ;;
        --sequencer-node-endpoint)
            INDEXER_NODE_ENDPOINT="$2"
            shift 2
            ;;
        --indexer-node-auth-username)
            INDEXER_NODE_AUTH_USERNAME="$2"
            shift 2
            ;;
        --indexer-node-auth-password)
            INDEXER_NODE_AUTH_PASSWORD="$2"
            shift 2
            ;;
        --channel-id)
            export CHANNEL_ID="$2"
            echo "hi"
            shift 2
            ;;
        --indexer-db-path)
            INDEXER_DB_PATH=="$2"
            shift 2
            ;;
        *)
            echo -e "${RED}Unknown option: $1${NC}"
            exit 1
            ;;
    esac
done

# Load env file if provided
if [ -n "$ENV_FILE" ]; then
    if [ -f "$ENV_FILE" ]; then
        echo -e "${BLUE}Loading environment from: $ENV_FILE${NC}"
        set -a
        source "$ENV_FILE"
        set +a
    else
        echo -e "${RED}Error: env file not found: $ENV_FILE${NC}"
        exit 1
    fi
fi

# Validate required env vars
missing_vars=()
[ -z "$SEQUENCER_NODE_ENDPOINT" ] && missing_vars+=("SEQUENCER_NODE_ENDPOINT")
[ -z "$INDEXER_NODE_ENDPOINT" ] && missing_vars+=("INDEXER_NODE_ENDPOINT")

if [ ${#missing_vars[@]} -ne 0 ]; then
    echo -e "${RED}Error: Missing required environment variables:${NC}"
    for var in "${missing_vars[@]}"; do
        echo "  - $var"
    done
    echo ""
    echo "See .env-local.example for the required format."
    exit 1
fi

# Clean data directory if requested
if [ "$CLEAN_START" = true ]; then
    echo -e "${YELLOW}Cleaning data directory...${NC}"
    rm -rf "$DATA_DIR"
fi

# Create data directory (needed for channel ID file)
mkdir -p "$DATA_DIR"

# Get local IP for sharing
LOCAL_IP=$(ipconfig getifaddr en0 2>/dev/null || hostname -I 2>/dev/null | awk '{print $1}' || echo "localhost")

# Check if binaries exist, if not build them
SEQUENCER_BIN="$REPO_ROOT/target/release/demo-sqlite-sequencer"
INDEXER_BIN="$REPO_ROOT/target/release/demo-sqlite-indexer"

if [[ "$SERVICE" == "sequencer" ]]; then
    echo -e "${YELLOW}Building sequencer...${NC}"
    cd "$REPO_ROOT"
    cargo build --release -p demo-sqlite-sequencer
fi

if [[ "$SERVICE" == "indexer" ]]; then
    echo -e "${YELLOW}Building indexer...${NC}"
    cd "$REPO_ROOT"
    cargo build --release -p demo-sqlite-indexer
fi

# Run the selected service(s)
case $SERVICE in
    sequencer)
        echo -e "${GREEN}Starting sequencer...${NC}"
        cd "$SCRIPT_DIR"
        exec "$SEQUENCER_BIN"
        ;;
    indexer)
        echo -e "${GREEN}Starting indexer...${NC}"
        cd "$SCRIPT_DIR"
        exec "$INDEXER_BIN"
        ;;
esac
