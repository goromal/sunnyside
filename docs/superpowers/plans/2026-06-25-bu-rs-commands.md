# sunnyside bu/rs Commands Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers-extended-cc:subagent-driven-development (recommended) or superpowers-extended-cc:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add `bu` (backup) and `rs` (restore) subcommands to sunnyside that atomically compress, XOR-scramble, and store/restore file trees.

**Architecture:** `main.rs` is restructured around an `Option<Commands>` clap subcommand enum; when no subcommand is passed the existing flat-arg legacy path runs unchanged. `bu` and `rs` each operate entirely in a deterministic temp dir next to `--dest` (same filesystem), only replacing the destination once the new file is fully ready. Compression uses `gzp` for parallel gzip; decompression uses `flate2::MultiGzDecoder` (required for multi-member gzip output from `gzp`).

**Tech Stack:** Rust, clap 4.2 (derive), gzp 0.9 (parallel gzip), flate2 1.0 (decompression), tar 0.4, indicatif 0.17 (progress spinners), rayon (XOR parallelism)

---

## File Map

| File | Action |
|------|--------|
| `sunnyside/Cargo.toml` | Add deps, bump version to 0.2.0 |
| `sunnyside/src/main.rs` | Substantial rewrite — new CLI structure + implementations |
| `anixpkgs/pkgs/rust-packages/sunnyside/default.nix` | Version bump + cargoHash update |
| `anixpkgs/test/test_sunnyside.sh` | Add bu/rs regression test sections |

---

### Task 1: Add dependencies and restructure CLI with shared helpers

**Goal:** Update Cargo.toml with new dependencies and rewrite main.rs with the new clap subcommand structure and all shared helper functions, while keeping the legacy flat-arg mode working exactly as before.

**Files:**
- Modify: `sunnyside/Cargo.toml`
- Modify: `sunnyside/src/main.rs`

**Acceptance Criteria:**
- [ ] `cargo build` succeeds with zero errors
- [ ] `./target/debug/sunnyside --help` lists both `bu` and `rs` subcommands
- [ ] `./target/debug/sunnyside bu --help` shows `--target`, `--shift`, `--key`, `--dest` flags
- [ ] Legacy mode still works: `echo hi > /tmp/ss_test.txt && ./target/debug/sunnyside -t /tmp/ss_test.txt -s 0 -k x` produces `/tmp/hh.tyz` (same behavior as before)

**Verify:** `cargo build 2>&1 | tail -2` → `Finished dev [unoptimized + debuginfo] target(s) in X.XXs`

**Steps:**

- [ ] **Step 1: Update Cargo.toml**

Replace `sunnyside/Cargo.toml` entirely with:

```toml
[package]
name = "sunnyside"
version = "0.2.0"
edition = "2021"

[dependencies]
rayon = "1.10.0"
indicatif = "0.17"
flate2 = "1.0"
tar = "0.4"

[dependencies.clap]
version = "=4.2.0"
features = ["derive"]

[dependencies.gzp]
version = "0.9"
default-features = false
features = ["deflate_rust"]
```

- [ ] **Step 2: Rewrite main.rs with CLI structure and shared helpers**

Replace `sunnyside/src/main.rs` entirely with:

