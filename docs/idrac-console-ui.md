# Dell console and desktop UI implementation

Status: implemented locally; verification and delivery are in progress. This document tracks the whole requested outcome, rather than
treating a visual refresh as completion.

## Required outcome

- The tray popup lists configured Dell servers and opens their VNC consoles.
- A Bosn-owned container supplies the console runtime and has bounded lifecycle,
  ordinary ownership labels, low CPU priority and no published ports.
- Console observation and keyboard/mouse input work on an actual configured
  iDRAC server. Opening a login page alone does not prove console control.
- The popup, run dashboard and system monitor share a readable visual design.
- Evaluate Qt and egui through kernal-api before choosing a native migration.

## Current evidence

The widget is its own Cargo workspace. Its pinned kernal-api 0.1.31 enables
`tauri-webview` and `system-tray`. Its Cargo feature list contains no Qt or egui
backend. The native tray has three fixed events: Activate, OpenDashboard and
Quit. Activate already opens the HTML popup; server selection belongs there.
Pages have no privileged IPC bridge: controls go through authenticated,
same-origin daemon routes and typed widget commands.

The operator has an existing local Dell connection profile and a VNC helper.
Inspection shows TLS is established before the RFB handshake, and the peer
certificate is checked against a recorded SHA-256 fingerprint before VNC
authentication. That is a transport requirement for the new console. Do not
commit the operator's inventory, addresses, fingerprints or credentials here.
The profile's hardware and provisioning observations are historical, not a
fresh hardware health report.

## Console implementation constraints

Parse server profiles into typed structs at the boundary. Give each target a
stable identifier and display name, iDRAC host, VNC port, certificate pin. The browser sends a target identifier, never an arbitrary
host or executable. The separate VNC password is entered into the viewer and
used only for RFB authentication; it is never stored in the catalog, URL or daemon.
Existing local inventory supplies the public connection fields without copying
its credential values.

Use a pinned, compatible VNC client. If it needs a bridge, that bridge must
establish TLS to iDRAC and verify the configured pin before relaying RFB. Never
silently disable iDRAC encryption to accommodate a client. Bind local listeners
only to loopback and authorize each browser session. Reject changed certificates
with an actionable error. Keep clipboard and file transfer off initially.

Opening a console must not change server power, firmware, boot targets or VNC
settings. Report certificate and authentication failures distinctly. Connection failures
should direct the user to check reachability, the VNC setting and console license. Do not show a successful
connection until RFB authentication and framebuffer initialization succeed.

Use existing Bosn setup/container ownership and retention paths. Closing the
viewer releases its session; daemon restart and client disappearance must not
leave an untracked container or externally reachable proxy.

## UI framework assessment

The existing webview boundary already separates the desktop from the headless
daemon and ships the same pages in browser and widget. A Qt/egui migration needs
a kernal-api backend and corresponding native packaging, accessibility, Wayland
placement, tray and cross-platform coverage. It is not a Cargo feature switch
in the pinned dependency. First align the existing views' typography, spacing,
palette, focus states and navigation. Reassess a native frontend against those
same acceptance criteria once a supported backend is available; retain the
daemon API boundary whichever frontend is chosen.

## Using the console

Create `<state>/servers.toml` (the same state directory used by the daemon):

```toml
[[servers]]
id = "rack1"
name = "Dell rack server"
host = "192.0.2.10"
port = 5901
certificate_sha256 = "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"
```

Use the actual VNC port and a SHA-256 fingerprint independently verified for
that server. The example is a documentation address, not a usable connection.
The existing Dell inventory's `host`, `vnc_port` and
`https_certificate_sha256` fields map to these fields respectively, provided
the VNC endpoint is verified to present that same certificate.
Configuration reloads when a console/list request arrives. The
tray popup refreshes its server list every ten seconds. The explicit link is
`bosn ui --path /console/rack1`.

