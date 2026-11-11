#!/bin/sh
# Test scripts/install-ont.sh in a temporary directory with a small
# stand-in binary (a shell script that prints the expected version line).
# No root, no host path is touched.
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
INSTALL=$SCRIPT_DIR/install-ont.sh

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT

fail() {
  printf 'FAIL: %s\n' "$1" >&2
  exit 1
}

# A commit that is an ancestor of origin/main in this checkout.
if git -C "$REPO_ROOT" rev-parse --verify -q refs/remotes/origin/main >/dev/null 2>&1; then
  GOOD=$(git -C "$REPO_ROOT" rev-parse refs/remotes/origin/main)
else
  GOOD=$(git -C "$REPO_ROOT" rev-parse HEAD)
fi

make_standin() {
  # $1 = path, $2 = marker comment (makes the bytes differ)
  {
    printf '#!/bin/sh\n'
    printf '# %s\n' "$2"
    printf 'if [ "${1:-}" = "--version" ]; then printf "ont 0.1.0 %s\\n" "%s"; exit 0; fi\n' "$GOOD" "$GOOD"
    printf 'printf "stand-in ont\\n"\n'
  } >"$1"
  chmod 0755 "$1"
}

BIN1=$TMP/bin
STATE1=$TMP/state
make_standin "$TMP/ont-v1" v1
make_standin "$TMP/ont-v2" v2
SHA_V1=$(sha256sum "$TMP/ont-v1" | awk '{print $1}')
SHA_V2=$(sha256sum "$TMP/ont-v2" | awk '{print $1}')

run_test_install() {
  # $1 = prebuilt path; extra env is inherited from the caller.
  ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" \
    ONT_INSTALL_PREBUILT="$1" "$INSTALL" install "$GOOD" >/dev/null
}

# install (prebuilt stand-in) and check passes.
run_test_install "$TMP/ont-v1"
[ -x "$BIN1/ont" ] || fail "binary was not installed"
ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" \
  "$INSTALL" check >/dev/null || fail "check failed after first install"

# A dry-run install changes nothing.
before=$(sha256sum "$BIN1/ont" | awk '{print $1}')
ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" \
  ONT_INSTALL_PREBUILT="$TMP/ont-v2" "$INSTALL" install "$GOOD" --dry-run >/dev/null
after=$(sha256sum "$BIN1/ont" | awk '{print $1}')
[ "$before" = "$after" ] || fail "dry-run changed the installed binary"

# A second install keeps the first as previous.
run_test_install "$TMP/ont-v2"
PREV12=$(printf '%s' "$SHA_V1" | cut -c1-12)
[ -f "$STATE1/install/ont.before-$PREV12" ] \
  || fail "previous binary was not kept"
[ "$(sha256sum "$BIN1/ont" | awk '{print $1}')" = "$SHA_V2" ] || fail "second install did not replace the binary"
ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" \
  "$INSTALL" check >/dev/null || fail "check failed after second install"

# A changed binary makes check exit 2.
printf 'x' >>"$BIN1/ont"
set +e
ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" \
  "$INSTALL" check >/dev/null 2>&1
code=$?
set -e
[ "$code" -eq 2 ] || fail "check on a changed binary exited $code, want 2"

# Rollback restores the first binary and check passes.
ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" \
  "$INSTALL" rollback >/dev/null || fail "rollback failed"
[ "$(sha256sum "$BIN1/ont" | awk '{print $1}')" = "$SHA_V1" ] || fail "rollback did not restore the first binary"
ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" \
  "$INSTALL" check >/dev/null || fail "check failed after rollback"

# Refusals.
set +e
ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" \
  ONT_INSTALL_PREBUILT="$TMP/ont-v1" "$INSTALL" install not-a-commit >/dev/null 2>&1
code=$?
[ "$code" -eq 2 ] || fail "bad commit exited $code, want 2"

ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" \
  ONT_INSTALL_PREBUILT="$TMP/ont-v1" "$INSTALL" install "$GOOD" >/dev/null 2>&1
code=$?
[ "$code" -eq 2 ] || fail "overrides without ONT_INSTALL_TEST exited $code, want 2"

ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR=/usr/local/bin ONT_INSTALL_STATE_DIR="$STATE1" \
  ONT_INSTALL_PREBUILT="$TMP/ont-v1" "$INSTALL" install "$GOOD" >/dev/null 2>&1
code=$?
set -e
[ "$code" -eq 2 ] || fail "override equal to the real directory exited $code, want 2"

printf 'test-install-ont: PASS\n'
