# sunnyside `bu` and `rs` — Design Spec

**Date:** 2026-06-25  
**Repos:** `sunnyside` (Rust CLI), `anixpkgs` (packaging + tests)

---

## Goal

Add two subcommands to sunnyside for atomic, cryptographic backup and restoration of large file trees:

- `sunnyside bu` — compress, scramble, and store a file or directory as a single opaque blob
- `sunnyside rs` — reverse: descramble and decompress a backup to a destination path

---

## CLI Interface

The existing flat-arg interface is preserved unchanged (backward compatible with `sread`/`swrite`):

```
sunnyside --target <file> --shift <n> --key <char>
```

New subcommands:

```
sunnyside bu --target <file|dir> --shift <n> --key <char> --dest <file>
sunnyside rs --target <file>     --shift <n> --key <char> --dest <path>
```

Clap structure: `struct Cli` with `Option<Commands>` subcommand field plus `Option<String/usize/char>` for the legacy flat args. When `command` is `None`, the legacy path runs (validates that all three flat args are present).

---

## `bu` Operation

**Sequence (4 stages):**

1. **Create temp dir** — `<dest_parent>/.sunnyside_tmp_<dest_filename>` (same filesystem as dest, deterministic name for future resume support). If it already exists from a prior interrupted run, remove and recreate.
2. **Compress** — tar + parallel gzip (`gzp` crate, `num_threads(0)` = all CPUs). Output: `tmp/<target_basename>`. The archive root entry is named `<target_basename>` (so extraction recreates the original name). Files are handled via `tar.append_path_with_name`; directories via `tar.append_dir_all`.
3. **Scramble** — XOR the compressed file in-place in 4 MB chunks using rayon. Same key/XOR logic as the existing sunnyside code.
4. **Shift + finalize** — Rename `tmp/<target_basename>` to `tmp/<shifted_basename>.tyz` (same alphabet rotation as existing code). Then: atomically remove old `--dest` if present, rename shifted file to `--dest`, delete temp dir.

**Invariant:** `--target` is never modified. `--dest` is only replaced after the new file is fully ready.

**Performance at 100 GB (NVMe):**
- Compress (gzp, 8 cores): ~3–5 min
- Scramble (rayon): ~30–60 sec (on compressed output)
- Total: roughly **4–6 minutes**

---

## `rs` Operation

**Sequence (4 stages, symmetric with `bu`):**

1. **Create temp dir** — same naming convention as `bu`, next to `--dest`.
2. **Copy + unshift** — Copy `--target` to `tmp/<target_filename>`. Strip `.tyz` suffix, reverse-shift the basename using the same alphabet rotation (reversed). Rename to `tmp/<unshifted_name>`. (Note: if `--dest` in the original `bu` call was not the naturally-shifted name, the unshifted intermediate name may differ from the original archive root — this is fine; extraction proceeds by archive contents, not filename.)
3. **Descramble** — XOR in-place with same key (XOR is its own inverse). Rayon-parallelized.
4. **Decompress + finalize** — Extract tar.gz to `tmp/extract/`. Identify the single root entry. Atomically remove old `--dest` if present, rename extracted root to `--dest`, delete temp dir.

**Decompression note:** `flate2::GzDecoder` (single-threaded) is used for decompression — decompression is ~3–5× faster than compression, so parallelism is less critical. A 100 GB source compressed to ~60 GB decompresses in ~2 minutes.

---

## Dependencies (Cargo.toml additions)

```toml
gzp = { version = "0.9", default-features = false, features = ["deflate_rust"] }
flate2 = "1.0"
tar = "0.4"
indicatif = "0.17"
```

`gzp` with `deflate_rust` avoids the `libdeflate` C dependency, keeping the build fully in Rust. `flate2` is kept for decompression (still reads standard gzip streams that `gzp` produced).

---

## Progress Output

Four-stage spinner per command using `indicatif::ProgressBar`:

```
⠙ [1/4] Compressing...
✓ [1/4] Compressed
⠙ [2/4] Scrambling...
✓ [2/4] Scrambled
⠙ [3/4] Shifting name...
✓ [3/4] Shifted
⠙ [4/4] Finalizing...
✓ [4/4] Done
```

---

## anixpkgs Changes

### `pkgs/rust-packages/sunnyside/default.nix`
- Bump `version` to `"0.2.0"`
- Update `cargoHash` after Cargo.lock resolves

### `test/test_sunnyside.sh`
Add two new test sections after the existing sunnyside test:

**Test 1 — single file backup/restore:**
```bash
echo "BACKUP_TEST" > original.txt
sunnyside bu -t original.txt -s 4 -k u -d backup.tyz
[[ -f backup.tyz ]] || { echo_red "bu: dest not created"; exit 1; }
sunnyside rs -t backup.tyz -s 4 -k u -d restored.txt
[[ "$(cat restored.txt)" == "BACKUP_TEST" ]] || { echo_red "rs: file content mismatch"; exit 1; }
```

**Test 2 — directory backup/restore:**
```bash
mkdir -p testdir/sub
echo "NESTED" > testdir/sub/file.txt
sunnyside bu -t testdir -s 4 -k u -d dir_backup.tyz
[[ -f dir_backup.tyz ]] || { echo_red "bu: dir dest not created"; exit 1; }
sunnyside rs -t dir_backup.tyz -s 4 -k u -d testdir_restored
[[ "$(cat testdir_restored/sub/file.txt)" == "NESTED" ]] || { echo_red "rs: dir content mismatch"; exit 1; }
```

---

## Out of Scope

- Resume from interrupted run (stretch goal — temp dir name is deterministic, wiring up resume logic is future work)
- Parallel decompression

---

## Version Bump

`sunnyside` Cargo.toml: `0.1.0` → `0.2.0`
