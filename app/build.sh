#!/bin/bash
# Builds promptmaxxer.app (release). Log: ~/Library/Logs/promptmaxxer-build.log.
# ./build.sh       release .app bundle: src-tauri/target/release/bundle/macos/promptmaxxer.app
# ./build.sh dev   debug binary only, faster
# Signing: APPLE_SIGNING_IDENTITY if set (a Developer ID, or any code-signing identity in your login keychain;
# list them with `security find-identity -p codesigning`), else ad-hoc ("-").
# Ad-hoc signing gives every build a new identity, so macOS asks for Accessibility again after each rebuild.
# A stable self-signed identity avoids that: see "Signing" in the README.
# This script never creates, imports or trusts a certificate.
set -euo pipefail
cd "$(dirname "$0")/src-tauri"
export PATH="$HOME/.rustup/toolchains/stable-aarch64-apple-darwin/bin:/opt/homebrew/opt/rustup/bin:$HOME/.cargo/bin:$PATH"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-4}" CARGO_INCREMENTAL=0 CARGO_PROFILE_RELEASE_DEBUG=0
export APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:--}"
LOG="$HOME/Library/Logs/promptmaxxer-build.log"
echo "== $(date -u +%Y-%m-%dT%H:%M:%SZ) build ${1:-release}, signing: $APPLE_SIGNING_IDENTITY" >> "$LOG"
if [ "${1:-}" = "dev" ]; then cargo build 2>&1 | tee -a "$LOG"; else cargo tauri build --bundles app 2>&1 | tee -a "$LOG"; fi
