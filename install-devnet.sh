#!/bin/bash

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Print functions
print_info() {
    echo -e "${GREEN}[INFO]${NC} $1"
}

print_warn() {
    echo -e "${YELLOW}[WARN]${NC} $1"
}

print_error() {
    echo -e "${RED}[ERROR]${NC} $1"
}

# Banner
echo "================================================"
echo "  Logos Blockchain Devnet Node Installer"
echo "================================================"
echo ""

# Check if .env file exists and load it
if [ -f ".env" ]; then
    print_info "Loading credentials from .env file..."
    export $(grep -v '^#' .env | xargs)
fi

# Prompt for credentials if not in environment
if [ -z "$DEVNET_USERNAME" ]; then
    read -p "Enter devnet username: " DEVNET_USERNAME
fi

if [ -z "$DEVNET_PASSWORD" ]; then
    read -sp "Enter devnet password: " DEVNET_PASSWORD
    echo ""
fi

# Detect platform
detect_platform() {
    local os=$(uname -s | tr '[:upper:]' '[:lower:]')
    local arch=$(uname -m)
    
    case "$os" in
        linux)
            case "$arch" in
                x86_64)
                    echo "linux-x86_64"
                    ;;
                aarch64|arm64)
                    echo "linux-aarch64"
                    ;;
                *)
                    print_error "Unsupported architecture: $arch"
                    exit 1
                    ;;
            esac
            ;;
        darwin)
            case "$arch" in
                arm64|aarch64)
                    echo "macos-aarch64"
                    ;;
                *)
                    print_error "Unsupported macOS architecture: $arch"
                    exit 1
                    ;;
            esac
            ;;
        *)
            print_error "Unsupported operating system: $os"
            exit 1
            ;;
    esac
}

PLATFORM=$(detect_platform)
print_info "Detected platform: $PLATFORM"

# Prompt for circuit release version
read -p "Enter circuit release version (e.g., v0.4.1): " CIRCUIT_RELEASE_VERSION
if [ -z "$CIRCUIT_RELEASE_VERSION" ]; then
    CIRCUIT_RELEASE_VERSION="v0.4.1"
    print_warn "No version specified, using default: $CIRCUIT_RELEASE_VERSION"
fi

# Prompt for release version
read -p "Enter release version (e.g., 0.1.1): " RELEASE_VERSION
if [ -z "$RELEASE_VERSION" ]; then
    RELEASE_VERSION="0.1.1"
    print_warn "No version specified, using default: $RELEASE_VERSION"
fi



# Construct download URLs
BINARY_NAME="logos-blockchain-node-${PLATFORM}-${RELEASE_VERSION}"
CIRCUITS_NAME="logos-blockchain-circuits-${CIRCUIT_RELEASE_VERSION}-${PLATFORM}.tar.gz"
GITHUB_RELEASE_BASE="https://github.com/logos-blockchain/logos-blockchain/releases/download/${RELEASE_VERSION}"

BINARY_URL="${GITHUB_RELEASE_BASE}/${BINARY_NAME}"
CIRCUITS_URL="${GITHUB_RELEASE_BASE}/${CIRCUITS_NAME}"

print_info "Binary URL: $BINARY_URL"
print_info "Circuits URL: $CIRCUITS_URL"

# Create devnet directory
print_info "Creating devnet directory..."
mkdir -p devnet
cd devnet

# Download binary
print_info "Downloading node binary..."
if ! wget -O "$BINARY_NAME" "$BINARY_URL"; then
    print_error "Failed to download binary. Please check the release version and try again."
    exit 1
fi

# Download circuits
print_info "Downloading circuits..."
if ! wget -O "$CIRCUITS_NAME" "$CIRCUITS_URL"; then
    print_error "Failed to download circuits. Please check the release version and try again."
    exit 1
fi

# Extract circuits
print_info "Extracting circuits..."
tar -xzf "$CIRCUITS_NAME"

# Move circuits to home directory
print_info "Installing circuits to ~/.logos-blockchain-circuits..."
CIRCUITS_DIR=$(tar -tzf "$CIRCUITS_NAME" | head -1 | cut -f1 -d"/")

print_info "Cleaning the nomos_circuits"
rm -rf "$HOME/.logos-blockchain-circuits/*"
cp -R "$CIRCUITS_DIR/" "$HOME/.logos-blockchain-circuits/"

# Make binary executable
print_info "Making binary executable..."
chmod +x "$BINARY_NAME"

# Prompt for network configuration
echo ""
#print_info "Network Configuration"
#read -p "Enter your IP address (default: 127.0.0.1): " NODE_IP
#NODE_IP=${NODE_IP:-127.0.0.1}

#read -p "Enter network port (default: 3000): " NETWORK_PORT
#NETWORK_PORT=${NETWORK_PORT:-3000}

#read -p "Enter blend port (default: 3400): " BLEND_PORT
#BLEND_PORT=${BLEND_PORT:-3400}

#read -p "Enter API port (default: 8080): " API_PORT
#API_PORT=${API_PORT:-8080}

