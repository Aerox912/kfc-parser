# Enshrouded 1.0 Implementation Plan

> **For agentic workers:** Use superpowers:executing-plans to implement this plan task-by-task. Keep the selected Light route; the parent implements and a fresh reviewer checks the final changes.

**Goal:** Deliver reproducible loader and four mod compatibility fixes on Nick's GitHub repositories.

**Architecture:** Reuse the pinned KFC4 parser work. Make incremental writes preserve the published dataset until directory publication; use existing patch-only delivery for the three mods.

**Tech Stack:** Rust, vendored Lua 5.4, PowerShell, Python unittest.

**Spec:** ../specs/2026-10-15-enshrouded-1.0.md

## Global constraints

- Keep installed game/config/saves/servers unchanged.
- Use fresh copied fixtures, no original mod/game redistribution, and preserve upstream attribution.
- Do not claim native/gameplay/multiplayer acceptance from offline tests.
- Preserve compatibility identifiers and independently owned ESM implementations.

## Review focus

- A later failed mod run must not overwrite currently referenced resource/content bytes.
- Concurrent writers and failed publication must preserve the current directory.
- Backup replacement and CLI failures must report unsuccessful outcomes.
- Updated content references must retain category, bundle and sorting metadata.
- Creative 1.0 guards must validate the actual replacement schema rather than skip missing components.

### Task 1: Loader compatibility and safe publication

Files: Cargo.toml/lock, kfc-base container writer and tests, mod-loader-lua backup helper and release callback example, mod-manager-cli command tests, dbghelp-proxy init.

- [x] Add failing synthetic writer tests for repeated/failed writes, concurrent writer rejection and atomic directory publication.
- [x] Preserve referenced bytes with append-only resource writes and unused content slots; flush before atomic directory replacement; hold a writer lock.
- [x] Replace backups atomically and propagate CLI failures. Reject unsupported CLI runtime loading explicitly. Return failed proxy initialization instead of silently disabling required mods.
- [x] Bring forward the demonstrated release callback regression and unwind correction.
- [x] Run the workspace suite and the actual release callback probe, then record results and commit the bounded loader change.

### Task 2: Mod schema migrations including Rested

Files in isolated enshrouded-mod-patches worktree: compatibility transformation module, recipes, tests, documentation. Original sources remain outside Git.

- [x] Export exact 1.0 reference fields and Creative template contracts from a fresh copied fixture.
- [x] Add failing transformation tests against pinned originals and invalid/altered inputs.
- [x] Implement explicit localization, SoundResource and Creative schema migrations, preserving metadata/guards and custom PCM.
- [x] Verify individual results, then rebuild the original complete mod set in a fresh copied fixture with needed content archives.
- [x] Run the patch repository suite and commit the bounded compatibility changes.

### Task 3: Review and delivery

- [x] Obtain one independent review of both diffs and reconcile material findings with regression tests.
- [ ] Build local candidate artifacts with hashes, provenance, instructions and clear runtime limitations.
- [ ] Push dedicated fix branches to Nick's repositories and provide reviewable pull requests; do not merge or publish a game release.
- [ ] Update NIC-498/Notion and inform the existing ESM Loader chat of exact commits and adoption requirements.

Nick expanded Task 2 on 2026-10-15 to include Rested Enhanced. Validate the 1.0 variable-slot relocation, complete layout/identity/data contract, native node signatures and attribute targets before accepting its original enhanced payload. Keep apply/revert effects and Hoarder localization composition.
