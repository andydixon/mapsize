# Functionality and Next Steps Report

Project: **mapsize** v1.0.0 plus the fixes from the security audit of 2026-10-02 (`audits/20261002-1102-securityAudit.md`).

## 1. Executive Summary

mapsize is a well-structured, well-documented codebase with unusually good invariants:

- a single writer for the inventory,
- a deadlock-free bounded scheduler,
- a documented lock discipline,
- a defensive renderer.

The full test suite passes across 11 packages, `go vet` is clean on every target OS, and `govulncheck` reports no vulnerabilities.

The highest-impact opportunities:

1. **Make actions after the scan relative to file descriptors** (scanner, duplicate hashing, reveal), so that what mapsize acts on is provably what it scanned. This is the remaining correctness and security gap when running as root.
2. **Exercise the macOS and Windows paths for real.** Trash and reveal there are implemented but have never been run (ROADMAP 0.8). CI runs `go test` on those OSes, but the platform tests are Linux-only.
3. **Small UX and robustness fixes:**
   - repeated actions on already-trashed nodes,
   - JSON names that cannot be mapped back to real files,
   - an unbounded filter-query nesting depth.

## 2. Current Functional Risks

| Risk | Evidence | User impact |
|---|---|---|
| Trash and reveal on macOS and Windows are untested | ROADMAP 0.8; `ops_darwin.go` and `ops_windows.go` have no tests | First-use failures on those platforms; MS-09 (explorer comma parsing) is unverified |
| Trashing inside an already-trashed directory | `trashDone` (`internal/tui/actions.go`) flags only the trashed node `FlagDeleted`; its descendants stay selectable in some views until a rescan | Normally fails safely, because the path is gone and `Lstat` errors. If someone recreates the same path, the stale aggregates would be subtracted again |
| Scan redirection by symlink races | `platform.ReadDir` opens directories by path (MS-08) | Wrong totals on trees other users can write to |
| `--duplicates` reading from a hung NFS/SMB mount | `fullHash` makes a blocking `read(2)` | It cannot be cancelled (inherent); worth documenting |
| JSON names lossy for non-UTF-8 bytes | `encoding/json` replaces invalid UTF-8 with U+FFFD | Consumers cannot locate those files |
| Filter queries with extreme nesting | Recursive descent in `filter/parser.go` | A self-inflicted crash from a pathological paste |
| Copy refusal for unusual names (new) | `copyPath` refuses anything `Sanitize` would change | Users with legitimately odd names cannot copy them; the info modal shows the escaped form instead |

## 3. Code Quality Improvements

- **Platform trash code.** After this audit, `ensurePrivateDir` and `renameChecked` live in `trash_unix.go`, shared by Linux and macOS. The remaining Linux/macOS differences (trash layout, the `.trashinfo` file) are inherent. Keep any new platform logic behind `internal/platform`, as ARCHITECTURE.md requires.
- **`requestTrash` captures identity in the confirmation closure.** If inode identity is added (security finding MS-11), extend `platform.Expect` rather than adding parameters.
- **Ignored `Write` errors in `snapshot.Save` and `export.JSON`.** These are safe because the final `Flush` reports a sticky error. A short comment saying so would stop future "fixes" from adding noise.
- **`pprof.StartCPUProfile` error ignored** in `cmd/mapsize/main.go`. Report it, so that a profiling run does not silently produce nothing.
- **ARCHITECTURE.md drift.** Its OS-abstraction section names `readdir_unix.go`, `readdir_windows.go` and `fs_*.go`. The actual files are `readdir_bsd.go`, `readdir_other.go`, `sys_*.go` and `mode_unix.go`, plus the new `trash_unix.go` and `open_*.go`. The document says that if the two disagree, one of them should be fixed.

## 4. Frontend Interaction and Error-Handling Improvements

(This is a terminal UI; there is no web frontend.)

- **Show *why* an action was refused, with a next step.** Trash now reports "item changed since the scan … rescan first". Consider offering `r` (rescan) directly from that message.
- **Mark descendants of trashed directories.** Either hide nodes whose ancestor carries `FlagDeleted` in the list, top and duplicate views, or have `requestTrash` refuse them early with "already moved to trash".
- **Duplicate view while hashing.** Esc cancels between files. Showing "waiting on <file>" when one read stalls for more than N seconds would make a hung network mount visible.
- **Copy refusal.** Offer to copy the *escaped* form, clearly labelled, so that users can still reference odd names in scripts.

