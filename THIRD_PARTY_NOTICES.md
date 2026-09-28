# Third-party notices

LAPFS is distributed under GPL-3.0-only. Dependency notices and original license files remain in the source directories. Cargo.lock pins the exact versions; dependencies/ includes registry source for offline rebuilding. This inventory includes build/test and platform-specific dependencies, not just code linked into Linux ARM64.

## APFS implementation

The APFS parser and writer derive from [enesilhaydin/apfs-explorer](https://github.com/enesilhaydin/apfs-explorer) at commit `0ef6cd705ac3cca4efc953adbb99f705d04fe440`, GPL-3.0-only. Source: vendor/apfs-core, vendor/apfs, vendor/apfs-write. The original license is preserved in licenses/APFS-UPSTREAM-GPL-3.0.txt. Local modifications and their date are documented in [UPSTREAM.md](UPSTREAM.md). The upstream project does not endorse this beta.

## FUSE

[fuser 0.16.0](https://github.com/cberner/fuser/tree/v0.16.0), MIT. Copyright (c) 2020-present Christopher Berner; Copyright © 2013-2019 Andreas Neuhaus. See the authoritative [original notice](vendor/fuser/LICENSE.md); names/dates in that file control. The local build.rs change uses CARGO_CFG_TARGET_OS for cross-compilation.

## Registry dependencies

Original license expressions below are from locked Cargo metadata. Alternative-license expressions are preserved; this table does not replace the files or grant new rights. For Unicode data, preserve the Unicode-3.0 license in unicode-ident. Where a package has no standalone license file, its source headers and declared upstream license remain part of the bundled source.

| Package | Version | Declared license | Upstream |
|---|---|---|---|
| `anyhow` | 1.0.104 | MIT OR Apache-2.0 | [source](https://github.com/dtolnay/anyhow) |
| `bitflags` | 2.13.2 | MIT OR Apache-2.0 | [source](https://github.com/bitflags/bitflags) |
| `block-buffer` | 0.10.4 | MIT OR Apache-2.0 | [source](https://github.com/RustCrypto/utils) |
| `cfg-if` | 1.0.5 | MIT OR Apache-2.0 | [source](https://github.com/rust-lang/cfg-if) |
| `cfg_aliases` | 0.2.2 | MIT | [source](https://github.com/katharostech/cfg_aliases) |
| `cpufeatures` | 0.2.17 | MIT OR Apache-2.0 | [source](https://github.com/RustCrypto/utils) |
| `crypto-common` | 0.1.7 | MIT OR Apache-2.0 | [source](https://github.com/RustCrypto/traits) |
| `digest` | 0.10.7 | MIT OR Apache-2.0 | [source](https://github.com/RustCrypto/traits) |
| `errno` | 0.3.14 | MIT OR Apache-2.0 | [source](https://github.com/lambda-fairy/rust-errno) |
| `fastrand` | 2.5.0 | Apache-2.0 OR MIT | [source](https://github.com/smol-rs/fastrand) |
| `generic-array` | 0.14.7 | MIT | [source](https://github.com/fizyk20/generic-array.git) |
| `getrandom` | 0.4.3 | MIT OR Apache-2.0 | [source](https://github.com/rust-random/getrandom) |
| `itoa` | 1.0.18 | MIT OR Apache-2.0 | [source](https://github.com/dtolnay/itoa) |
| `libc` | 0.2.189 | MIT OR Apache-2.0 | [source](https://github.com/rust-lang/libc) |
| `linux-raw-sys` | 0.12.1 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | [source](https://github.com/sunfishcode/linux-raw-sys) |
| `log` | 0.4.34 | MIT OR Apache-2.0 | [source](https://github.com/rust-lang/log) |
| `memchr` | 2.8.3 | Unlicense OR MIT | [source](https://github.com/BurntSushi/memchr) |
| `nix` | 0.29.0 | MIT | [source](https://github.com/nix-rust/nix) |
| `once_cell` | 1.21.4 | MIT OR Apache-2.0 | [source](https://github.com/matklad/once_cell) |
| `page_size` | 0.6.0 | MIT/Apache-2.0 | [source](https://github.com/Elzair/page_size_rs) |
| `pin-project-lite` | 0.2.17 | Apache-2.0 OR MIT | [source](https://github.com/taiki-e/pin-project-lite) |
| `proc-macro2` | 1.0.107 | MIT OR Apache-2.0 | [source](https://github.com/dtolnay/proc-macro2) |
| `quote` | 1.0.47 | MIT OR Apache-2.0 | [source](https://github.com/dtolnay/quote) |
| `r-efi` | 6.0.0 | MIT OR Apache-2.0 OR LGPL-2.1-or-later | [source](https://github.com/r-efi/r-efi) |
| `rustix` | 1.1.5 | Apache-2.0 WITH LLVM-exception OR Apache-2.0 OR MIT | [source](https://github.com/bytecodealliance/rustix) |
| `serde` | 1.0.229 | MIT OR Apache-2.0 | [source](https://github.com/serde-rs/serde) |
| `serde_core` | 1.0.229 | MIT OR Apache-2.0 | [source](https://github.com/serde-rs/serde) |
| `serde_derive` | 1.0.229 | MIT OR Apache-2.0 | [source](https://github.com/serde-rs/serde) |
| `serde_json` | 1.0.151 | MIT OR Apache-2.0 | [source](https://github.com/serde-rs/json) |
| `sha2` | 0.10.9 | MIT OR Apache-2.0 | [source](https://github.com/RustCrypto/hashes) |
| `smallvec` | 1.16.2 | MIT OR Apache-2.0 | [source](https://github.com/servo/rust-smallvec) |
| `syn` | 2.0.119 | MIT OR Apache-2.0 | [source](https://github.com/dtolnay/syn) |
| `syn` | 3.0.6 | MIT OR Apache-2.0 | [source](https://github.com/dtolnay/syn) |
| `tempfile` | 3.27.0 | MIT OR Apache-2.0 | [source](https://github.com/Stebalien/tempfile) |
| `thiserror` | 2.0.21 | MIT OR Apache-2.0 | [source](https://github.com/dtolnay/thiserror) |
| `thiserror-impl` | 2.0.21 | MIT OR Apache-2.0 | [source](https://github.com/dtolnay/thiserror) |
| `tinyvec` | 1.13.3 | Zlib OR Apache-2.0 OR MIT | [source](https://github.com/Lokathor/tinyvec) |
| `tracing` | 0.1.44 | MIT | [source](https://github.com/tokio-rs/tracing) |
| `tracing-attributes` | 0.1.31 | MIT | [source](https://github.com/tokio-rs/tracing) |
| `tracing-core` | 0.1.36 | MIT | [source](https://github.com/tokio-rs/tracing) |
| `typenum` | 1.20.1 | MIT OR Apache-2.0 | [source](https://github.com/paholg/typenum) |
| `unicode-ident` | 1.0.26 | (MIT OR Apache-2.0) AND Unicode-3.0 | [source](https://github.com/dtolnay/unicode-ident) |
| `unicode-normalization` | 0.1.25 | MIT OR Apache-2.0 | [source](https://github.com/unicode-rs/unicode-normalization) |
| `version_check` | 0.9.5 | MIT/Apache-2.0 | [source](https://github.com/SergioBenitez/version_check) |
| `winapi` | 0.3.9 | MIT/Apache-2.0 | [source](https://github.com/retep998/winapi-rs) |
| `winapi-i686-pc-windows-gnu` | 0.4.0 | MIT/Apache-2.0 | [source](https://github.com/retep998/winapi-rs) |
| `winapi-x86_64-pc-windows-gnu` | 0.4.0 | MIT/Apache-2.0 | [source](https://github.com/retep998/winapi-rs) |
| `windows-link` | 0.2.1 | MIT OR Apache-2.0 | [source](https://github.com/microsoft/windows-rs) |
| `windows-sys` | 0.61.2 | MIT OR Apache-2.0 | [source](https://github.com/microsoft/windows-rs) |
| `zerocopy` | 0.8.59 | BSD-2-Clause OR Apache-2.0 OR MIT | [source](https://github.com/google/zerocopy) |
| `zerocopy-derive` | 0.8.59 | BSD-2-Clause OR Apache-2.0 OR MIT | [source](https://github.com/google/zerocopy) |
| `zmij` | 1.0.23 | MIT | [source](https://github.com/dtolnay/zmij) |

| `cc` | 1.5.1 | MIT OR Apache-2.0 | [source](https://github.com/rust-lang/cc-rs) |
| `find-msvc-tools` | 0.1.14 | MIT OR Apache-2.0 | [source](https://github.com/rust-lang/cc-rs) |
| `sha2-asm` | 0.6.4 | MIT | [source](https://github.com/RustCrypto/asm-hashes) |
| `shlex` | 2.0.1 | MIT OR Apache-2.0 | [source](https://github.com/comex/rust-shlex) |
