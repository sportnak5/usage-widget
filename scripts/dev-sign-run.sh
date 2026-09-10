#!/bin/sh
# Cargo `runner` for local macOS development: sign the dev binary, then run it.
#
# macOS ties a Keychain ACL — the "Always Allow" granted once for Claude Code's
# login — to the signing identity of the program that asked. What `cargo run`
# leaves on an arm64 binary is an ad-hoc signature, which has no stable
# identity: its designated requirement is the binary's own hash, so every
# rebuild is a different program to the Keychain, the grant never matches, and
# the password dialog comes back. Signing each dev build with a real identity
# under a fixed identifier keeps one grant valid across rebuilds.
#
# Release bundles are signed by `tauri build`; this is only for `tauri dev`.
#
# Never fails the run. Unsigned still works — it just asks for the password
# again, which is the situation this exists to improve, not to enforce.
set -eu

BIN="$1"
shift

if [ "$(uname -s)" = "Darwin" ] && [ "${BIN##*/}" = "token-ledger" ]; then
  # An explicit choice wins; otherwise take the first valid signing identity on
  # the machine. Any stable identity will do — the ACL cares that it does not
  # change between builds, not which one it is.
  ID="${TOKEN_LEDGER_SIGN_ID:-$(security find-identity -v -p codesigning 2>/dev/null \
    | sed -n 's/.*) [0-9A-F]* "\(.*\)"$/\1/p' | head -1)}"
  if [ -n "$ID" ]; then
    # A fixed identifier: cargo's own is token_ledger-<hash of the build unit>,
    # which is stable in practice but not something to bet the grant on.
    #
    # Retried, because the common failure is transient: on a rebuild the
    # previous instance may still be exiting, and codesign cannot rewrite a
    # Mach-O that is still mapped by a running process. The reason is printed
    # rather than swallowed — a silent "it'll ask again" is not diagnosable.
    n=0
    until codesign --force --sign "$ID" --identifier com.tokenledger.dev "$BIN" 2>/tmp/dev-sign.err; do
      n=$((n + 1))
      if [ "$n" -ge 5 ]; then
        printf 'dev-sign: could not sign with "%s" after %s tries — the Keychain will ask again\n' "$ID" "$n" >&2
        sed 's/^/dev-sign:   /' /tmp/dev-sign.err >&2
        break
      fi
      sleep 1
    done
  else
    echo 'dev-sign: no code-signing identity found — the Keychain will ask again' >&2
  fi
fi

exec "$BIN" "$@"
