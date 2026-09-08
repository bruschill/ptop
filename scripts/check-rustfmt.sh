#!/usr/bin/env bash
set -euo pipefail

base_ref="${1:-HEAD^}"
if ! git rev-parse --verify "${base_ref}^{commit}" >/dev/null 2>&1; then
  echo "invalid rustfmt base ref: ${base_ref}" >&2
  exit 2
fi

files=()
while IFS= read -r file; do
  [[ -n "${file}" ]] && files+=("${file}")
done < <(git diff --name-only --diff-filter=ACMRT "${base_ref}" HEAD -- '*.rs')

if [[ ${#files[@]} -eq 0 ]]; then
  echo "No changed Rust files to check."
  exit 0
fi

printf 'Checking rustfmt for %s changed Rust file(s).\n' "${#files[@]}"
rustfmt --edition 2021 --config skip_children=true --check "${files[@]}"
