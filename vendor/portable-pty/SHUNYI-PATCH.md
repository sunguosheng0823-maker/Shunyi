# portable-pty 0.9.0: independent Windows terminals

This is the crates.io `portable-pty` 0.9.0 source, licensed under MIT (see `LICENSE.md`).

The local change in `src/win/psuedocon.rs` removes `PSEUDOCONSOLE_INHERIT_CURSOR` when creating a ConPTY. Shunyi creates a new terminal surface for each session; it does not inherit a console host's cursor. The resize quirk and Win32 input flags remain as upstream defines them. Unix code is unchanged.

Microsoft documents that the inherit-cursor flag requires an asynchronous cursor-position query/reply and can otherwise hang pseudoconsole operations. In the Windows native regression run, all three tests opening terminals stalled, while the four tests without terminals passed. Headless clients must be able to open, resize and close a terminal without a terminal emulator replying to that initialization query.

References:

- https://crates.io/crates/portable-pty/0.9.0
- https://learn.microsoft.com/en-us/windows/console/createpseudoconsole

The Windows-only native lifecycle test in `crates/rc-agent/src/lib.rs` exercises startup, resize, output and close with a bounded watchdog. Protocol tests separately cover multiple terminals, certificate rotation and single-use temporary access.
