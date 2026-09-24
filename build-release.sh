#!/usr/bin/env bash
# Build release binaries on macOS.
#
# Usage:
#   ./build-release.sh                # builds host arch only
#   ./build-release.sh intel          # x86_64-apple-darwin
#   ./build-release.sh arm            # aarch64-apple-darwin
#   ./build-release.sh universal      # both arches, then `lipo` fat binary
#   ./build-release.sh all            # both arches side by side, no lipo
#
# The output binary name is derived from Cargo.toml the same way the Windows
# `build-release.bat` does — prefer `[[bin]] name`, fall back to `[package] name`.

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CARGO_TOML="${SCRIPT_DIR}/Cargo.toml"
RELEASE_DIR="${SCRIPT_DIR}/releases"

if [[ ! -f "${CARGO_TOML}" ]]; then
    echo "[error] Cargo.toml not found at ${CARGO_TOML}" >&2
    exit 1
fi

# Resolve cargo (PATH first, then default rustup install).
CARGO="cargo"
if ! command -v cargo >/dev/null 2>&1; then
    if [[ -x "${HOME}/.cargo/bin/cargo" ]]; then
        CARGO="${HOME}/.cargo/bin/cargo"
    else
        echo "[error] cargo not found on PATH and ~/.cargo/bin/cargo missing." >&2
        exit 1
    fi
fi

# Read the binary name from Cargo.toml. Prefer [[bin]] name, then [package].
# Uses POSIX-portable awk (macOS ships BWK awk which lacks gawk's match(...,arr)).
extract_bin_name() {
    awk '
        BEGIN { section = "" }
        /^\[\[bin\]\]/  { section = "bin"; next }
        /^\[package\]/  { section = "pkg"; next }
        /^\[/           { section = "";    next }
        /^[[:space:]]*name[[:space:]]*=/ {
            line = $0
            sub(/^[^"]*"/, "", line)
            sub(/".*$/,     "", line)
            if (section == "bin" && bin == "") bin = line
            else if (section == "pkg" && pkg == "") pkg = line
        }
        END {
            if (bin != "") print bin
            else if (pkg != "") print pkg
        }
    ' "$1"
}

BIN_NAME="$(extract_bin_name "${CARGO_TOML}")"
if [[ -z "${BIN_NAME}" ]]; then
    echo "[error] failed to read binary name from Cargo.toml" >&2
    exit 1
fi

mode="${1:-host}"
mkdir -p "${RELEASE_DIR}"

ensure_target() {
    local triple="$1"
    if ! rustup target list --installed 2>/dev/null | grep -q "^${triple}$"; then
        echo "[deps]  installing rustup target ${triple}" >&2
        rustup target add "${triple}" >&2
    fi
}

build_one() {
    local triple="$1"
    local out_name="$2"
    ensure_target "${triple}"
    echo "[build] ${CARGO} build --release --target ${triple}" >&2
    (cd "${SCRIPT_DIR}" && "${CARGO}" build --release --target "${triple}") >&2
    local built="${SCRIPT_DIR}/target/${triple}/release/${BIN_NAME}"
    if [[ ! -f "${built}" ]]; then
        echo "[error] expected output missing: ${built}" >&2
        return 1
    fi
    local out="${RELEASE_DIR}/${out_name}"
    cp -f "${built}" "${out}"
    echo "[copy]  ${built} -> ${out}" >&2
    printf '%s\n' "${out}"
}

case "${mode}" in
    host)
        echo "[build] ${CARGO} build --release"
        (cd "${SCRIPT_DIR}" && "${CARGO}" build --release)
        built="${SCRIPT_DIR}/target/release/${BIN_NAME}"
        if [[ ! -f "${built}" ]]; then
            echo "[error] expected output missing: ${built}" >&2
            exit 1
        fi
        out="${RELEASE_DIR}/${BIN_NAME}"
        cp -f "${built}" "${out}"
        echo "[copy]  ${built} -> ${out}"
        echo "[done]  release binary available at ${out}"
        ;;
    intel|x86_64)
        out="$(build_one "x86_64-apple-darwin" "${BIN_NAME}-macos-x86_64")"
        echo "[done]  release binary available at ${out}"
        ;;
    arm|arm64|aarch64)
        out="$(build_one "aarch64-apple-darwin" "${BIN_NAME}-macos-arm64")"
        echo "[done]  release binary available at ${out}"
        ;;
    all)
        out_x86="$(build_one "x86_64-apple-darwin" "${BIN_NAME}-macos-x86_64")"
        out_arm="$(build_one "aarch64-apple-darwin" "${BIN_NAME}-macos-arm64")"
        echo "[done]  release binaries:"
        echo "        ${out_x86}"
        echo "        ${out_arm}"
        ;;
    universal|fat)
        out_x86="$(build_one "x86_64-apple-darwin" "${BIN_NAME}-macos-x86_64")"
        out_arm="$(build_one "aarch64-apple-darwin" "${BIN_NAME}-macos-arm64")"
        out_universal="${RELEASE_DIR}/${BIN_NAME}-macos-universal"
        echo "[lipo]  ${out_x86} + ${out_arm} -> ${out_universal}"
        lipo -create -output "${out_universal}" "${out_x86}" "${out_arm}"
        lipo -info "${out_universal}"
        echo "[done]  universal binary available at ${out_universal}"
        ;;
    *)
        echo "[error] unknown mode: ${mode}" >&2
        echo "usage: $0 [host|intel|arm|all|universal]" >&2
        exit 2
        ;;
esac
