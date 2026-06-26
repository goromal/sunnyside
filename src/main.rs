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
