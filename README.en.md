# Shunyi · 瞬移

A terminal-first workspace for macOS: local projects, SSH, and accountless remote device access through a self-hosted relay.

[中文文档](README.md) · [Getting started](docs/getting-started.md) · [Deployment](deploy/README.md) · [Security](SECURITY.md) · [Contributing](CONTRIBUTING.md)

**0.1.0 is a preview release.** The downloadable desktop build targets Apple Silicon Macs. The repository also contains a standalone Agent and relay. Account login, account-based device discovery, P2P and remote desktop are not implemented.

[Download v0.1.0 preview](https://github.com/sunguosheng0823-maker/Shunyi/releases/tag/v0.1.0) · [CI runs](https://github.com/sunguosheng0823-maker/Shunyi/actions/workflows/ci.yml)

## Features

- Project trees and read-only source/Markdown previews.
- Full-height terminals with tabs and separate project contexts.
- A global host list with filters and visibility selection.
- A searchable, editable command library for the active terminal.
- Reusable access certificates and single-use temporary passwords.
- Local sharing controls, credential export and certificate rotation.

## Accountless access

The device owner runs an Agent and shares either an exported `.shunyi-cert` bundle, or a device ID with a generated temporary password. Both endpoints connect to the same self-hosted relay.

Certificate bundles contain private keys and must be protected like SSH private keys. Temporary passwords are atomically consumed on successful authentication. An authorized connection may carry several terminals; after the last terminal closes or the connection is lost, a used password cannot reconnect. Expiry limits authentication, not the duration of an already authorized connection.

The remote shell runs as the Agent's operating-system user. A regular-user Agent does not grant root. Desktop sharing is off by default.

The device channel uses TLS 1.3. Device IDs pin the device CA fingerprint. The Agent authorizes the current client certificate or a temporary password before opening a PTY. The relay forwards encrypted data and can observe routing metadata. Public relay deployments should use trusted WSS. SSH host keys are checked against the system `known_hosts` file.

## Build and try locally

Install stable Rust, Node.js 22 or newer, and platform build tools. macOS requires Xcode Command Line Tools; Linux backend builds require a C/C++ toolchain, `pkg-config`, and OpenSSL development headers.

```bash
cargo build --locked -p rc-agent -p rc-server
npm --prefix client/ui ci
```

Run the relay in one terminal:

```bash
UNIRC_BIND=127.0.0.1:8080 cargo run --locked -p rc-server
```

Initialize and run the Agent in another:

```bash
cargo run --locked -p rc-agent -- init
cargo run --locked -p rc-agent -- export-certificate ./my-device.shunyi-cert
UNIRC_SERVER=ws://127.0.0.1:8080 cargo run --locked -p rc-agent -- run
```

Start the Mac client:

```bash
cd client
ui/node_modules/.bin/tauri dev
```

Set the relay address to `ws://127.0.0.1:8080`, create a host using the Shunyi protocol, and choose the exported certificate. For temporary access, run `cargo run --locked -p rc-agent -- temporary-password 30` from the repository root and use the resulting password with the device ID.

The Mac release archive includes the desktop app and macOS Agent/relay binaries. Linux binaries must be built for Linux. Current Mac binaries use an ad-hoc development signature and have not been Apple-notarized. See the [release notes](CHANGELOG.md) and the release's `SHA256SUMS.txt`.

## Validation and scope

Integration tests run a real relay, Agent, TLS connection and PTY. They cover certificate access, rotation, single-use password races, reconnect rejection, route ownership and WSS upgrades. A native Mac window has also been exercised. Public-network deployment and an independent security audit are separate validation steps.

## License

[Apache-2.0](LICENSE). Copyright 2026 北京链脉科技有限公司. Dependencies retain their own licenses; see [NOTICE](NOTICE). This license does not grant trademark rights.
