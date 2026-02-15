#!/bin/sh
set -eu

REPO="superhq-ai/neko"
INSTALL_DIR="${NEKO_INSTALL_DIR:-$HOME/.local/bin}"

main() {
    detect_platform
    get_version
    download
    verify_checksum
    install_binary
    print_success
}

detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux)  OS="linux" ;;
        Darwin) OS="darwin" ;;
        *)
            echo "Error: unsupported OS: $OS" >&2
            exit 1
            ;;
    esac

    case "$ARCH" in
        x86_64|amd64)  ARCH="x86_64" ;;
        aarch64|arm64) ARCH="aarch64" ;;
        *)
            echo "Error: unsupported architecture: $ARCH" >&2
            exit 1
            ;;
    esac

    if [ -n "${NEKO_TARGET:-}" ]; then
        TARGET="$NEKO_TARGET"
    elif [ "$OS" = "linux" ]; then
        TARGET="${ARCH}-unknown-linux-gnu"
    elif [ "$OS" = "darwin" ]; then
        TARGET="${ARCH}-apple-darwin"
    fi

    echo "Detected platform: $TARGET"
}

get_version() {
    if [ -n "${NEKO_VERSION:-}" ]; then
        VERSION="$NEKO_VERSION"
    else
        VERSION="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
            | grep '"tag_name"' | cut -d '"' -f 4)"
    fi

    if [ -z "$VERSION" ]; then
        echo "Error: could not determine latest version" >&2
        exit 1
    fi

    echo "Installing neko $VERSION"
}

download() {
    TMPDIR="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR"' EXIT

    TARBALL="neko-${TARGET}.tar.gz"
    CHECKSUM="${TARBALL}.sha256"
    BASE_URL="https://github.com/${REPO}/releases/download/${VERSION}"

    echo "Downloading ${TARBALL}..."
    curl -fsSL "${BASE_URL}/${TARBALL}" -o "${TMPDIR}/${TARBALL}"
    curl -fsSL "${BASE_URL}/${CHECKSUM}" -o "${TMPDIR}/${CHECKSUM}"
}

verify_checksum() {
    echo "Verifying checksum..."
    cd "$TMPDIR"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum -c "$CHECKSUM"
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 -c "$CHECKSUM"
    else
        echo "Warning: no sha256 tool found, skipping checksum verification" >&2
    fi
    cd - >/dev/null
}

install_binary() {
    mkdir -p "$INSTALL_DIR"
    tar -xzf "${TMPDIR}/${TARBALL}" -C "$TMPDIR"
    mv "${TMPDIR}/neko" "${INSTALL_DIR}/neko"
    chmod +x "${INSTALL_DIR}/neko"
    echo "Installed neko to ${INSTALL_DIR}/neko"
}

print_success() {
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            echo ""
            echo "Add neko to your PATH by adding this to your shell profile:"
            echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
            ;;
    esac

    echo ""
    echo "Run 'neko --help' to get started."
}

main
