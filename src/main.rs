use clap::Parser;
use rayon::prelude::*;
use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Write};
use std::path::Path;
use std::process;

/// Make some scrambled eggs.
#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// File target.
    #[arg(short, long)]
    target: String,
    /// Shift amount.
    #[arg(short, long)]
    shift: usize,
    /// Scramble key.
    #[arg(short, long)]
    key: char,
}

fn main() -> io::Result<()> {
    let args = Args::parse();

    let a: Vec<char> = ('a'..='z')
        .chain('A'..='Z')
        .chain(std::iter::once('.'))
        .chain('0'..='9')
        .collect();
    let ext: &str = ".tyz";

    if !Path::new(&args.target).exists() {
        eprintln!("Specified source does not exist: {}", &args.target);
        process::exit(1);
    }

    if !&args.target.chars().all(|c| a.contains(&c)) {
        eprintln!("Letters, numbers, and dots only, please.");
        process::exit(1);
    }

    let mut cvt: bool = true;
    if (&args.target).contains(&ext) {
        println!("...and back again.");
        cvt = false;
    } else {
        println!("There...")
    }

    let mut srcp: String = String::new();
    let (a_left, a_right) = a.split_at(args.shift);
    let a_s: Vec<_> = a_right.iter().chain(a_left.iter()).cloned().collect();
    let mut from_chars: Vec<char> = Vec::new();
    let mut to_chars: Vec<char> = Vec::new();

    if !cvt {
        srcp = args.target.replace(ext, "");
        from_chars = a_s.clone();
        to_chars = a.clone();
    } else {
        srcp = args.target.to_string();
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

    println!("{} -> {}", args.target, tf);

    let mut inf = File::open(args.target)?;
    let mut outf = File::create(tf)?;
    let lve: u8 = args.key as u8;

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
