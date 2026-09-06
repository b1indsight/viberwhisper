#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
repository_root=$(cd "${script_dir}/.." && pwd)
cd "${repository_root}"

required_files=(
  .github/release-notes.md
  .github/workflows/release.yml
  Cargo.lock
  Cargo.toml
  LICENSE
  assets/Info.plist.ext
  assets/icon-32x32.png
  assets/icon-128x128.png
  assets/icon-256x256.png
  assets/icon.ico
  wix/main.wxs
  wix/upgrade-fixture.wxs
)

for required_file in "${required_files[@]}"; do
  if [[ ! -s "${required_file}" ]]; then
    echo "Required release input is missing or empty: ${required_file}" >&2
    exit 1
  fi
done

release_notes_file=${RELEASE_NOTES_FILE:-.github/release-notes.md}
if [[ ! -s "${release_notes_file}" ]]; then
  echo "Release notes header is missing or empty: ${release_notes_file}" >&2
  exit 1
fi
release_notes_text=$(tr '\n' ' ' < "${release_notes_file}")

require_release_notes_marker() {
  local description=$1
  local pattern=$2

  if ! grep -Eiq -- "${pattern}" <<< "${release_notes_text}"; then
    echo "Release notes header is missing required guidance: ${description}." >&2
    exit 1
  fi
}

require_release_notes_marker \
  "macOS ad-hoc signing" \
  'ad-hoc[[:space:]]+signed'
require_release_notes_marker \
  "missing Developer ID signing" \
  'not[[:space:]]+Developer[[:space:]]+ID[[:space:]]+signed'
require_release_notes_marker \
  "missing notarization" \
  'not[[:space:]]+notarized'
require_release_notes_marker \
  "missing Windows Authenticode signing" \
  'not[[:space:]]+Authenticode[[:space:]]+signed'
require_release_notes_marker \
  "OpenAI-compatible endpoint scope" \
  'OpenAI-compatible[[:space:]]+endpoints'
require_release_notes_marker \
  "localhost endpoint support" \
  'localhost'
require_release_notes_marker \
  "SHA256SUMS verification" \
  'SHA256SUMS'
require_release_notes_marker \
  "GitHub artifact provenance" \
  'GitHub[[:space:]]+artifact[[:space:]]+provenance'

metadata_file=$(mktemp)
trap 'rm -f "${metadata_file}"' EXIT
cargo metadata --locked --no-deps --format-version 1 > "${metadata_file}"
version=$(jq -r '.packages[] | select(.name == "viberwhisper") | .version' "${metadata_file}")
if [[ -z "${version}" || "${version}" == "null" ]]; then
  echo "Could not resolve the viberwhisper package version." >&2
  exit 1
fi

if [[ ! "${version}" =~ ^([0-9]+)\.([0-9]+)\.([0-9]+)$ ]]; then
  echo "Release versions must be stable numeric major.minor.patch values; got ${version}." >&2
  exit 1
fi

major=$((10#${BASH_REMATCH[1]}))
minor=$((10#${BASH_REMATCH[2]}))
patch=$((10#${BASH_REMATCH[3]}))
if (( major > 255 || minor > 255 || patch > 65535 )); then
  echo "Release version ${version} exceeds Windows Installer's 255.255.65535 limits." >&2
  exit 1
fi
if (( major == 0 && minor == 0 && patch == 0 )); then
  echo "Release version 0.0.0 is reserved for the frozen MSI upgrade fixture." >&2
  exit 1
fi

printf '%s\n' "${version}"
