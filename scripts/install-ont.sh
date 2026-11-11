#!/bin/bash
# Install, check or roll back the `ont` binary. No schema is installed and
# no index is run here; install prints those commands for the host step.
set -euo pipefail

REAL_BIN_DIR=/usr/local/bin
REAL_STATE_DIR=/var/lib/ontology-platform
CARGO_LOCK=/home/palantirkc/work/ws-replan/wt/.host-cargo.lock
SCRIPT_PATH=$(cd -- "$(dirname -- "$0")" && pwd -P)/$(basename -- "$0")
REPO_ROOT=$(cd -- "$(dirname -- "$0")/.." && pwd -P)

BIN_DIR=${ONT_INSTALL_BIN_DIR:-$REAL_BIN_DIR}
STATE_DIR=${ONT_INSTALL_STATE_DIR:-$REAL_STATE_DIR}
# Strip one trailing slash for the equality guard below.
BIN_DIR=${BIN_DIR%/}
STATE_DIR=${STATE_DIR%/}
[ -n "$BIN_DIR" ] || BIN_DIR=/
[ -n "$STATE_DIR" ] || STATE_DIR=/

DRY_RUN=0
ARGS=()
for arg in "$@"; do
  if [ "$arg" = "--dry-run" ]; then
    DRY_RUN=1
  else
    ARGS+=("$arg")
  fi
done
set -- ${ARGS[@]+"${ARGS[@]}"}

refuse() {
  printf 'refused: %s\n' "$1" >&2
  exit 2
}

# Test overrides are accepted only together, under the test flag, and
# never pointing at the real directories.
if [ "${ONT_INSTALL_TEST:-}" != "1" ]; then
  if [ -n "${ONT_INSTALL_BIN_DIR:-}" ] || [ -n "${ONT_INSTALL_STATE_DIR:-}" ] || [ -n "${ONT_INSTALL_PREBUILT:-}" ]; then
    refuse "ONT_INSTALL_BIN_DIR, ONT_INSTALL_STATE_DIR and ONT_INSTALL_PREBUILT require ONT_INSTALL_TEST=1"
  fi
else
  if [ "$BIN_DIR" = "$REAL_BIN_DIR" ] || [ "$STATE_DIR" = "$REAL_STATE_DIR" ]; then
    refuse "test install directories must not equal the real directories"
  fi
fi
if [ "$(id -u)" -ne 0 ] && [ "${ONT_INSTALL_TEST:-}" != "1" ]; then
  refuse "must run as uid 0"
fi

BIN_PATH=$BIN_DIR/ont
STORE_DIR=$STATE_DIR/ont
INSTALL_DIR=$STATE_DIR/install
INSTALLED_JSON=$INSTALL_DIR/installed.json

sha256_of() {
  sha256sum "$1" | awk '{print $1}'
}

json_field() {
  # $1 = file, $2 = key; prints the string value or empty.
  sed -n "s/.*\"$2\"[[:space:]]*:[[:space:]]*\"\([^\"]*\)\".*/\1/p" "$1" | head -n 1
}

now_utc() {
  date -u +%Y-%m-%dT%H:%M:%SZ
}

write_installed_json() {
  # $1 = commit, $2 = sha256, $3 = previous sha256 or empty
  local tmp prev
  tmp=$(mktemp "$INSTALL_DIR/.installed.json.XXXXXX")
  if [ -n "$3" ]; then
    prev="\"$3\""
  else
    prev=null
  fi
  printf '{"commit":"%s","sha256":"%s","installed_at":"%s","previous_sha256":%s}\n' \
    "$1" "$2" "$(now_utc)" "$prev" >"$tmp"
  chmod 0600 "$tmp"
  mv "$tmp" "$INSTALLED_JSON"
}

version_commit() {
  # Prints the third field of `ont <version> <commit>`, or empty.
  "$1" --version 2>/dev/null | awk 'NR==1 && $1=="ont" && NF==3 {print $3}'
}

