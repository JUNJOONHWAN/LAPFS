# Contributing

Use a disposable image and include a reproducer. For writer changes, test interrupted writes, replay, wrong-device refusal, retained ownership and native Apple fsck/hash consistency. Passing core CI does not establish USB power-loss safety.

Keep unsupported operations explicit. Do not silently bypass identity, snapshot, allocator or recovery guards. Do not post private disk images, credentials, personal file paths, device UUIDs or journals in public issues. Retain logs locally and provide a redacted error, version/hash, filesystem features and minimal synthetic reproducer.

Contributions are licensed under GPL-3.0-only, with compatible third-party notices preserved. Canonical release builds and tags originate on DGX. macOS is an independent validation host.
