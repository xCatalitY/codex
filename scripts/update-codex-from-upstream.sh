#!/usr/bin/env bash

set -euo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/update-codex-from-upstream.sh [OPTIONS]

Fetch upstream, rebase the current branch onto it, rebuild codex-cli, and show
the active codex binary/version.

Options:
  --upstream REF    Rebase onto REF instead of origin/main.
  --autostash       Let git temporarily stash tracked local changes during rebase.
  --test-core       Run `just test -p codex-core` after the rebuild.
  --no-build        Only fetch/rebase; skip the cargo rebuild and version check.
  -h, --help        Show this help.

Examples:
  scripts/update-codex-from-upstream.sh
  scripts/update-codex-from-upstream.sh --autostash
  scripts/update-codex-from-upstream.sh --upstream origin/main --test-core
EOF
}

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd -P)"
repo_root="$(cd "${script_dir}/.." && pwd -P)"

upstream_ref="origin/main"
autostash=false
run_core_tests=false
build=true

while [[ $# -gt 0 ]]; do
  case "$1" in
    --upstream)
      if [[ $# -lt 2 ]]; then
        echo "--upstream requires a ref" >&2
        exit 2
      fi
      upstream_ref="$2"
      shift 2
      ;;
    --autostash)
      autostash=true
      shift
      ;;
    --test-core)
      run_core_tests=true
      shift
      ;;
    --no-build)
      build=false
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown option: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if [[ "${build}" == false && "${run_core_tests}" == true ]]; then
  echo "--test-core cannot be combined with --no-build" >&2
  exit 2
fi

need_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "$1 is required" >&2
    exit 1
  fi
}

has_tracked_changes() {
  ! git diff --quiet || ! git diff --cached --quiet
}

print_step() {
  printf '\n==> %s\n' "$1"
}

need_command git
if [[ "${build}" == true ]]; then
  need_command cargo
  need_command realpath
fi
if [[ "${run_core_tests}" == true ]]; then
  need_command just
fi

cd "${repo_root}"

if [[ ! -d codex-rs ]]; then
  echo "expected codex-rs under ${repo_root}" >&2
  exit 1
fi

branch="$(git branch --show-current)"
if [[ -z "${branch}" ]]; then
  echo "refusing to update from detached HEAD" >&2
  exit 1
fi

remote="${upstream_ref%%/*}"
if [[ "${remote}" == "${upstream_ref}" ]]; then
  echo "upstream ref must include a remote, for example origin/main" >&2
  exit 2
fi

if ! git remote get-url "${remote}" >/dev/null 2>&1; then
  echo "remote ${remote} does not exist" >&2
  exit 1
fi

if has_tracked_changes && [[ "${autostash}" == false ]]; then
  echo "tracked local changes are present; commit/stash them or rerun with --autostash" >&2
  git status --short
  exit 1
fi

print_step "Fetching ${remote}"
git fetch "${remote}"

if ! git rev-parse --verify --quiet "${upstream_ref}^{commit}" >/dev/null; then
  echo "upstream ref ${upstream_ref} does not resolve to a commit" >&2
  exit 1
fi

before_head="$(git rev-parse --short HEAD)"
upstream_head="$(git rev-parse --short "${upstream_ref}")"
read -r ahead behind < <(git rev-list --left-right --count "HEAD...${upstream_ref}")

echo "Branch: ${branch}"
echo "HEAD: ${before_head}"
echo "Upstream: ${upstream_ref} (${upstream_head})"
echo "Divergence before rebase: ahead ${ahead}, behind ${behind}"

print_step "Rebasing ${branch} onto ${upstream_ref}"
rebase_args=()
if [[ "${autostash}" == true ]]; then
  rebase_args+=(--autostash)
fi

if ! git rebase "${rebase_args[@]}" "${upstream_ref}"; then
  echo "rebase failed; resolve conflicts, then run: git rebase --continue" >&2
  exit 1
fi

after_head="$(git rev-parse --short HEAD)"
read -r ahead_after behind_after < <(git rev-list --left-right --count "HEAD...${upstream_ref}")
echo "HEAD after rebase: ${after_head}"
echo "Divergence after rebase: ahead ${ahead_after}, behind ${behind_after}"

if [[ "${build}" == false ]]; then
  print_step "Skipping rebuild"
  exit 0
fi

print_step "Building codex-cli"
(
  cd "${repo_root}/codex-rs"
  cargo build -p codex-cli
)

built_codex="${repo_root}/codex-rs/target/debug/codex"
if [[ ! -x "${built_codex}" ]]; then
  echo "build completed, but ${built_codex} is not executable" >&2
  exit 1
fi

print_step "Built binary"
echo "${built_codex}"
"${built_codex}" --version

active_codex="$(command -v codex || true)"
if [[ -n "${active_codex}" ]]; then
  active_real="$(realpath "${active_codex}")"
  built_real="$(realpath "${built_codex}")"
  echo "Active codex: ${active_real}"
  if [[ "${active_real}" != "${built_real}" ]]; then
    echo "warning: shell codex does not point at the rebuilt binary" >&2
  fi
else
  echo "warning: codex is not on PATH" >&2
fi

if [[ "${run_core_tests}" == true ]]; then
  print_step "Running codex-core tests"
  (
    cd "${repo_root}"
    just test -p codex-core
  )
fi
