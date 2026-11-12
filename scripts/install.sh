#!/usr/bin/env bash
set -euo pipefail

echo "=================================================="
echo "⚡ Installing Siphon Download Manager & Daemon"
echo "=================================================="

BIN_DIR="${HOME}/.local/bin"
DATA_DIR="${HOME}/.local/share/siphon"
SYSTEMD_USER_DIR="${HOME}/.config/systemd/user"
DOWNLOAD_DIR="${HOME}/Downloads"

mkdir -p "${BIN_DIR}" "${DATA_DIR}" "${SYSTEMD_USER_DIR}" "${DOWNLOAD_DIR}"

# Check for Rust/Cargo
if command -v cargo &>/dev/null; then
    echo "⚙ Building release binaries with cargo..."
    cargo build --release -p siphond -p siphon
    install -m 755 target/release/siphond "${BIN_DIR}/siphond"
    install -m 755 target/release/siphon "${BIN_DIR}/siphon"
else
    echo "✘ Cargo not found. Please install Rust or use prebuilt binaries."
    exit 1
fi

# Install Systemd user service
echo "⚙ Installing systemd user service..."
cp scripts/siphon.service "${SYSTEMD_USER_DIR}/siphon.service"

if command -v systemctl &>/dev/null; then
    systemctl --user daemon-reload
    systemctl --user restart siphon.service || systemctl --user enable --now siphon.service
    echo "✔ siphond systemd user service active and up to date!"
    echo ""
    echo "💡 Home Server Pro-Tip:"
    echo "   To ensure downloads continue running overnight on your home server"
    echo "   after you disconnect your SSH session, run:"
    echo "   sudo loginctl enable-linger ${USER}"
fi

echo ""
echo "=================================================="
echo "✔ Siphon installation completed successfully!"
echo "  Run 'siphon' to launch the interactive TUI"
echo "  Run 'siphon --help' for CLI automation commands"
echo "=================================================="
