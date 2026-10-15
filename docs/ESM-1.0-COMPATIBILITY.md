# ESM EML compatibility candidate for Enshrouded 1.0

This fork preserves Brabb3l and contributors' Enshrouded Mod Loader (EML), EMM,
binary identifiers and GPL-3.0-or-later license. KFC4 parsing originates in
[upstream PR 29](https://github.com/Brabb3l/kfc-parser/pull/29), pinned at
`f9d45d8b7da06ed9314429fba8cbf555bcaf15e4`; that history is retained. The ESM
maintenance changes address failed startup and safe offline patch publication.

## Behavior

- KFC4 support is inherited from the pinned upstream work; existing KFC3 tests remain.
- Incremental resources append to the physical file. New content avoids DAT slots
  referenced by the published directory. Failed runs may leave unused bytes, but
  must preserve published references. Repeated builds can grow the scratch data.
- Data is flushed before atomic directory replacement. Backup replacement is also
  atomic. A shared dataset lease covers cache inspection, backup creation,
  preparation, publication and cache recording. Restore and competing writers fail
  while that lease is held. The `.eml-lock` sidecar persists intentionally.
- Invalid headers and CLI failures produce errors. CLI `--runtime` is explicitly
  unsupported. Required Lua failures propagate through proxy initialization;
  returning failure from the native proxy is source-verified only.
- Release panic unwinding allows vendored Lua 5.4 to catch a Rust callback error on
  MSVC. The release error-boundary probe verifies this path without launching a game.

## Verified scope, 15 October 2026

Client Steam build `25936191`, executable SHA-256
`b331a6d037c5c79314b57ec49f2f4958db89dc5a637d925aeb9ec576b1f64ba7`, data tag
`1265863|^/game38/branches/release_1.0|2026-10-14T14:03:32.164608Z`.

- Windows x64 / Rust 1.98.0 MSVC: 54 Rust tests passed; five pre-existing tests
  requiring external game fixtures were ignored. Release binaries build and the
  Rust-to-Lua callback error probe passes.
- Synthetic failure tests preserve the published directory and resource/content
  bytes, reject competing writers and wrong-dataset leases, and reject backup or
  restore during the interval between directory publication and cache recording.
- The copied installed mod set, with four migrated scripts, applies 4,912 resources.
  Independent parsing reads all 4,912 changed resources and validates seven new
  content hashes. This is patch execution evidence, not whole-mod gameplay support.
- A later injected Rested conflict exits with status 1, preserving the published
  directory, backup, resource prefix and referenced copied content archives.
- The patch repository records recipe input/output hashes, schema checks and the
  separate Rested static contract. See
  [ESM 1.0 mod compatibility](https://github.com/Aerox912/enshrouded-mod-patches/blob/fix/enshrouded-1.0/docs/ESM-1.0-COMPATIBILITY.md).

One independent Superpowers code review identified two transaction-boundary races.
Both were fixed with regressions. ChatGPT Surface consultation covered publication,
Rested relocation, reference resolution and Hoarder localization coexistence.
The implementation retains the author's enhanced Rested payload after static
checks; unseen native semantic changes remain a runtime acceptance risk.

## Reproduction on a disposable copy

Preserve matching executable, pristine KFC backup, resources and the original DAT
archives required by selected mods. Do not mix versions or take a patched directory
as the vanilla baseline. Obtain original mods from their authors and apply the
hash-pinned recipes in the companion patch repository to separate output files.
Keep private exports and original game/mod data out of source control.

```powershell
$env:CARGO_TARGET_DIR = 'E:\Build\esm-eml\cargo-target'
cargo test --workspace
cargo build --release -p mod-manager-cli -p dbghelp-proxy
cargo run --release -p mod-loader-lua --example error_boundary_probe
& "$env:CARGO_TARGET_DIR\release\emm.exe" run -g E:\Scratch\game-copy --file-name enshrouded --patch --force
cargo run -p kfc --example verify_patch -- E:\Scratch\game-copy E:\Scratch\game-copy\enshrouded.kfc.bak E:\Scratch\game-copy\.cache\types.json E:\Scratch\new-private-readback.json
```

Readback output must be a new file. It contains proprietary game data and must
remain private. Diagnostic examples are offline tools, not runtime adapters.

## Adoption gates

No live installation, settings, save or hosted server was changed for this work.
Do not infer native DLL startup, audio playback, forge placement, Rested expiry,
reapplication, save compatibility or multiplayer authority from these checks.
Other mods may execute Lua successfully while silently finding no old-schema assets.

Dedicated server build 25936466 has not been validated by this candidate. Client
assets must never be copied over server assets. The separate server compatibility
DLL's executable guards and native RVAs are outside this repair. In particular,
server stack-size behavior must be validated before loading a valuable world;
the copied client run is not that validation.

For ESM adoption, retain mod IDs and author credits, preserve the nested content
metadata, and use the new nested buff/attribute registries. This component does
not supply the separate ESM native lifecycle or asset API. Package promotion and
runtime acceptance belong to the existing ESM/deployment owners.
