mod install;

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;

#[derive(Parser)]
#[command(name = "heic-convert", version, about = "OpenHEIC: convert HEIC/HEIF images to max-quality JPEG, and view HEIC natively in Windows")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Convert HEIC/HEIF files (or every HEIC in folders, recursively) to JPEG.
    Convert(ConvertArgs),
    /// Install the Windows HEIC viewer codec and Explorer "Convert to JPEG" menu (needs admin).
    Install {
        #[arg(long, hide = true)]
        elevated: bool,
    },
    /// Remove everything `install` added (needs admin).
    Uninstall {
        #[arg(long, hide = true)]
        elevated: bool,
    },
}

#[derive(clap::Args)]
struct ConvertArgs {
    /// Files and/or folders. Folders are scanned recursively.
    #[arg(required = true)]
    paths: Vec<PathBuf>,
    /// Write JPEGs into this folder (mirroring sub-folders) instead of next to the source.
    #[arg(short, long)]
    out: Option<PathBuf>,
    /// Replace JPEGs that already exist.
    #[arg(long)]
    overwrite: bool,
    /// JPEG quality 1-100 (default 100 = visually lossless).
    #[arg(short, long, default_value_t = heic_core::DEFAULT_QUALITY, value_parser = clap::value_parser!(u8).range(1..=100))]
    quality: u8,
    /// Do not copy EXIF / XMP / ICC metadata.
    #[arg(long)]
    strip_metadata: bool,
    /// Number of parallel conversions (default: number of CPU cores).
    #[arg(short, long)]
    jobs: Option<usize>,
    /// Do not scan sub-folders of folder arguments.
    #[arg(long)]
    no_recursive: bool,
    /// Wait for Enter before exiting (used by the Explorer context menu).
    #[arg(long, hide = true)]
    pause: bool,
}

fn main() {
    let cli = Cli::parse();
    let pause = matches!(&cli.cmd, Cmd::Convert(a) if a.pause);
    let code = match run(cli) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    };
    if pause {
        use std::io::Read;
        eprintln!("\nPress Enter to close...");
        let _ = std::io::stdin().read(&mut [0u8]);
    }
    std::process::exit(code);
}

fn run(cli: Cli) -> Result<i32> {
    match cli.cmd {
        Cmd::Convert(a) => convert(a),
        Cmd::Install { elevated } => install::install(elevated).map(|_| 0),
        Cmd::Uninstall { elevated } => install::uninstall(elevated).map(|_| 0),
    }
}

fn is_heic_ext(p: &Path) -> bool {
    p.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ["heic", "heif", "hif"].iter().any(|x| e.eq_ignore_ascii_case(x)))
}

/// A source file and the root it was found under (for mirroring with --out).
struct Job {
    src: PathBuf,
    rel: PathBuf,
}

fn collect(args: &ConvertArgs) -> Result<Vec<Job>> {
    let mut seen = BTreeSet::new();
    let mut jobs = Vec::new();
    for p in &args.paths {
        let meta = std::fs::metadata(p).with_context(|| format!("cannot access {}", p.display()))?;
        if meta.is_dir() {
            let walker = walkdir::WalkDir::new(p)
                .max_depth(if args.no_recursive { 1 } else { usize::MAX })
                .follow_links(false);
            for entry in walker {
                let entry = match entry {
                    Ok(e) => e,
                    Err(e) => {
                        eprintln!("warning: {e}");
                        continue;
                    }
                };
                if entry.file_type().is_file() && is_heic_ext(entry.path()) {
                    let src = entry.path().to_path_buf();
                    let rel = src.strip_prefix(p).unwrap_or(&src).to_path_buf();
                    if seen.insert(canonical(&src)) {
                        jobs.push(Job { src, rel });
                    }
                }
            }
        } else {
            let rel = PathBuf::from(p.file_name().context("invalid file name")?);
            if seen.insert(canonical(p)) {
                jobs.push(Job { src: p.clone(), rel });
            }
        }
    }
    Ok(jobs)
}

fn canonical(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

fn output_path(job: &Job, out: Option<&Path>) -> PathBuf {
    match out {
        Some(dir) => dir.join(&job.rel).with_extension("jpg"),
        None => job.src.with_extension("jpg"),
    }
}

enum Outcome {
    Converted,
    Skipped,
}

fn convert_one(job: &Job, args: &ConvertArgs) -> Result<Outcome> {
    let dst = output_path(job, args.out.as_deref());
    if dst.exists() && !args.overwrite {
        return Ok(Outcome::Skipped);
    }
    if let Some(parent) = dst.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = dst.with_extension("jpg.part");
    let opts = heic_core::JpegOptions { quality: args.quality, keep_metadata: !args.strip_metadata };
    let result = heic_core::convert_file(&job.src, &tmp, opts)
        .map_err(anyhow::Error::from)
        .and_then(|_| {
            copy_times(&job.src, &tmp)?;
            if dst.exists() {
                std::fs::remove_file(&dst)?;
            }
            std::fs::rename(&tmp, &dst)?;
            Ok(())
        });
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map(|_| Outcome::Converted)
}

fn copy_times(src: &Path, dst: &Path) -> Result<()> {
    use std::os::windows::fs::FileTimesExt;
    let m = std::fs::metadata(src)?;
    let mut times = std::fs::FileTimes::new().set_modified(m.modified()?).set_accessed(m.accessed()?);
    if let Ok(c) = m.created() {
        times = times.set_created(c);
    }
    std::fs::OpenOptions::new().write(true).open(dst)?.set_times(times)?;
    Ok(())
}

fn convert(args: ConvertArgs) -> Result<i32> {
    if let Some(n) = args.jobs {
        rayon::ThreadPoolBuilder::new().num_threads(n.max(1)).build_global()?;
    }
    let jobs = collect(&args)?;
    if jobs.is_empty() {
        bail!("no HEIC/HEIF files found");
    }
    println!("Found {} HEIC file(s).", jobs.len());

    let pb = ProgressBar::new(jobs.len() as u64);
    pb.set_style(
        ProgressStyle::with_template("{bar:40.cyan/blue} {pos}/{len} [{elapsed_precise}<{eta}] {msg}")
            .unwrap()
            .progress_chars("=> "),
    );
    let start = Instant::now();
    let failures = Mutex::new(Vec::new());
    let converted = AtomicUsize::new(0);
    let skipped = AtomicUsize::new(0);

    jobs.par_iter().for_each(|job| {
        let name = job.src.file_name().unwrap_or_default().to_string_lossy().into_owned();
        pb.set_message(name);
        match convert_one(job, &args) {
            Ok(Outcome::Converted) => _ = converted.fetch_add(1, Ordering::Relaxed),
            Ok(Outcome::Skipped) => _ = skipped.fetch_add(1, Ordering::Relaxed),
            Err(e) => failures.lock().unwrap().push((job.src.clone(), e)),
        }
        pb.inc(1);
    });
    pb.finish_and_clear();
    let (converted, skipped) = (converted.into_inner(), skipped.into_inner());

    let failures = failures.into_inner().unwrap();
    for (p, e) in &failures {
        eprintln!("FAILED {}: {e:#}", p.display());
    }
    println!(
        "Done in {:.1}s: {converted} converted, {skipped} skipped (already exist{}), {} failed.",
        start.elapsed().as_secs_f64(),
        if args.overwrite { "" } else { "; use --overwrite to replace" },
        failures.len()
    );
    Ok(if failures.is_empty() { 0 } else { 2 })
}
