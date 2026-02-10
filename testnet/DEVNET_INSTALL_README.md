# Logos Blockchain Devnet Node Installation

This directory contains an automated installation script for setting up a Logos Blockchain devnet node.

## Quick Start

### Option 1: Using Environment Variables

1. Copy the example environment file:
   ```bash
   cp .env.devnet.example .env
   ```

2. Edit `.env` and add your credentials:
   ```bash
   LB_DEVNET_USERNAME=your_username
   LB_DEVNET_PASSWORD=your_password
   ```

3. Run the installation script:
   ```bash
   chmod +x scripts/install-devnet.sh
   ./scripts/install-devnet.sh
   ```

### Option 2: Interactive Installation

Simply run the script and it will prompt you for credentials:
```bash
chmod +x scripts/install-devnet.sh
./scripts/install-devnet.sh
```

## What the Script Does

The installation script automatically:

1. **Detects your platform** (linux-x86_64, linux-aarch64, or macos-aarch64)
2. **Prompts for release versions** (circuit release and node release)
3. **Downloads the node binary** from GitHub releases
4. **Downloads and extracts circuits** to `~/.logos-blockchain-circuits`
5. **Generates your user configuration** via the devnet API
6. **Extracts your node key** for wallet operations
7. **Creates helper scripts** for running the node, checking balance, and transferring funds
8. **Generates a systemd service unit** for running the node as a system service

## Directory Structure

After installation, you'll have:

```
devnet/
├── logos-blockchain-node-<platform>-<version>       # Node binary
├── logos-blockchain-circuits-<version>-<platform>.tar.gz  # Circuits archive
├── config.yaml                              # Your node configuration
├── run-node.sh                                      # Helper script to start node
├── check-balance.sh                                 # Helper script to check balance
├── transfer-funds.sh                                # Helper script to transfer funds
└── logos-blockchain-devnet.service                  # Systemd service unit
```

Additionally, circuits are installed to:
```
~/.logos-blockchain-circuits/                        # Circuit files
```

## Running Your Node

### Option A: Run Manually

```bash
cd devnet
./run-node.sh
```

Or manually:
```bash
cd devnet
./logos-blockchain-node-<platform>-<version> config.yaml
```

### Option B: Run as Systemd Service (Recommended)

For persistent operation, install and run as a systemd service:

1. **Install the service:**
   ```bash
   sudo cp devnet/logos-blockchain-devnet.service /etc/systemd/system/
   sudo systemctl daemon-reload
   ```

2. **Enable and start the service:**
   ```bash
   sudo systemctl enable logos-blockchain-devnet
   sudo systemctl start logos-blockchain-devnet
   ```

3. **Check status:**
   ```bash
   sudo systemctl status logos-blockchain-devnet
   ```

4. **View logs:**
   ```bash
   sudo journalctl -u logos-blockchain-devnet -f
   ```

5. **Stop the service (if needed):**
   ```bash
   sudo systemctl stop logos-blockchain-devnet
   ```

### Fund Your Wallet

1. Go to https://devnet.blockchain.logos.co/node/0/
2. Log in with your devnet credentials
3. Enter your node key (displayed at the end of installation)
4. Submit to receive test tokens

### Check Your Balance

```bash
cd devnet
./check-balance.sh
```

Or manually:
```bash
curl http://localhost:8080/wallet/<your-node-key>/balance
```

### Transfer Funds

To transfer funds to another wallet:

```bash
cd devnet
./transfer-funds.sh <recipient_public_key> <amount>
```

Example:
```bash
./transfer-funds.sh 74e5dffeceaf4c825f9db77bcc7722e08e76a5fd1bd5611351869bed575e1419 2
```

The script accepts optional parameters for change and funding keys:
```bash
./transfer-funds.sh <recipient_public_key> <amount> [change_public_key] [funding_public_key]
```

If not specified, your node key will be used for both change and funding.

## Configuration Options

During installation, you'll be prompted for:

- **Release Version**: The version to download (e.g., 0.1.0)
- **IP Address**: Your node's IP (default: 127.0.0.1)
- **Network Port**: P2P network port (default: 3000)
- **Blend Port**: Blend network port (default: 3400)
- **API Port**: HTTP API port (default: 8080)
- **Node Identifier**: A friendly name for your node

## Supported Platforms

- Linux x86_64
- Linux aarch64 (Raspberry Pi 5, ARM servers)
- macOS aarch64 (Apple Silicon)

## Troubleshooting

### Download Fails

If the download fails, verify:
- The release version exists on GitHub
- You have internet connectivity
- The release includes binaries for your platform

### Configuration Generation Fails

If config generation fails:
- Verify your devnet credentials are correct
- Check that the devnet API is accessible
- Ensure you have network connectivity

### Node Won't Start

If the node fails to start:
- Check that all ports are available (not in use)
- Verify circuits are installed in `~/.logos-blockchain-circuits`
- Check the node logs for specific errors

### Balance Shows Zero

If your balance is zero after funding:
- Wait a few moments for block propagation
- Verify you entered the correct node key on the faucet
- Check that your node is running and connected

## Security Notes

- **Never commit your `.env` file** to version control
- Keep your `config.yaml` secure (contains private keys)
- The `.env.devnet.example` file is safe to commit (no credentials)

## Manual Installation

If you prefer to install manually, follow the steps in the original how-to document or review the [`scripts/install-devnet.sh`](../scripts/install-devnet.sh) script to see each command.

## Getting Help

For issues or questions:
- Check the [GitHub repository](https://github.com/logos-blockchain/logos-blockchain)
- Review the testnet documentation
- Contact the development team

## Files

- [`scripts/install-devnet.sh`](../scripts/install-devnet.sh) - Main installation script
- [`.env.devnet.example`](.env.devnet.example) - Environment variables template
- `DEVNET_INSTALL_README.md` - This file
