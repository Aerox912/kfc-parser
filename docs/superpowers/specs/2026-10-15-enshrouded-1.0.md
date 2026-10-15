# Enshrouded 1.0 compatibility repair

Nick authorized a GitHub fork and a fixed loader plus compatibility patches for Hoarder's Helper, Electric Blink and Creative Gem Forges. Work is isolated; the installed game, configurations, saves and hosted servers remain unchanged.

The fork starts from upstream compatibility PR29, commit f9d45d8b7da06ed9314429fba8cbf555bcaf15e4. Preserve its authorship and GPL notices. Retain binary/API identifiers. Document this as an ESM-maintained EML compatibility component, not a replacement for the independently owned ESM Loader.

Acceptance: KFC4 parsing/serialization against client build 25936191; ordinary Lua callback errors must not abort an MSVC release executable; failed preparation must preserve the currently published directory and referenced bytes; publication must report errors; CLI failures must have unsuccessful status; required-mod failures must not silently launch unmodded. Preserve KFC3 behavior covered by fixtures. Mod migrations must preserve content-reference metadata and Creative's template guards, and pass a fresh complete-set copied-data run. Native game execution, audio playback, saves and multiplayer require separate runtime acceptance.

Patch-only delivery for Nexus originals uses exact input/output hashes in Aerox912/enshrouded-mod-patches. Do not redistribute original mod sources or game assets. Existing ESM module ownership remains unchanged.

Nick explicitly added Rested Enhanced compatibility on 2026-10-15. Preserve its effects and strict revision guards; source/build/offline results do not prove native effect apply/expiry/reapply behavior.
