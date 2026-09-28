# Mac/Linux handoff: after-impact report (2026-09-28)

## Observed cause and change

A normal FUSE unmount previously had no positive device handoff command; `fusermount3 -u` could return before the writer finished, and a dead daemon could leave a disconnected FUSE mount entry. `Session::close` also lacked a final device sync when its queue was already empty. The external recovery journal is on DGX, so a cut during in-place APFS updates cannot be recovered by macOS alone.

Added final device sync before publishing CLOSED; `lapfs handoff-ready TARGET OFFSET SESSION` refuses active writer locks, checks the exact target/session, completes any pending recovery, requires empty queue/no owner, syncs the device and exclusively parses the APFS source read-only. `scripts/safe-eject.py` checks the exact running mount identity, performs only normal unmount, handles a dead-daemon stale mount, waits for FUSE close, then requires the positive handoff receipt. It refuses other filesystem types, a different live writer, normal-unmount failure and incomplete recovery.

## Verified after change

- DGX beta.6 release binary SHA: `9c192adef6f4a8e5a73a0fe02e5f8305c513efe7a355da731ef8e78aaf61283c`. Rust ordinary tests and existing Linux FUSE/rsync regression suites passed. The two release-binary image outputs passed independent Mac `fsck_apfs -n` plus 104 original-file SHA checks; rsync data/mode/mtime/link checks passed.
- New handoff test refuses an active writer and wrong target. Release-binary FUSE normal unmount and SIGKILL with stale mount both obtained a positive handoff receipt. Both resulting images passed Mac `fsck_apfs -n`, all 104 original hashes and the added file hash. See [sanitized receipt](validation/handoff-beta6.json).
- A separate Mac-native write, normal detach, Linux read/write, normal handoff and Mac-native read/fsck roundtrip passed on a disposable image. It used the same handoff implementation before the beta.6 version-string rebuild; it is not a physical USB power-cut test.

## Impact boundaries

Changed: `src/buffered.rs`, `src/main.rs`, `src/physical.rs`, `tests/buffered.rs`, `scripts/safe-eject.py`, package inclusion, version/lockfile, README, release/recovery/limit/validation/handoff docs. Unrelated rustfmt changes were reverted. Existing selected Corsair beta.5 mount remains active and unchanged; no original APFS user file, research data, service, GPU process, scheduler, mail or external output was modified. This source/runtime release does not silently replace the running mount.

## Remaining limit

A positive handoff receipt is a clean-transition gate, not a proof against false USB flush acknowledgement or power loss during an in-place transaction. Mac-first mounting after an unclean physical pull remains unsafe. Recovery requires the same DGX and its intact local session/journal. Native APFS CoW/commit redesign and repeatable hardware power-cut tests are required before claiming no-eject Mac-first safety.