```rust
use clap::{Args, Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process,
};

// ── CLI ──────────────────────────────────────────────────────────────────────

#[derive(Parser, Debug)]
#[command(author, version, about = "Make some scrambled eggs.", long_about = None)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
    /// File target (legacy mode).
    #[arg(short, long)]
    target: Option<String>,
    /// Shift amount (legacy mode).
    #[arg(short, long)]
    shift: Option<usize>,
    /// Scramble key (legacy mode).
    #[arg(short, long)]
    key: Option<char>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Backup: compress, scramble, and store a file or directory tree.
    Bu(BuArgs),
    /// Restore: descramble and decompress a backup.
    Rs(RsArgs),
}

#[derive(Args, Debug)]
struct BuArgs {
    /// Source file or directory to back up.
    #[arg(short, long)]
    target: String,
    /// Shift amount.
    #[arg(short, long)]
    shift: usize,
    /// Scramble key.
    #[arg(short, long)]
    key: char,
    /// Destination backup file path.
    #[arg(short, long)]
    dest: String,
}

#[derive(Args, Debug)]
struct RsArgs {
    /// Backup file to restore.
    #[arg(short, long)]
    target: String,
    /// Shift amount (must match the bu call).
    #[arg(short, long)]
    shift: usize,
    /// Scramble key (must match the bu call).
    #[arg(short, long)]
    key: char,
    /// Destination path to restore to.
    #[arg(short, long)]
    dest: String,
}

// ── SHARED HELPERS ───────────────────────────────────────────────────────────

fn alphabet() -> Vec<char> {
    ('a'..='z')
        .chain('A'..='Z')
        .chain(std::iter::once('.'))
        .chain('0'..='9')
        .collect()
}

fn shift_name(name: &str, shift: usize, a: &[char]) -> String {
    let n = a.len();
    let s = shift % n;
    let tbl: HashMap<char, char> = a
        .iter()
        .enumerate()
        .map(|(i, &c)| (c, a[(i + s) % n]))
        .collect();
    name.chars().map(|c| *tbl.get(&c).unwrap_or(&c)).collect()
}

fn unshift_name(name: &str, shift: usize, a: &[char]) -> String {
    let n = a.len();
    let s = shift % n;
    let tbl: HashMap<char, char> = a
        .iter()
        .enumerate()
        .map(|(i, &c)| (c, a[(i + n - s) % n]))
        .collect();
    name.chars().map(|c| *tbl.get(&c).unwrap_or(&c)).collect()
}

fn scramble_inplace(path: &Path, key: char) -> io::Result<()> {
    let lve = key as u8;
    let mut file = OpenOptions::new().read(true).write(true).open(path)?;
    let mut buf = vec![0u8; 4 * 1024 * 1024];
    let mut pos = 0u64;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let out: Vec<u8> = buf[..n].par_iter().map(|&b| b ^ lve).collect();
        file.seek(SeekFrom::Start(pos))?;
        file.write_all(&out)?;
        pos += n as u64;
    }
    Ok(())
}

fn spinner(msg: &str) -> ProgressBar {
    let pb = ProgressBar::new_spinner();
    pb.set_style(
        ProgressStyle::with_template("{spinner:.cyan} {msg}")
            .unwrap()
            .tick_strings(&["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"]),
    );
    pb.set_message(msg.to_string());
    pb.enable_steady_tick(std::time::Duration::from_millis(80));
    pb
}

fn tmp_dir_for(dest: &Path) -> PathBuf {
    let parent = dest.parent().unwrap_or(Path::new("."));
    let name = dest.file_name().unwrap().to_str().unwrap();
    parent.join(format!(".sunnyside_tmp_{}", name))
}

// ── BU / RS (stubs — implemented in Tasks 2 and 3) ──────────────────────────

fn do_bu(_args: &BuArgs) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Other, "bu not yet implemented"))
}

fn do_rs(_args: &RsArgs) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Other, "rs not yet implemented"))
}

// ── MAIN ─────────────────────────────────────────────────────────────────────

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    if let Some(cmd) = cli.command {
        return match cmd {
            Commands::Bu(args) => do_bu(&args),
            Commands::Rs(args) => do_rs(&args),
        };
    }

    // Legacy flat-arg mode (backward compat with sread/swrite)
    let target = cli.target.unwrap_or_else(|| {
        eprintln!("error: --target required");
        process::exit(1);
    });
    let shift = cli.shift.unwrap_or_else(|| {
        eprintln!("error: --shift required");
        process::exit(1);
    });
    let key = cli.key.unwrap_or_else(|| {
        eprintln!("error: --key required");
        process::exit(1);
    });

    let a: Vec<char> = ('a'..='z')
        .chain('A'..='Z')
        .chain(std::iter::once('.'))
        .chain('0'..='9')
        .collect();
    let ext: &str = ".tyz";

    if !Path::new(&target).exists() {
        eprintln!("Specified source does not exist: {}", &target);
        process::exit(1);
    }

    if !&target.chars().all(|c| a.contains(&c)) {
        eprintln!("Letters, numbers, and dots only, please.");
        process::exit(1);
    }

    let mut cvt: bool = true;
    if (&target).contains(&ext) {
        println!("...and back again.");
        cvt = false;
    } else {
        println!("There...")
    }

    let mut srcp: String = String::new();
    let (a_left, a_right) = a.split_at(shift);
    let a_s: Vec<_> = a_right.iter().chain(a_left.iter()).cloned().collect();
    let mut from_chars: Vec<char> = Vec::new();
    let mut to_chars: Vec<char> = Vec::new();

    if !cvt {
        srcp = target.replace(ext, "");
        from_chars = a_s.clone();
        to_chars = a.clone();
    } else {
        srcp = target.to_string();
        from_chars = a.clone();
        to_chars = a_s.clone();
    }

    let translation_table: HashMap<char, char> = from_chars
        .iter()
        .zip(to_chars.iter())
        .map(|(&from, &to)| (from, to))
        .collect();
    let mut tf: String = srcp
        .chars()
        .map(|c| *translation_table.get(&c).unwrap_or(&c))
        .collect();

    if cvt {
        tf.push_str(ext);
    }

    println!("{} -> {}", target, tf);

    let mut inf = File::open(target)?;
    let mut outf = File::create(tf)?;
    let lve: u8 = key as u8;

    let mut buffer = [0; 4096];
    loop {
        let bytes_read = inf.read(&mut buffer)?;
        if bytes_read == 0 {
            break;
        }
        let scrambled_bytes: Vec<_> = buffer[..bytes_read]
            .par_iter()
            .map(|&byte| byte ^ lve)
            .collect();
        outf.write_all(&scrambled_bytes)?;
    }

    Ok(())
}
```