#read -p "Enter node identifier (default: devnet-node): " NODE_IDENTIFIER
#NODE_IDENTIFIER=${NODE_IDENTIFIER:-devnet-node}

# Download deployment.yaml
#print_info "Downloading deployment.yaml..."
#if ! wget -O deployment.yaml https://raw.githubusercontent.com/logos-blockchain/logos-blockchain/refs/heads/testnet/testnet/deployment.yaml; then
#    print_error "Failed to download deployment.yaml"
#    exit 1
#fi

# Generate user config
print_info "Generating user configuration..."
if ! curl -X POST -L --location-trusted https://devnet.blockchain.logos.co/node/0/cfgsync/generate-config \
     -u "${DEVNET_USERNAME}:${DEVNET_PASSWORD}" \
     -H "Content-Type: application/json" \
     -d '{
           "ip": "192.168.4.2",
           "identifier": "not-essential-for-local-nodes",
           "network_port": 3000,
           "blend_port": 3400,
           "api_port": 8080
         }' \
     -o my_user_config.yaml; then
    print_error "Failed to generate user configuration"
    exit 1
fi

# Extract the non-voucher key
print_info "Extracting node key..."
ls
NODE_KEY=$(grep -A4 known_keys my_user_config.yaml | head -2 | tail -1 | awk '{print $2}')

if [ -z "$NODE_KEY" ]; then
    print_error "Failed to extract node key from configuration"
    exit 1
fi

# Create a helper run script
print_info "Creating run script..."
cat > run-node.sh <<EOF
#!/bin/bash
./${BINARY_NAME} my_user_config.yaml
EOF
chmod +x run-node.sh

# Create a helper script to check balance
print_info "Creating balance check script..."
cat > check-balance.sh <<EOF
#!/bin/bash
curl http://localhost:8080/wallet/${NODE_KEY}/balance
EOF
chmod +x check-balance.sh

# Generate systemd service unit
print_info "Generating systemd service unit..."
WORKING_DIR="$(pwd)"
SERVICE_NAME="logos-blockchain-devnet"

cat > ${SERVICE_NAME}.service <<EOF
[Unit]
Description=Logos Blockchain Devnet Node
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=${USER}
WorkingDirectory=${WORKING_DIR}
Environment="LOGOS_BLOCKCHAIN_CIRCUITS=${HOME}/.logos-blockchain-circuits"
ExecStart=${WORKING_DIR}/${BINARY_NAME} ${WORKING_DIR}/my_user_config.yaml
Restart=on-failure
RestartSec=10
StandardOutput=journal
StandardError=journal
SyslogIdentifier=logos-blockchain

# Security hardening
NoNewPrivileges=true
PrivateTmp=true
ProtectSystem=strict
ProtectHome=read-only
ReadWritePaths=${WORKING_DIR}
ReadWritePaths=${HOME}/.logos-blockchain-circuits

[Install]
WantedBy=multi-user.target
EOF

print_info "Systemd service unit created: ${SERVICE_NAME}.service"

# Installation complete
echo ""
echo "================================================"
print_info "Installation completed successfully!"
echo "================================================"
echo ""
echo "Your node key: ${NODE_KEY}"
echo ""
echo "Next steps:"
echo ""
echo "Option A - Run manually:"
echo "  1. Start your node:"
echo "     cd devnet"
echo "     ./run-node.sh"
echo ""
echo "Option B - Run as systemd service (recommended for persistent operation):"
echo "  1. Install the service:"
echo "     sudo cp devnet/${SERVICE_NAME}.service /etc/systemd/system/"
echo "     sudo systemctl daemon-reload"
echo ""
echo "  2. Enable and start the service:"
echo "     sudo systemctl enable ${SERVICE_NAME}"
echo "     sudo systemctl start ${SERVICE_NAME}"
echo ""
echo "  3. Check status:"
echo "     sudo systemctl status ${SERVICE_NAME}"
echo ""
echo "  4. View logs:"
echo "     sudo journalctl -u ${SERVICE_NAME} -f"
echo ""
echo "  5. Stop the service (if needed):"
echo "     sudo systemctl stop ${SERVICE_NAME}"
echo ""
echo "Funding your wallet:"
echo "  Go to: https://devnet.blockchain.logos.co/node/0/"
echo "  Username: ${DEVNET_USERNAME}"
echo "  Enter your node key: ${NODE_KEY}"
echo ""
echo "Check your wallet balance:"
echo "  cd devnet"
echo "  ./check-balance.sh"
echo ""
echo "Configuration files:"
echo "  - Node binary: devnet/${BINARY_NAME}"
echo "  - User config: devnet/my_user_config.yaml"
#echo "  - Deployment config: devnet/deployment.yaml"
echo "  - Circuits: ~/.logos-blockchain-circuits"
echo "  - Systemd service: devnet/${SERVICE_NAME}.service"
echo ""
echo "Helper scripts:"
echo "  - ./devnet/run-node.sh - Start the node manually"
echo "  - ./devnet/check-balance.sh - Check wallet balance"
echo ""
print_warn "Make sure to keep your my_user_config.yaml safe!"
echo "================================================"
