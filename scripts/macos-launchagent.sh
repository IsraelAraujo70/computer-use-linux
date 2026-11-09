#!/bin/sh
# Install the macOS MCP server as a LaunchAgent behind a Unix socket.
#
# Why: TCC (Accessibility, Screen Recording) charges a process's grants to its
# *responsible* process. Started from a terminal, sshd or an app (T3, Claude
# Code), that is the terminal/sshd/app, not this binary. launchd-spawned
# processes are their own responsible process, so the grant given to
# $BIN (fixed path + fixed signing identity) is the one that counts, and it
# survives rebuilds as long as the path, identifier and certificate stay.
#
# launchd listens on $SOCK and, per connection (inetd mode), runs `$BIN mcp`
# with stdin/stdout on the socket. MCP clients connect with:
#   local:  nc -U $SOCK
#   remote: ssh mac nc -U $SOCK
#
# Usage: scripts/macos-launchagent.sh [path-to-built-binary]
#   SIGN_IDENTITY / SIGN_KEYCHAIN / BUNDLE_ID override the signing defaults.
set -eu

SRC=${1:-target/release/computer-use-linux}
BIN=${BIN:-$HOME/.local/bin/computer-use-mac}
BUNDLE_ID=${BUNDLE_ID:-dev.israel.computer-use-mac}
SIGN_IDENTITY=${SIGN_IDENTITY:-computer-use-dev}
SIGN_KEYCHAIN=${SIGN_KEYCHAIN:-$HOME/.config/cul-signing/signing.keychain-db}
LABEL=$BUNDLE_ID
SOCK_DIR=$HOME/.local/state/computer-use-mac
SOCK=$SOCK_DIR/mcp.sock
PLIST=$HOME/Library/LaunchAgents/$LABEL.plist

mkdir -p "$(dirname "$BIN")" "$SOCK_DIR"
chmod 700 "$SOCK_DIR"
cp "$SRC" "$BIN"
if [ -f "$HOME/.config/cul-signing/kc.pass" ]; then
  security unlock-keychain -p "$(cat "$HOME/.config/cul-signing/kc.pass")" "$SIGN_KEYCHAIN"
fi
codesign -f -s "$SIGN_IDENTITY" --keychain "$SIGN_KEYCHAIN" -i "$BUNDLE_ID" "$BIN"

cat > "$PLIST" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$LABEL</string>
  <key>ProgramArguments</key><array><string>$BIN</string><string>mcp</string></array>
  <key>Sockets</key>
  <dict>
    <key>Listener</key>
    <dict>
      <key>SockPathName</key><string>$SOCK</string>
      <key>SockPathMode</key><integer>384</integer>
    </dict>
  </dict>
  <key>inetdCompatibility</key><dict><key>Wait</key><false/></dict>
  <key>EnvironmentVariables</key>
  <dict><key>PATH</key><string>/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string></dict>
  <key>StandardErrorPath</key><string>$SOCK_DIR/stderr.log</string>
</dict>
</plist>
EOF

launchctl bootout "gui/$(id -u)/$LABEL" 2>/dev/null || true
launchctl bootstrap "gui/$(id -u)" "$PLIST"
echo "listening on $SOCK (grant Accessibility + Screen Recording to $BIN once)"
