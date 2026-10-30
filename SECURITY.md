# Security

Sidecast lets a phone see a screen of your computer and control its mouse and keyboard, so security problems matter. Thank you for reporting them responsibly.

## Reporting a vulnerability

Report it privately through **[GitHub's private vulnerability reporting](https://github.com/achreftlili/sidecast/security/advisories/new)**. Please don't open a public issue. Include what you found, how to reproduce it, and the commit you tested. You'll get an answer within a week; fixes land on `main` and the advisory is published once a fix is available.

Only the latest commit on `main` is supported.

## How Sidecast protects you

The full design is in [docs/PROTOCOL.md](docs/PROTOCOL.md) and [docs/DECISIONS.md](docs/DECISIONS.md). In short:

- **No cloud.** The phone connects directly to the Mac (same network, hotspot, or your own Tailscale tailnet). There are no Sidecast servers, accounts or telemetry.
- **Encrypted.** Every connection is TLS 1.2+ with the Mac's self-signed certificate; the phone pins that certificate when it pairs and refuses a different one afterwards.
- **Pairing needs physical access to the Mac's screen.** The Mac shows a random 6-digit code for 2 minutes; the phone proves it knows the code with `HMAC-SHA256(code, certificate fingerprint ‖ nonce)`, so the code itself never crosses the network. 5 wrong tries end the attempt.
- **Per-device tokens.** A successful pairing gives the phone a random 32-byte token; the Mac stores only its SHA-256 hash and compares in constant time. **Forget** in the Mac's Settings revokes a phone.
- **Unauthenticated connections can't do anything** except ask to pair, which only shows a code on the Mac.
- **Least privilege for root.** The optional keep-awake rule lets Sidecast run exactly `/usr/bin/pmset -a disablesleep 0` and `… 1` with sudo, nothing else, and is checked with `visudo -c` before install.

## What a paired phone can do

A paired phone **can see the virtual display and control the Mac's pointer and keyboard**, like a second keyboard and mouse. While it is connected and a Mac app is recording from "Sidecast Microphone", it streams the phone's microphone to the Mac. Only pair devices you trust.

## Known limitations

- **Pairing and an active man in the middle.** An attacker who can intercept and relay the connection *at the moment you pair* can recover the 6-digit code offline from the phone's proof and pair in the phone's place. This needs an active attacker on your network during the 2-minute window; connections after pairing are protected by the pinned certificate. A PAKE such as SPAKE2 is the planned fix.
- The phone stores its token in its app-private storage, not in the Android Keystore.
- `CGVirtualDisplay` is a private macOS API.
- The Mac listens on TCP port 47800 (by default) on all interfaces (Wi-Fi, Ethernet, Tailscale). Anyone who can reach that port can start a pairing request, which shows a prompt on the Mac.