do_install() {
  local commit=${1:-}
  [[ $commit =~ ^[0-9a-f]{40}$ ]] || refuse "commit must be 40 lowercase hex"
  git -C "$REPO_ROOT" merge-base --is-ancestor "$commit" refs/remotes/origin/main 2>/dev/null \
    || refuse "commit_not_merged: $commit is not an ancestor of refs/remotes/origin/main"
  if [ "$DRY_RUN" = "1" ]; then
    printf 'dry-run: git -C %s archive %s | tar -x into a new temporary directory\n' "$REPO_ROOT" "$commit"
    if [ -n "${ONT_INSTALL_PREBUILT:-}" ]; then
      printf 'dry-run: use prebuilt binary %s (build skipped)\n' "$ONT_INSTALL_PREBUILT"
    else
      printf 'dry-run: flock %s cargo build --release -p ont --locked (CARGO_TARGET_DIR under the temporary directory, CARGO_BUILD_JOBS=2)\n' "$CARGO_LOCK"
    fi
    printf 'dry-run: check the built binary prints ont <version> %s\n' "$commit"
    printf 'dry-run: create %s and %s with mode 0700\n' "$STORE_DIR" "$INSTALL_DIR"
    printf 'dry-run: keep any previous binary as %s/ont.before-<first 12 hex of its sha256>\n' "$INSTALL_DIR"
    printf 'dry-run: install -m 0755 the new binary as a temporary name in %s, then mv over %s\n' "$BIN_DIR" "$BIN_PATH"
    printf 'dry-run: write %s by temporary name and rename; remove the temporary build directory\n' "$INSTALLED_JSON"
    return 0
  fi

  local tmp built
  tmp=$(mktemp -d)
  trap 'rm -rf "$tmp"' EXIT
  if [ -n "${ONT_INSTALL_PREBUILT:-}" ]; then
    [ -f "$ONT_INSTALL_PREBUILT" ] || refuse "prebuilt binary not found: $ONT_INSTALL_PREBUILT"
    built=$ONT_INSTALL_PREBUILT
  else
    local avail_gib
    avail_gib=$(df -BG --output=avail / | tail -n 1 | tr -dc '0-9')
    [ -n "$avail_gib" ] || avail_gib=0
    [ "$avail_gib" -ge 18 ] || refuse "free space on / is under 18 GiB"
    mkdir -p "$tmp/src" "$tmp/target"
    git -C "$REPO_ROOT" archive "$commit" | tar -x -C "$tmp/src"
    # The build script reads the build commit from git HEAD; give the
    # exported tree that exact HEAD (fetch + reset --mixed keeps the
    # exported files and a clean status).
    git -C "$tmp/src" init -q
    git -C "$tmp/src" fetch -q "$REPO_ROOT" "$commit"
    git -C "$tmp/src" reset -q --mixed FETCH_HEAD
    (cd "$tmp/src" && flock "$CARGO_LOCK" env CARGO_TARGET_DIR="$tmp/target" CARGO_BUILD_JOBS=2 cargo build --release -p ont --locked)
    built=$tmp/target/release/ont
  fi
  [ -x "$built" ] || refuse "built binary not found: $built"
  local got_commit
  got_commit=$(version_commit "$built" || true)
  [ "$got_commit" = "$commit" ] || refuse "built binary does not print commit $commit (got: ${got_commit:-none})"

  mkdir -p "$BIN_DIR"
  mkdir -p "$STORE_DIR" "$INSTALL_DIR"
  chmod 0700 "$STORE_DIR" "$INSTALL_DIR"

  local previous_sha=""
  if [ -f "$BIN_PATH" ]; then
    previous_sha=$(sha256_of "$BIN_PATH")
    cp "$BIN_PATH" "$INSTALL_DIR/ont.before-${previous_sha:0:12}"
    chmod 0600 "$INSTALL_DIR/ont.before-${previous_sha:0:12}"
  fi

  local tmp_bin new_sha
  tmp_bin=$(mktemp "$BIN_DIR/.ont.XXXXXX")
  install -m 0755 "$built" "$tmp_bin"
  mv "$tmp_bin" "$BIN_PATH"
  new_sha=$(sha256_of "$BIN_PATH")
  write_installed_json "$commit" "$new_sha" "$previous_sha"
  rm -rf "$tmp"
  trap - EXIT

  printf 'installed_sha256: %s\n' "$new_sha"
  printf 'rollback command: %s rollback\n' "$SCRIPT_PATH"
  printf 'next: %s --store %s schema install --from %s --commit %s --path <path inside the repository>\n' "$BIN_PATH" "$STORE_DIR/ontology.sqlite" "$REPO_ROOT" "$commit"
  printf 'next: %s --store %s index\n' "$BIN_PATH" "$STORE_DIR/ontology.sqlite"
}

