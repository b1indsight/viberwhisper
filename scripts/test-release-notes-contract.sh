#!/usr/bin/env bash
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
validator="${script_dir}/validate-release-contract.sh"
release_workflow="${script_dir}/../.github/workflows/release.yml"
fixture_dir=$(mktemp -d)
trap 'rm -rf "${fixture_dir}"' EXIT

valid_notes="${fixture_dir}/valid.md"
cat > "${valid_notes}" <<'EOF'
## Distribution notes

- The macOS app is ad-hoc signed, not Developer ID signed, and not notarized.
- Windows artifacts are not Authenticode signed.
- Packages support the API inference profile only; Local mode requires a source checkout.
- Validate downloads with `SHA256SUMS` and GitHub artifact provenance.
EOF

run_validator() {
  RELEASE_NOTES_FILE="$1" bash "${validator}"
}

if ! run_validator "${valid_notes}" > "${fixture_dir}/valid.out" 2>&1; then
  cat "${fixture_dir}/valid.out" >&2
  echo "Expected the complete release notes contract to pass." >&2
  exit 1
fi

assert_rejected() {
  local name=$1
  local expected_error=$2
  local notes_file=$3
  local output_file="${fixture_dir}/${name}.out"

  if run_validator "${notes_file}" > "${output_file}" 2>&1; then
    echo "Expected ${name} release notes to be rejected." >&2
    exit 1
  fi
  if ! grep -Fq "${expected_error}" "${output_file}"; then
    cat "${output_file}" >&2
    echo "Expected ${name} failure to contain: ${expected_error}" >&2
    exit 1
  fi
}

missing_notes="${fixture_dir}/missing.md"
empty_notes="${fixture_dir}/empty.md"
: > "${empty_notes}"
assert_rejected missing "Release notes header is missing or empty" "${missing_notes}"
assert_rejected empty "Release notes header is missing or empty" "${empty_notes}"

assert_marker_rejected() {
  local name=$1
  local expected_error=$2
  local old_text=$3
  local replacement=$4
  local notes_file="${fixture_dir}/${name}.md"

  sed "s/${old_text}/${replacement}/" "${valid_notes}" > "${notes_file}"
  assert_rejected "${name}" "${expected_error}" "${notes_file}"
}

# Each mutation models a plausible prose edit that would make published guidance incomplete.
assert_marker_rejected macos_ad_hoc "macOS ad-hoc signing" "ad-hoc signed" "locally signed"
assert_marker_rejected macos_developer_id "missing Developer ID signing" "not Developer ID signed" "Developer ID status unspecified"
assert_marker_rejected macos_notarization "missing notarization" "not notarized" "notarization status unspecified"
assert_marker_rejected windows_authenticode "missing Windows Authenticode signing" "not Authenticode signed" "Windows signing status unspecified"
assert_marker_rejected api_only "API-only packaged scope" "API inference profile only" "API inference profile"
assert_marker_rejected local_checkout "Local-mode source checkout requirement" "requires a source checkout" "requires a separate installation"
assert_marker_rejected checksums "SHA256SUMS verification" 'SHA256SUMS' 'checksums'
assert_marker_rejected provenance "GitHub artifact provenance" "GitHub artifact provenance" "build metadata"

create_block=$(sed -n '/create_args=(/,/gh release create/p' "${release_workflow}")
for required_command_part in \
  '--draft' \
  '--generate-notes' \
  '--notes-file .github/release-notes.md' \
  'gh release create'; do
  if ! grep -Fq -- "${required_command_part}" <<< "${create_block}"; then
    echo "Release creation is missing: ${required_command_part}" >&2
    exit 1
  fi
done

draft_line=$(grep -n -m 1 'gh release create' "${release_workflow}" | cut -d: -f1)
publish_line=$(grep -n -m 1 'gh release edit' "${release_workflow}" | cut -d: -f1)
if (( draft_line >= publish_line )); then
  echo "The complete draft must be created before it is published." >&2
  exit 1
fi

echo "Release notes contract fixtures passed."
