# Mac-first interrupted-write audit — 2026-09-28

## Status

**Release gate failed.** This isolated DGX branch is a prototype. Do not deploy it or
advertise Mac-first recovery after an unexpected USB disconnect. The public
beta.6 and currently running Corsair beta.5 were not changed by this work.

## Before-change impact report

- Authority host: DGX `thinkstationpgx-ff82`; canonical source
  `/home/zooh/Documents/LAPFS/source` at `b430daf`, clean when examined.
- Active Corsair remains the existing beta.5 FUSE RW mount. It was not
  unmounted or written by this audit.
- Write call chain: `mount_rw` → `buffered::Session::flush_inner` →
  `apfs_batch::prepare_held` → vendored `file::write_range_plain` /
  `txn::Transaction::commit_inner` → external `journal::apply` → APFS device.
- Existing range writes overwrote old file blocks in place. `journal::recover`
  can restore them on DGX; macOS cannot access the DGX-only undo log.
- Checkpoint commit also writes the shared allocation bitmap, chunk-info
  blocks, spaceman, checkpoint map, bootstrap NXSB and ring NXSB in multiple
  device operations. Its old/new allocation state can be inconsistent between
  operations.
- Scope was limited to an isolated Git worktree and disposable 128 MiB APFS
  images. No physical APFS writer, service, schedule or research file changed.

## Reproduction on released beta.6 code

On a Mac-formatted disposable image, first create an 8192-byte ordinary file.
Prepare a 4096-byte overwrite and kill the apply process after **each** of its
21 physical write/flush operations. Copy each dirty image to macOS without
calling LAPFS recovery. Attach read-only and run native `fsck_apfs -n`.

- Mac accepted all 21 images as readable.
- `fsck_apfs -n` failed at operations **18 and 19** with underallocation and
  overallocation errors; other 19 prefixes checked clean.
- File bytes changed from old to new at operation **3**, before the NXSB
  checkpoint was sealed. A clean fsck result at that prefix did not mean the
  file update was atomic.
- Baseline evidence: `evidence/mac-first-prefix-1ayfwo36` in this worktree
  and the Mac `prefix/mac-results.json` saved with the task evidence.

## Candidate change and after-change impact report

This branch adds a test-only `SPARK_APFS_KILL_AFTER_APPLY_OP` hook, scripts to
generate/check every interrupted prefix, and a **prototype** replacement of
in-place file-data writes with allocation of new blocks plus extent/extentref
edits. It does not yet change the active mount or release artifacts.

- A normally completed 4096-byte overwrite passed DGX readback and native Mac
  `fsck_apfs -n` plus file readback.
- The Mac-first gate checked **all 22 operations** of that candidate. Every
  original fixture file retained its expected SHA; the modified file was
  always either the complete old or complete new content.
- Native Mac fsck **failed at operations 15–21**. Operations 15–18 had the old
  file; 19–21 had the new file. Space-manager counts and allocation bitmaps
  were inconsistent in those prefixes. The final operation 22 checked clean.
- A separate experiment that wrote the union of old/new allocation bits
  before the NXSB made other prefixes fail; it was reverted and is not in this
  branch.
- Candidate evidence: `evidence/mac-first-release-gate-20260928` on DGX and
  the corresponding Mac `mac-results.json` and `0001`–`0022` fsck logs.
- Actual modified source files: `src/journal.rs` and
  `vendor/apfs-write/src/file.rs`. Added test scripts:
  `tests/make_mac_first_prefixes.py` and
  `tests/check_mac_first_prefixes.py`. No changed schedules, services, mail,
  physical-device blocks or Corsair files.

## Required next change and acceptance

The remaining issue is checkpoint-local allocation state: the old and new
checkpoints must each reference a self-consistent spaceman / CIB / bitmap
graph. Reordering writes to a **shared** bitmap cannot make both checkpoints
valid at every interruption point. Implement native APFS copy-on-write of
affected allocation metadata (including allocation for those copies), correct
free-queue accounting, and a checkpoint publish barrier. Then run this
no-recovery Mac gate over every operation type, repeated transactions,
large/fragmented volumes and actual expendable USB power-cut experiments.
Only after those pass should a new public beta or current Corsair runtime
switch be considered.

This test models an abrupt process death after a completed image write/flush.
It does not model torn sectors, USB bridge write-cache misreporting or loss of
DGX power. Those require separate hardware fault tests.

Primary architecture references: Apple's
[APFS crash-protection FAQ](https://developer.apple.com/library/archive/documentation/FileManagement/Conceptual/APFS_Guide/FAQ/FAQ.html)
and the [APFS reference](https://developer.apple.com/support/apple-file-system/Apple-File-System-Reference.pdf).
