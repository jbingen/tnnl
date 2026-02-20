#!/bin/sh
set -e

REPO="soria-dev/tnnl"
INSTALL_DIR="/usr/local/bin"
BINARY="tnnl"

main() {
    os=$(uname -s | tr '[:upper:]' '[:lower:]')
    arch=$(uname -m)

    case "$arch" in
        x86_64|amd64) arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        *) echo "Unsupported architecture: $arch" && exit 1 ;;
    esac

    case "$os" in
        linux) target="${arch}-unknown-linux-musl" ;;
        darwin) target="${arch}-apple-darwin" ;;
        *) echo "Unsupported OS: $os" && exit 1 ;;
    esac

    if [ -z "$VERSION" ]; then
        VERSION=$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | grep '"tag_name"' | head -1 | cut -d'"' -f4)
    fi

    if [ -z "$VERSION" ]; then
        echo "Failed to determine latest version"
        exit 1
    fi

    url="https://github.com/${REPO}/releases/download/${VERSION}/tnnl-${target}.tar.gz"

    echo "Downloading tnnl ${VERSION} for ${target}..."

    tmpdir=$(mktemp -d)
    trap "rm -rf $tmpdir" EXIT

    curl -fsSL "$url" -o "${tmpdir}/tnnl.tar.gz"
    tar -xzf "${tmpdir}/tnnl.tar.gz" -C "$tmpdir"

    if [ -w "$INSTALL_DIR" ]; then
        mv "${tmpdir}/tnnl" "${INSTALL_DIR}/${BINARY}"
    else
        echo "Installing to ${INSTALL_DIR} (requires sudo)..."
        sudo mv "${tmpdir}/tnnl" "${INSTALL_DIR}/${BINARY}"
    fi

    chmod +x "${INSTALL_DIR}/${BINARY}"

    echo "tnnl ${VERSION} installed to ${INSTALL_DIR}/${BINARY}"
}

main