Click **Open console** under **Servers** in the tray popup, then enter the
separate iDRAC VNC password. The viewer is noVNC 1.7.0 (RFB/VNC compatible), not
an installed TightVNC desktop application. Its notices and reproducible source
archive are linked from the viewer. It needs Docker and an initial download of
the digest-pinned Python runtime image. No VNC listener is exposed on the host.

The container connects over TLS 1.2 or newer, checks the configured certificate
pin, and only then forwards RFB. The daemon carries binary traffic through
bounded Docker pipes and an authenticated same-origin WebSocket. Four consoles
per daemon are allowed. Idle sessions expire after ten minutes, and every session
has a twelve-hour ceiling. Active consoles prevent idle daemon handoff. Creation finishes before the viewer attaches by immutable container ID; an
interrupted creation cannot launch a viewer from a lost creation reply. Closing
the page or clicking Disconnect closes stdin and removes the exactly labeled
container by immutable Docker ID. Containers also exit on stdin EOF and have an
in-container lifetime timer for abrupt daemon loss. No bind mounts, Docker socket
mount, host port, privileged mode or persistent console volume is used.

## Delivery evidence

- Implemented locally: shared light/dark design tokens, spacing updates across
  the three main views, reduced-motion support, a System popup shortcut and
  server console selection.
- Browser layout checks: light/dark at 440×620 for the popup and 1280×800 for
  dashboard/system; no document horizontal overflow. The console's pinned-cert
  error was rendered through the actual bundled noVNC module without JS errors.
- Isolated focused checks: all 35 UI tests (including HTTP console authorization
  and failed-upgrade reservation release) and both real TLS transport tests pass.
- A native browser WebSocket against a synthetic RFB server verifies a rendered
  RGB framebuffer pixel, ArrowDown press/release transmission and disconnect,
  under the same CSP used by Bosn. No browser JS errors were observed. This is
  separate from live iDRAC verification.
- The first broad Rust run was cut off by its 64 KiB output budget, not a reported
  test failure. Subsequent runs use an 8 MiB capture budget.
- Live Redfish verification: the configured chassis identity matches local
  inventory, it is powered on, and its named VNC credential is available in the
  existing authorized keyring. No power or boot configuration was changed.
- A second daemon in an empty state directory was correctly refused by Bosn's
  registry-identity protection. The installed daemon speaks protocol 2, while
  the candidate speaks protocol 5; its pre-flight refuses work. Live daemon UI
  verification awaits a supported upgrade; never bypass registry protection or
  stop a busy daemon.
- Live transport verification: the actual Dell authenticated through the new
  digest-pinned, read-only, low-priority TLS bridge container and returned a
  1024×768 framebuffer. No input was sent. EOF exited the container with status
  0, and Docker inspection proved it was removed.
- Review found stale reconnect-event handling and an absent idle refresh timer.
  Both regressions reproduce on commit `9756e64f` and pass with current source
  in `ci/test_console_controls.js`. The pytest wrapper includes them in CI.
- The first full local gate found a console-asset include base that violates
  the publishing guard. Console assets now use the existing page asset loader,
  and the same include-base guard passes.
- Live keyboard proof: an independently captured full-frame Redfish image
  showed an empty NixOS login prompt. Enter press/release through the new
  create/attach TLS bridge produced a second prompt, verified by another
  full-frame Redfish screenshot. No text, power or boot changes were sent. The
  single-update VNC capture helper can return partial images, so a blank helper
  capture is not evidence that the actual server screen is blank.
- The one primary pre-push reviewer confirmed the fixes and returned clean.
- The full Rust lane (format, boundary checks, clippy and workspace tests)
  passed on `d92c946a`. The Linux lane found optional-pipe annotations and the
  KeyboardInterrupt lint in the new TLS test fixture; both are fixed and the
  focused pyright/interrupt checks pass. The full gate must pass the final tree.
- Pending: updated full local gate, live daemon/browser integration, PR and
  merge. The installed protocol-2 daemon is still serving other work.
