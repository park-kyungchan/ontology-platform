#!/bin/bash
# Install, check or roll back the `ont` binary. No schema is installed and
# no index is run here; install prints those commands for the host step.
set -euo pipefail

# Never let the caller redirect git away from the checkout named by -C.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_COMMON_DIR

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
  if [ "$(realpath -m -- "$BIN_DIR")" = "$(realpath -m -- "$REAL_BIN_DIR")" ] || [ "$(realpath -m -- "$STATE_DIR")" = "$(realpath -m -- "$REAL_STATE_DIR")" ]; then
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

CLEANUP_BUILD=""
CLEANUP_BIN=""
cleanup() {
  [ -z "$CLEANUP_BIN" ] || rm -f -- "$CLEANUP_BIN"
  [ -z "$CLEANUP_BUILD" ] || rm -rf -- "$CLEANUP_BUILD"
  return 0
}
trap cleanup EXIT

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
  tmp=$(mktemp "$INSTALL_DIR/.installed.json.XXXXXX") || return 1
  if [ -n "$3" ]; then
    prev="\"$3\""
  else
    prev=null
  fi
  printf '{"commit":"%s","sha256":"%s","installed_at":"%s","previous_sha256":%s}\n' \
    "$1" "$2" "$(now_utc)" "$prev" >"$tmp" || { rm -f -- "$tmp"; return 1; }
  chmod 0600 "$tmp" || { rm -f -- "$tmp"; return 1; }
  mv "$tmp" "$INSTALLED_JSON" || { rm -f -- "$tmp"; return 1; }
}

version_commit() {
  # Prints the third field of `ont <version> <commit>`, or empty.
  "$1" --version 2>/dev/null | awk 'NR==1 && $1=="ont" && NF==3 {print $3}'
}

state_write_failed() {
  # $1 = operation; $2 = install's previous sha256, when any.
  printf 'installed.json write failed; manual recovery commands:\n' >&2
  if [ "$1" = "rollback" ]; then
    # The previous copy is already restored. Repeating rollback records it.
    printf '%q rollback\n' "$SCRIPT_PATH" >&2
  elif [ -n "$2" ]; then
    printf 'install -m 0755 -- %q %q\n' "$INSTALL_DIR/ont.before-${2:0:12}" "$BIN_PATH" >&2
  else
    printf 'rm -f -- %q\n' "$BIN_PATH" >&2
  fi
  printf '# installed.json still describes the installation before this attempt; verify it before check or rollback\n' >&2
  exit 1
}

do_install() {
  local commit=${1:-}
  [[ $commit =~ ^[0-9a-f]{40}$ ]] || refuse "commit must be 40 lowercase hex"
  if [ "$DRY_RUN" = "1" ]; then
    printf 'dry-run: git -C %s fetch origin +refs/heads/main:refs/remotes/origin/main --refmap= before the ancestor check\n' "$REPO_ROOT"
  else
    printf 'fetch: git -C %s fetch origin +refs/heads/main:refs/remotes/origin/main --refmap=\n' "$REPO_ROOT"
    git -C "$REPO_ROOT" fetch origin +refs/heads/main:refs/remotes/origin/main --refmap= || refuse "git_unavailable: fetch origin failed"
  fi
  local commit_type
  commit_type=$(printf '%s\n' "$commit" | git -C "$REPO_ROOT" cat-file --batch-check='%(objecttype)' 2>/dev/null) \
    || refuse "git_unavailable: commit lookup failed"
  [ "$commit_type" = "commit" ] || refuse "commit_unknown: $commit is not a known commit in the checkout"
  local ancestor_status=0
  git -C "$REPO_ROOT" merge-base --is-ancestor "$commit" refs/remotes/origin/main 2>/dev/null || ancestor_status=$?
  case $ancestor_status in
    0) ;;
    1) refuse "commit_not_merged: $commit is not an ancestor of refs/remotes/origin/main" ;;
    *) refuse "git_unavailable: ancestor check failed (status $ancestor_status)" ;;
  esac
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

  local tmp built tmp_bin=""
  tmp=$(mktemp -d)
  CLEANUP_BUILD=$tmp
  if [ -n "${ONT_INSTALL_PREBUILT:-}" ]; then
    [ -f "$ONT_INSTALL_PREBUILT" ] || refuse "prebuilt binary not found: $ONT_INSTALL_PREBUILT"
    built=$ONT_INSTALL_PREBUILT
  else
    local avail_gib
    avail_gib=$(df -BG --output=avail "$tmp" | tail -n 1 | tr -dc '0-9')
    [ -n "$avail_gib" ] || avail_gib=0
    [ "$avail_gib" -ge 10 ] || refuse "free space on the build file system is under 10 GiB"
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
    [ -f "$INSTALL_DIR/ont.before-${previous_sha:0:12}" ] &&
      [ "$(sha256_of "$INSTALL_DIR/ont.before-${previous_sha:0:12}")" = "$previous_sha" ] \
      || refuse "previous binary copy is missing or has the wrong sha256"
  fi

  local new_sha
  tmp_bin=$(mktemp "$BIN_DIR/.ont.XXXXXX")
  CLEANUP_BIN=$tmp_bin
  install -m 0755 "$built" "$tmp_bin"
  mv "$tmp_bin" "$BIN_PATH"
  new_sha=$(sha256_of "$BIN_PATH")
  if ! write_installed_json "$commit" "$new_sha" "$previous_sha"; then
    state_write_failed install "$previous_sha"
  fi
  prune_previous
  rm -rf "$tmp"

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
  local restored_commit tmp_bin=""
  mkdir -p "$BIN_DIR"
  # The retained copy is 0600: execute a disposable copy before replacing ont.
  tmp_bin=$(mktemp "$BIN_DIR/.ont.XXXXXX")
  CLEANUP_BIN=$tmp_bin
  install -m 0755 "$previous" "$tmp_bin"
  restored_commit=$(version_commit "$tmp_bin" || true)
  [ -n "$restored_commit" ] || refuse "previous binary does not print a version line"
  mv "$tmp_bin" "$BIN_PATH"
  [ "$(sha256_of "$BIN_PATH")" = "$previous_sha" ] || refuse "restored binary sha256 mismatch"
  if ! write_installed_json "$restored_commit" "$previous_sha" ""; then
    state_write_failed rollback ""
  fi
  printf 'restored_sha256: %s\n' "$previous_sha"
  printf 'store untouched: %s and its schema were not changed\n' "$STORE_DIR"
}

prune_previous() {
  local count=0 path
  while IFS= read -r path; do
    count=$((count + 1))
    [ "$count" -le 3 ] || rm -f -- "$path"
  done < <(find "$INSTALL_DIR" -maxdepth 1 -type f -name 'ont.before-*' -printf '%T@ %p\n' | sort -rn | cut -d' ' -f2-)
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
