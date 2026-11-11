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

# Exercise fetch against a disposable local remote, never the host checkout.
mkdir -p "$TMP/checkout/scripts"
cp "$INSTALL" "$TMP/checkout/scripts/install-ont.sh"
INSTALL=$TMP/checkout/scripts/install-ont.sh
git -C "$TMP/checkout" init -q -b main
git -C "$TMP/checkout" -c user.name=Test -c user.email=test@example.invalid commit -qm fixture --allow-empty
git clone -q --bare "$TMP/checkout" "$TMP/origin.git"
git -C "$TMP/checkout" remote add origin "$TMP/origin.git"
git -C "$TMP/checkout" fetch -q origin
GOOD=$(git -C "$TMP/checkout" rev-parse HEAD)

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
  ONT_INSTALL_PREBUILT="$TMP/ont-v1" "$INSTALL" install "$GOOD" >"$TMP/guard.err" 2>&1
code=$?
[ "$code" -eq 2 ] || fail "overrides without ONT_INSTALL_TEST exited $code, want 2"
rg_guard=$(cat "$TMP/guard.err")
case $rg_guard in *"require ONT_INSTALL_TEST=1"*) ;; *) fail "wrong override guard reason" ;; esac

ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR=/usr/local/bin ONT_INSTALL_STATE_DIR="$STATE1" \
  ONT_INSTALL_PREBUILT="$TMP/ont-v1" "$INSTALL" install "$GOOD" >/dev/null 2>&1
code=$?
set -e
[ "$code" -eq 2 ] || fail "override equal to the real directory exited $code, want 2"

# Every refusal below must identify the actual guard, not merely exit 2.
expect_refusal() {
  expected=$1
  shift
  set +e
  "$@" >"$TMP/refusal" 2>&1
  code=$?
  set -e
  [ "$code" -eq 2 ] || fail "expected refusal $expected, got $code"
  case $(cat "$TMP/refusal") in *"$expected"*) ;; *) fail "missing refusal message $expected" ;; esac
}
fixture() {
  env ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" "$INSTALL" "$@"
}
expect_refusal 'no previous binary recorded' fixture rollback
expect_refusal 'require ONT_INSTALL_TEST=1' env ONT_INSTALL_PREBUILT="$TMP/ont-v1" "$INSTALL" check
expect_refusal 'must not equal the real directories' env ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR=/var/lib/ontology-platform "$INSTALL" check
ln -s /usr/local/bin "$TMP/real-bin"
ln -s /var/lib/ontology-platform "$TMP/real-state"
expect_refusal 'must not equal the real directories' env ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$TMP/real-bin" ONT_INSTALL_STATE_DIR="$STATE1" "$INSTALL" check
expect_refusal 'must not equal the real directories' env ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$TMP/real-state" "$INSTALL" check
expect_refusal 'must not equal the real directories' env ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR=/usr//local/bin/ ONT_INSTALL_STATE_DIR="$STATE1" "$INSTALL" check
# The guard reason also holds when id would report an unprivileged user.
mkdir "$TMP/tools"
printf '#!/bin/sh\nprintf "1000\\n"\n' >"$TMP/tools/id"
chmod +x "$TMP/tools/id"
expect_refusal 'require ONT_INSTALL_TEST=1' env PATH="$TMP/tools:$PATH" ONT_INSTALL_BIN_DIR="$BIN1" "$INSTALL" check

cp "$STATE1/install/installed.json" "$TMP/good.json"
sed "s/\"commit\":\"$GOOD\"/\"commit\":\"0000000000000000000000000000000000000000\"/" "$TMP/good.json" >"$STATE1/install/installed.json"
expect_refusal 'mismatch: --version commit' fixture check
cp "$TMP/good.json" "$STATE1/install/installed.json"
chmod 0755 "$STATE1/ont"
expect_refusal 'want 0700' fixture check
chmod 0700 "$STATE1/ont"
run_test_install "$TMP/ont-v2"
rm "$STATE1/install/ont.before-$PREV12"
expect_refusal 'previous binary not found' fixture rollback
# Bad previous version must leave binary/state intact and remove temporary names.
printf '#!/bin/sh\nexit 1\n' >"$TMP/bad-version"
cp "$TMP/bad-version" "$BIN1/ont"
run_test_install "$TMP/ont-v2"
cp "$STATE1/install/installed.json" "$TMP/before.json"
expect_refusal 'previous binary does not print a version line' fixture rollback
cmp "$TMP/before.json" "$STATE1/install/installed.json" || fail 'rollback changed state before version validation'
[ "$(sha256sum "$BIN1/ont" | awk '{print $1}')" = "$SHA_V2" ] || fail 'bad version replaced binary'
[ -z "$(find "$BIN1" -name '.ont.*' -print)" ] || fail 'stray rollback temporary'
# Keep at most three previous copies, only after state is written.
for n in 3 4 5 6; do
  make_standin "$TMP/ont-next" "v$n"
  run_test_install "$TMP/ont-next"
done
[ "$(find "$STATE1/install" -name 'ont.before-*' | wc -l)" -eq 3 ] || fail 'retention did not keep three copies'

