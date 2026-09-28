# Physical canary continuation, 2026-09-28

## Before-impact report

The first beta.3 root transition passed the disposable kernel block suite and the selected-volume prepare/cancel check. Its 8,388,617-byte canary was accepted, `fsync` returned, and the mounted readback SHA matched the expected value in 4.84 seconds. The normal FUSE unmount then returned `EBUSY`. The exception handler made a second normal unmount attempt, also `EBUSY`, and retained the live RW mount and session. A second run stopped at its old assumption that any existing mount had to be RO. This was a transition-helper continuation error; it is not evidence of a journal-cap or write failure. The original user files have not been touched by the helper. The canary remains mounted and was independently read back with the same SHA.

The exact task-owned private helper and its resumed session are the only mutation targets. The LAPFS release binary, queue format, device registry, prior recovery journals, other services, and public beta.2 assets stay as they are. The next step must verify the existing process command line and canary before normal unmount; never force/lazy unmount, remove a pending journal, or start a second writer on an already mounted volume.

## After-impact evidence, pending root continuation

Private runtime helper now recognizes the exact existing RW session. It checks process command line, canary size and SHA, retries only normal unmount on transient `EBUSY`, records root-visible mount holders if busy persists, checks the unmounted raw APFS SHA, remounts a fresh session, reads and deletes the canary, compares all original root names, and leaves RW mounted. Python compilation and live PID identity checks passed. Root canary continuation and current native Apple inspection are pending; a mounted readback alone does not complete the transition.