- [ ] **Step 3: Verify build and legacy behavior**

```bash
cd sunnyside
cargo build 2>&1 | tail -3
```
Expected: `Finished dev [unoptimized + debuginfo] target(s) in X.XXs`

```bash
./target/debug/sunnyside --help | head -8
```
Expected output contains `bu` and `rs` subcommand names.

```bash
echo "HELLO" > /tmp/ss_test.txt
cd /tmp && /path/to/sunnyside/target/debug/sunnyside -t ss_test.txt -s 0 -k x
ls ss_test.tyz
```
Expected: `ss_test.tyz` exists (legacy mode unchanged; shift=0 means name unchanged, key=x XORs content).

- [ ] **Step 4: Commit**

```bash
cd sunnyside
git add Cargo.toml Cargo.lock src/main.rs
git commit -m "feat: add subcommand CLI structure and shared helpers for bu/rs"
```

---

### Task 2: Implement `bu` subcommand

**Goal:** Implement `compress_target` (parallel gzip tar archive) and `do_bu` (4-stage backup pipeline), replacing the Task 1 stub.

**Files:**
- Modify: `sunnyside/src/main.rs` (add gzp import, compress_target function, replace do_bu stub)

**Acceptance Criteria:**
- [ ] `cargo build` succeeds
- [ ] `sunnyside bu -t <file> -s 4 -k u -d /tmp/bu_test.tyz` creates `/tmp/bu_test.tyz`
- [ ] `sunnyside bu -t <dir> -s 4 -k u -d /tmp/bu_dir.tyz` creates `/tmp/bu_dir.tyz`
- [ ] No temp dir remains after successful bu
- [ ] `--target` file/directory is not modified by bu
- [ ] Running bu twice replaces the old dest atomically (no partial states)

**Verify:** `cargo build && echo TEST > /tmp/bu_src.txt && ./target/debug/sunnyside bu -t /tmp/bu_src.txt -s 4 -k u -d /tmp/bu_out.tyz && ls -lh /tmp/bu_out.tyz` → shows file exists with non-zero size