do_check() {
  if [ "$DRY_RUN" = "1" ]; then
    printf 'dry-run: check %s exists, its sha256 equals %s, its --version prints the recorded commit, and %s has mode 0700\n' "$BIN_PATH" "$INSTALLED_JSON" "$STORE_DIR"
    return 0
  fi
  [ -f "$BIN_PATH" ] || { printf 'mismatch: %s does not exist\n' "$BIN_PATH" >&2; exit 2; }
  printf 'exists: %s\n' "$BIN_PATH"
  [ -f "$INSTALLED_JSON" ] || { printf 'mismatch: %s does not exist\n' "$INSTALLED_JSON" >&2; exit 2; }
  local recorded_sha recorded_commit actual_sha got_commit mode
  recorded_sha=$(json_field "$INSTALLED_JSON" sha256)
  recorded_commit=$(json_field "$INSTALLED_JSON" commit)
  actual_sha=$(sha256_of "$BIN_PATH")
  [ "$actual_sha" = "$recorded_sha" ] || { printf 'mismatch: sha256 of %s is %s, installed.json records %s\n' "$BIN_PATH" "$actual_sha" "$recorded_sha" >&2; exit 2; }
  printf 'sha256: %s matches installed.json\n' "$actual_sha"
  got_commit=$(version_commit "$BIN_PATH" || true)
  [ "$got_commit" = "$recorded_commit" ] || { printf 'mismatch: --version commit is %s, installed.json records %s\n' "${got_commit:-none}" "$recorded_commit" >&2; exit 2; }
  printf 'version_commit: %s matches installed.json\n' "$got_commit"
  mode=$(stat -c '%a' "$STORE_DIR" 2>/dev/null || echo missing)
  [ "$mode" = "700" ] || { printf 'mismatch: %s mode is %s, want 0700\n' "$STORE_DIR" "$mode" >&2; exit 2; }
  printf 'store_dir_mode: 0700 (%s)\n' "$STORE_DIR"
}

do_rollback() {
  if [ "$DRY_RUN" = "1" ]; then
    printf 'dry-run: restore the previous binary named by %s over %s (temporary name then mv), verify its sha256, and rewrite %s; the store is not touched\n' "$INSTALLED_JSON" "$BIN_PATH" "$INSTALLED_JSON"
    return 0
  fi
  [ -f "$INSTALLED_JSON" ] || refuse "no installed.json; nothing to roll back"
  local previous_sha previous
  previous_sha=$(json_field "$INSTALLED_JSON" previous_sha256)
  [ -n "$previous_sha" ] || refuse "no previous binary recorded"
  previous=$INSTALL_DIR/ont.before-${previous_sha:0:12}
  [ -f "$previous" ] || refuse "previous binary not found: $previous"
  [ "$(sha256_of "$previous")" = "$previous_sha" ] || refuse "previous binary sha256 does not match installed.json"
  mkdir -p "$BIN_DIR"
  local tmp_bin restored_commit
  tmp_bin=$(mktemp "$BIN_DIR/.ont.XXXXXX")
  install -m 0755 "$previous" "$tmp_bin"
  mv "$tmp_bin" "$BIN_PATH"
  [ "$(sha256_of "$BIN_PATH")" = "$previous_sha" ] || refuse "restored binary sha256 mismatch"
  restored_commit=$(version_commit "$BIN_PATH" || true)
  [ -n "$restored_commit" ] || refuse "restored binary does not print a version line"
  write_installed_json "$restored_commit" "$previous_sha" ""
  printf 'restored_sha256: %s\n' "$previous_sha"
  printf 'store untouched: %s and its schema were not changed\n' "$STORE_DIR"
}

case ${1:-} in
  install)
    [ $# -eq 2 ] || refuse "usage: install <commit>"
    do_install "$2"
    ;;
  check)
    [ $# -eq 1 ] || refuse "usage: check"
    do_check
    ;;
  rollback)
    [ $# -eq 1 ] || refuse "usage: rollback"
    do_rollback
    ;;
  *)
    printf 'usage: %s (install <commit>|check|rollback) [--dry-run]\n' "$SCRIPT_PATH" >&2
    exit 2
    ;;
esac