# Refresh a stale ref before checking ancestry.
git -C "$TMP/checkout" -c user.name=Test -c user.email=test@example.invalid commit -qm newer --allow-empty
git -C "$TMP/checkout" push -q origin main
GOOD=$(git -C "$TMP/checkout" rev-parse HEAD)
make_standin "$TMP/new-head" newer
old_ref=$(git -C "$TMP/checkout" rev-parse refs/remotes/origin/main)
git -C "$TMP/checkout" update-ref refs/remotes/origin/main "$old_ref~1"
fixture install "$(git -C "$TMP/checkout" rev-parse "$old_ref~1")" --dry-run >"$TMP/dry"
case $(cat "$TMP/dry") in *'fetch origin before the ancestor check'*) ;; *) fail 'dry run omitted fetch step' ;; esac
[ "$(git -C "$TMP/checkout" rev-parse refs/remotes/origin/main)" = "$(git -C "$TMP/checkout" rev-parse "$old_ref~1")" ] || fail 'dry run fetched'
run_test_install "$TMP/new-head"
[ "$(git -C "$TMP/checkout" rev-parse refs/remotes/origin/main)" = "$GOOD" ] || fail 'install did not fetch'

# Fault injection uses PATH wrappers only in the disposable fixture.
printf '#!/bin/sh\ncase "$*" in *merge-base*) exit 128 ;; esac\nexec /usr/bin/git "$@"\n' >"$TMP/tools/git"
chmod +x "$TMP/tools/git"
expect_refusal 'git_unavailable: ancestor check failed (status 128)' env PATH="$TMP/tools:$PATH" ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" ONT_INSTALL_PREBUILT="$TMP/new-head" "$INSTALL" install "$GOOD"
rm "$TMP/tools/git"
# A corrupted backup copy must be refused before replacement.
printf '#!/bin/sh\n/usr/bin/cp "$@" || exit $?\nprintf x >>"$2"\n' >"$TMP/tools/cp"
chmod +x "$TMP/tools/cp"
expect_refusal 'previous binary copy is missing or has the wrong sha256' env PATH="$TMP/tools:$PATH" ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" ONT_INSTALL_PREBUILT="$TMP/new-head" "$INSTALL" install "$GOOD"
cmp "$TMP/new-head" "$BIN1/ont" || fail 'bad backup replaced binary'
rm "$TMP/tools/cp"
# Failed state rename after replacement prints exact recovery and cleans temp files.
printf '#!/bin/sh\ncase "$2" in */installed.json) exit 1 ;; esac\nexec /usr/bin/mv "$@"\n' >"$TMP/tools/mv"
chmod +x "$TMP/tools/mv"
make_standin "$TMP/last" last
set +e
env PATH="$TMP/tools:$PATH" ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" ONT_INSTALL_PREBUILT="$TMP/last" "$INSTALL" install "$GOOD" >"$TMP/recovery" 2>&1
code=$?
set -e
[ "$code" -ne 0 ] || fail 'state write failure succeeded'
case $(cat "$TMP/recovery") in *'manual recovery commands:'*'install -m 0755 -- '*) ;; *) fail 'manual recovery omitted' ;; esac
[ -z "$(find "$BIN1" "$STATE1/install" -name '.ont.*' -o -name '.installed.json.*')" ] || fail 'stray temporary after state failure'
rm "$TMP/tools/mv"
sed -n '/^install -m 0755 -- /p' "$TMP/recovery" >"$TMP/manual-recovery"
bash "$TMP/manual-recovery"
fixture check >/dev/null || fail 'printed recovery commands did not restore check'
# A failed install into the temporary binary is cleaned up too.
printf '#!/bin/sh\nexit 1\n' >"$TMP/tools/install"
chmod +x "$TMP/tools/install"
set +e
env PATH="$TMP/tools:$PATH" ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" ONT_INSTALL_PREBUILT="$TMP/last" "$INSTALL" install "$GOOD" >/dev/null 2>&1
code=$?
set -e
[ "$code" -ne 0 ] || fail 'temporary install failure succeeded'
[ -z "$(find "$BIN1" -name '.ont.*')" ] || fail 'stray failed-install temporary'
rm "$TMP/tools/install"
# Refuse before cargo, and record that df received the mktemp build path.
printf '#!/bin/sh\nprintf "%%s\\n" "$3" >"$ONT_DF_LOG"\nprintf "Avail\\n0G\\n"\n' >"$TMP/tools/df"
chmod +x "$TMP/tools/df"
expect_refusal 'free space on the build file system is under 18 GiB' env PATH="$TMP/tools:$PATH" ONT_DF_LOG="$TMP/df-path" TMPDIR="$TMP" ONT_INSTALL_TEST=1 ONT_INSTALL_BIN_DIR="$BIN1" ONT_INSTALL_STATE_DIR="$STATE1" "$INSTALL" install "$GOOD"
case $(cat "$TMP/df-path") in "$TMP"/tmp.*) ;; *) fail 'df did not inspect temporary build directory' ;; esac

printf 'test-install-ont: PASS\n'