**Steps:**

- [ ] **Step 1: Add gzp import at top of main.rs**

In `sunnyside/src/main.rs`, add to the existing `use` block at the top:

```rust
use gzp::{deflate::Gzip, ZBuilder};
```

The top of the file should now have these use statements:

```rust
use clap::{Args, Parser, Subcommand};
use gzp::{deflate::Gzip, ZBuilder};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process,
};
```

- [ ] **Step 2: Add `compress_target` function**

Insert `compress_target` after `tmp_dir_for` and before the `do_bu` stub in `main.rs`:

```rust
fn compress_target(src: &Path, dst: &Path) -> io::Result<()> {
    let out = File::create(dst)?;
    let parz = ZBuilder::<Gzip, _>::new().num_threads(0).from_writer(out);
    let mut tar_builder = tar::Builder::new(parz);
    let name = src.file_name().unwrap();
    if src.is_dir() {
        tar_builder.append_dir_all(name, src)?;
    } else {
        tar_builder.append_path_with_name(src, name)?;
    }
    tar_builder.into_inner()?.finish()?;
    Ok(())
}
```

- [ ] **Step 3: Replace the `do_bu` stub with full implementation**

Replace:
```rust
fn do_bu(_args: &BuArgs) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Other, "bu not yet implemented"))
}
```

With:
```rust
fn do_bu(args: &BuArgs) -> io::Result<()> {
    let a = alphabet();
    let src = Path::new(&args.target);
    let dst = Path::new(&args.dest);

    if !src.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("target does not exist: {}", args.target),
        ));
    }

    if let Some(p) = dst.parent() {
        if !p.as_os_str().is_empty() {
            fs::create_dir_all(p)?;
        }
    }

    let tmp = tmp_dir_for(dst);
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    fs::create_dir_all(&tmp)?;

    let src_name = src.file_name().unwrap().to_str().unwrap().to_string();

    let pb = spinner("[1/4] Compressing...");
    let compressed = tmp.join(&src_name);
    compress_target(src, &compressed)?;
    pb.finish_with_message("[1/4] Compressed  ✓");

    let pb = spinner("[2/4] Scrambling...");
    scramble_inplace(&compressed, args.key)?;
    pb.finish_with_message("[2/4] Scrambled   ✓");

    let pb = spinner("[3/4] Shifting name...");
    let shifted_name = format!("{}.tyz", shift_name(&src_name, args.shift, &a));
    fs::rename(&compressed, tmp.join(&shifted_name))?;
    pb.finish_with_message("[3/4] Shifted     ✓");

    let pb = spinner("[4/4] Finalizing...");
    if dst.exists() {
        if dst.is_dir() {
            fs::remove_dir_all(dst)?;
        } else {
            fs::remove_file(dst)?;
        }
    }
    fs::rename(tmp.join(&shifted_name), dst)?;
    fs::remove_dir_all(&tmp)?;
    pb.finish_with_message("[4/4] Done        ✓");

    Ok(())
}
```

- [ ] **Step 4: Build and test with a file**

```bash
cd sunnyside
cargo build 2>&1 | tail -2
echo "BACKUP_TEST_CONTENT" > /tmp/bu_src.txt
./target/debug/sunnyside bu -t /tmp/bu_src.txt -s 4 -k u -d /tmp/bu_file.tyz
ls -lh /tmp/bu_file.tyz
```
Expected: `/tmp/bu_file.tyz` exists with non-zero size. `/tmp/bu_src.txt` still exists unchanged.

Verify no temp dir left behind:
```bash
ls /tmp/ | grep sunnyside_tmp
```
Expected: empty output.

- [ ] **Step 5: Test with a directory**