## 5. Testing Roadmap

- **Platform:**
  - macOS: run the platform tests on the CI `macos-latest` runner. Port `TestTrashFreedesktop`-style tests to `~/.Trash` and to a volume trash on a disk image created with `hdiutil`.
  - Windows: a Recycle Bin smoke test, plus reveal with comma-bearing names (MS-09).
- **Abuse cases:**
  - A scanner race test in which a goroutine toggles a directory and a symlink during a scan, asserting that no node falls outside the root's (dev, ino) closure. This needs MS-08 first.
  - Filter nesting-depth and length limits.
  - A test that `--read-only` blocks every mutating key: `d`, `delete`, and `d` inside the duplicate view.
- **Config:** malformed JSON, out-of-range `refresh_hz`, and bad colours.
- **Regression:** keep running the four tests added in this audit and the existing hostile-snapshot and fuzz targets in CI. Consider a nightly job running `make fuzz` for longer.
- **Load:** the cold-cache HDD benchmark already on the ROADMAP, and a snapshot load benchmark at the new node-limit boundary.

## 6. Recommended New Functionality

- **Safer root workflow:** a `--trash-owned-by-me` option, or a default that refuses to trash items in directories owned by other users when running as root. This prevents most of the TOCTOU class of bugs outright.
- **Lossless export:** add a `name_escaped`/`path_escaped` (or base64) field to JSON and CSV for names that are not valid UTF-8.
- **Snapshot provenance (optional):** `--sign KEY` and `--verify KEY` using Ed25519, so teams that share snapshots can trust their origin. See audit finding MS-13.
- **Configurable keybindings** (already on the ROADMAP).
- **Windows allocated size** through `GetCompressedFileSizeW` (already on the ROADMAP).

## 7. Next-Level Opportunities

- **Descriptor-relative scanner:**
  - Keep a parent fd per pending directory, or reopen with `openat2(RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS)` on Linux 5.6+.
  - Verify (dev, ino) after each open.
  - Store `Ino` for the nodes that need it.
  - This closes MS-08, MS-11 and MS-12 together, and may reduce path-building work in hot loops.
- **Memory:** the name arena on the ROADMAP would also cut the per-node cost behind the snapshot amplification limit.
- **Release pipeline:** build releases in GitHub Actions with SHA-pinned actions, attach Sigstore signatures and SLSA provenance, and publish the Homebrew formula automatically.
- **Accessibility:** the `mono` theme and the non-colour selection cues are good. A screen-reader-friendly `--no-ui` interactive list mode could serve users who cannot use a treemap.

## 8. Prioritised Action Plan

| Horizon | Action | Rationale | Risk |
|---|---|---|---|
| Immediate | Done: the trash identity check, copy guard, FIFO-safe open, snapshot node cap and CI token scope | Closes the Medium findings | Low; all have tests |
| Short | Fix the ARCHITECTURE.md file list, and hide or refuse descendants of trashed nodes | Accurate documentation; avoids confusing stale entries | Very low |
| Short | Run platform tests on macOS and Windows CI; verify the explorer comma behaviour | Untested destructive code paths | Low |
| Short | Pin actions by SHA and add Dependabot | Supply chain | Very low |
| Medium | Descriptor-relative scanning and actions | Removes the remaining TOCTOU class | Medium: touches the scanner hot path, so benchmark against PERFORMANCE.md |
| Medium | Lossless JSON names | Data fidelity | Low: additive field |
| Long | Signed releases and provenance; optional signed snapshots | Trust in artifacts | Low |

## 9. Caveats and Validation Needed

- macOS and Windows behaviour was only cross-compiled with `go vet`, never executed. The Darwin volume-trash change (strict private directory) in particular should be checked on a real external volume. If `.Trashes/<uid>` already exists with different permissions, trash now falls back to an error instead of using it.
- The trash identity check refuses items whose owner or type changed since the scan. Users who `chown` while browsing will see refusals until they rescan; this is intended.
- The snapshot node cap assumes about 15 bytes per node (PERFORMANCE.md). Trees made of extremely repetitive names compress better. The cap allows one node per byte, about 15× headroom, but it is worth validating against the largest real snapshots before the next release.
- The concurrent commit `ab3ff7b` (Homebrew template) landed during the audit and was reviewed.