```bash
mkdir -p /tmp/bu_testdir/subdir
echo "NESTED_FILE" > /tmp/bu_testdir/subdir/data.txt
echo "ROOT_FILE" > /tmp/bu_testdir/root.txt
./target/debug/sunnyside bu -t /tmp/bu_testdir -s 4 -k u -d /tmp/bu_dir.tyz
ls -lh /tmp/bu_dir.tyz
```
Expected: `/tmp/bu_dir.tyz` exists. Source dir `/tmp/bu_testdir` unchanged.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs Cargo.lock
git commit -m "feat: implement bu subcommand with parallel gzip compression"
```

---

### Task 3: Implement `rs` subcommand

**Goal:** Implement `extract_archive` (multi-member gzip decompression + tar extraction) and `do_rs` (4-stage restore pipeline), replacing the Task 1 stub. Must use `MultiGzDecoder` because `gzp` emits concatenated multi-member gzip.

**Files:**
- Modify: `sunnyside/src/main.rs` (add flate2 + BufReader imports, extract_archive function, replace do_rs stub)

**Acceptance Criteria:**
- [ ] `cargo build` succeeds
- [ ] Round-trip test: `bu` a file then `rs` it → restored file content matches original
- [ ] Round-trip test: `bu` a directory then `rs` it → restored directory structure and file contents match original
- [ ] No temp dir remains after successful rs
- [ ] `--target` backup file is not modified or deleted by rs

**Verify:** `echo "ROUNDTRIP" > /tmp/rt_src.txt && ./target/debug/sunnyside bu -t /tmp/rt_src.txt -s 4 -k u -d /tmp/rt_backup.tyz && ./target/debug/sunnyside rs -t /tmp/rt_backup.tyz -s 4 -k u -d /tmp/rt_restored.txt && cat /tmp/rt_restored.txt` → `ROUNDTRIP`

**Steps:**

- [ ] **Step 1: Add flate2 and BufReader imports**

In `sunnyside/src/main.rs`, add to the top-level use block:

```rust
use flate2::read::MultiGzDecoder;
```

Also add `BufReader` to the std imports. The full use block at the top should now be:

```rust
use clap::{Args, Parser, Subcommand};
use flate2::read::MultiGzDecoder;
use gzp::{deflate::Gzip, ZBuilder};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    process,
};
```

- [ ] **Step 2: Add `extract_archive` function**

Insert after `compress_target` and before the `do_bu` implementation:

```rust
fn extract_archive(archive: &Path, extract_dir: &Path) -> io::Result<PathBuf> {
    fs::create_dir_all(extract_dir)?;
    let f = File::open(archive)?;
    let dec = MultiGzDecoder::new(BufReader::new(f));
    let mut tar = tar::Archive::new(dec);
    tar.unpack(extract_dir)?;
    let mut roots: Vec<PathBuf> = fs::read_dir(extract_dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .collect();
    if roots.len() != 1 {
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("expected 1 archive root entry, found {}", roots.len()),
        ));
    }
    Ok(roots.remove(0))
}
```

- [ ] **Step 3: Replace the `do_rs` stub with full implementation**

Replace:
```rust
fn do_rs(_args: &RsArgs) -> io::Result<()> {
    Err(io::Error::new(io::ErrorKind::Other, "rs not yet implemented"))
}
```

With:
```rust
fn do_rs(args: &RsArgs) -> io::Result<()> {
    let a = alphabet();
    let src = Path::new(&args.target);
    let dst = Path::new(&args.dest);

    if !src.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("target does not exist: {}", args.target),
        ));
    }

    if let Some(p) = dst.parent() {
        if !p.as_os_str().is_empty() {
            fs::create_dir_all(p)?;
        }
    }

    let tmp = tmp_dir_for(dst);
    if tmp.exists() {
        fs::remove_dir_all(&tmp)?;
    }
    fs::create_dir_all(&tmp)?;

    let src_name = src.file_name().unwrap().to_str().unwrap();
    let shifted_base = src_name.strip_suffix(".tyz").unwrap_or(src_name);
    let original_name = unshift_name(shifted_base, args.shift, &a);

    let pb = spinner("[1/4] Preparing...");
    let tmp_archive = tmp.join(&original_name);
    fs::copy(src, &tmp_archive)?;
    pb.finish_with_message("[1/4] Prepared    ✓");

    let pb = spinner("[2/4] Descrambling...");
    scramble_inplace(&tmp_archive, args.key)?;
    pb.finish_with_message("[2/4] Descrambled ✓");

    let pb = spinner("[3/4] Decompressing...");
    let extract_dir = tmp.join("extract");
    let extracted = extract_archive(&tmp_archive, &extract_dir)?;
    pb.finish_with_message("[3/4] Decompressed ✓");

    let pb = spinner("[4/4] Finalizing...");
    if dst.exists() {
        if dst.is_dir() {
            fs::remove_dir_all(dst)?;
        } else {
            fs::remove_file(dst)?;
        }
    }
    fs::rename(extracted, dst)?;
    fs::remove_dir_all(&tmp)?;
    pb.finish_with_message("[4/4] Done        ✓");

    Ok(())
}
```

- [ ] **Step 4: Build and run file round-trip test**

```bash
cd sunnyside
cargo build 2>&1 | tail -2
echo "ROUNDTRIP_FILE" > /tmp/rt_src.txt
./target/debug/sunnyside bu -t /tmp/rt_src.txt -s 4 -k u -d /tmp/rt_backup.tyz
./target/debug/sunnyside rs -t /tmp/rt_backup.tyz -s 4 -k u -d /tmp/rt_restored.txt
cat /tmp/rt_restored.txt
```
Expected: `ROUNDTRIP_FILE`

Also verify backup still exists and source unchanged:
```bash
ls /tmp/rt_backup.tyz /tmp/rt_src.txt
```
Expected: both files still present.

- [ ] **Step 5: Run directory round-trip test**

```bash
mkdir -p /tmp/rt_dir/nested
echo "TOP" > /tmp/rt_dir/top.txt
echo "DEEP" > /tmp/rt_dir/nested/deep.txt
./target/debug/sunnyside bu -t /tmp/rt_dir -s 4 -k u -d /tmp/rt_dir.tyz
./target/debug/sunnyside rs -t /tmp/rt_dir.tyz -s 4 -k u -d /tmp/rt_dir_out
cat /tmp/rt_dir_out/top.txt
cat /tmp/rt_dir_out/nested/deep.txt
```
Expected: `TOP` then `DEEP`

Verify no temp dirs remain:
```bash
ls /tmp/ | grep sunnyside_tmp
```
Expected: empty output.

- [ ] **Step 6: Commit**

```bash
git add src/main.rs Cargo.lock
git commit -m "feat: implement rs subcommand with multi-member gzip decompression"
```

- [ ] **Step 7: Push sunnyside to GitHub**

```bash
git push origin HEAD
```

Note the new commit SHA — needed for Task 4.

---

### Task 4: anixpkgs integration — package update and regression tests

**Goal:** Update the anixpkgs sunnyside package to point at the new sunnyside version, update the cargo hash, and add bu/rs regression tests to `test_sunnyside.sh`. Requires Tasks 1–3 pushed to GitHub.

**Files:**
- Modify: `anixpkgs/pkgs/rust-packages/sunnyside/default.nix`
- Modify: `anixpkgs/test/test_sunnyside.sh`

**Acceptance Criteria:**
- [ ] `nix flake update sunnyside` completes in anixpkgs
- [ ] anixpkgs sunnyside package builds without hash mismatch errors
- [ ] `test_sunnyside.sh` bu/rs sections pass (file and directory round-trips)

**Verify:** `bash anixpkgs/test/test_sunnyside.sh` (in a nix shell with sunnyside in scope) exits 0 with no red output

**Steps:**

- [ ] **Step 1: Update flake.lock to fetch the new sunnyside commit**

```bash
cd anixpkgs
nix flake update sunnyside
```
Expected: `• Updated input 'sunnyside': github:goromal/sunnyside/OLD → github:goromal/sunnyside/NEW`

- [ ] **Step 2: Set placeholder cargoHash and bump version**

Edit `anixpkgs/pkgs/rust-packages/sunnyside/default.nix`:

```nix
{
  lib,
  rustPlatform,
  pkg-src,
}:
rustPlatform.buildRustPackage rec {
  pname = "sunnyside";
  version = "0.2.0";
  src = pkg-src;
  cargoHash = "";
  meta = {
    description = "File scrambler.";
    longDescription = ''
      Written in Rust. [Repository](https://github.com/goromal/sunnyside)
    '';
    autoGenUsageCmd = "--help";
  };
}
```

(Set `cargoHash = ""` — the build will fail with the expected hash in the error.)

- [ ] **Step 3: Get the correct cargoHash**

```bash
cd anixpkgs
nix build .#sunnyside 2>&1 | grep "got:"
```
Expected output contains a line like:
```
         got:    sha256-XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX=
```

Copy the `sha256-...` value from that line.

- [ ] **Step 4: Set the correct cargoHash**

Update `anixpkgs/pkgs/rust-packages/sunnyside/default.nix` replacing `cargoHash = ""` with the hash from Step 3:

```nix
cargoHash = "sha256-XXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXXX=";
```

- [ ] **Step 5: Verify the package builds**

```bash
cd anixpkgs
nix build .#sunnyside
./result/bin/sunnyside --version
./result/bin/sunnyside bu --help
```
Expected: version shows `0.2.0`, bu help shows `--target`, `--shift`, `--key`, `--dest`.

- [ ] **Step 6: Add bu/rs regression tests to test_sunnyside.sh**

In `anixpkgs/test/test_sunnyside.sh`, append after the existing `sread` test section (before the `# Cleanup` line):

```bash
make-title -c yellow "Testing sunnyside bu and rs (file)"
echo "BACKUP_FILE_TEST" > bu_src.txt
sunnyside bu -t bu_src.txt -s 4 -k u -d bu_out.tyz
[[ -f bu_out.tyz ]] || { echo_red "bu: dest file not created"; exit 1; }
[[ -f bu_src.txt ]] || { echo_red "bu: source file was deleted"; exit 1; }
sunnyside rs -t bu_out.tyz -s 4 -k u -d bu_restored.txt
[[ -f bu_restored.txt ]] || { echo_red "rs: restored file not created"; exit 1; }
if [[ "$(cat bu_restored.txt)" != "BACKUP_FILE_TEST" ]]; then
    echo_red "rs: restored file content mismatch"
    exit 1
fi

make-title -c yellow "Testing sunnyside bu and rs (directory)"
mkdir -p bu_testdir/nested
echo "ROOT" > bu_testdir/root.txt
echo "DEEP" > bu_testdir/nested/deep.txt
sunnyside bu -t bu_testdir -s 4 -k u -d bu_dir.tyz
[[ -f bu_dir.tyz ]] || { echo_red "bu: dir dest not created"; exit 1; }
[[ -d bu_testdir ]] || { echo_red "bu: source dir was deleted"; exit 1; }
sunnyside rs -t bu_dir.tyz -s 4 -k u -d bu_dir_out
[[ -d bu_dir_out ]] || { echo_red "rs: restored dir not created"; exit 1; }
if [[ "$(cat bu_dir_out/root.txt)" != "ROOT" ]]; then
    echo_red "rs: dir root file content mismatch"
    exit 1
fi
if [[ "$(cat bu_dir_out/nested/deep.txt)" != "DEEP" ]]; then
    echo_red "rs: nested file content mismatch"
    exit 1
fi
```

The insertion point is just before `# Cleanup` near the bottom of the file.

- [ ] **Step 7: Run the full test suite in nix shell**

```bash
cd anixpkgs
nix-shell test/shell.nix --run "cd test && bash test_sunnyside.sh"
```
Expected: all sections pass, final output shows no red lines, exit code 0.

- [ ] **Step 8: Commit anixpkgs changes**

```bash
cd anixpkgs
git add flake.lock pkgs/rust-packages/sunnyside/default.nix test/test_sunnyside.sh
git commit -m "feat: update sunnyside to 0.2.0 with bu/rs subcommands"
```
