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
    /// Also convert the extra images in multi-image HEICs (bursts, collections)
    /// to NAME-2.jpg, NAME-3.jpg, ... (default: only the main image -> NAME.jpg).
    #[arg(long)]
    all_images: bool,
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

/// A source file, the root it was found under (for mirroring with --out),
/// and the JPEG path for its main image.
struct Job {
    src: PathBuf,
    rel: PathBuf,
    first: PathBuf,
}

/// Sets each job's output path. When two sources would write the same JPEG
/// (e.g. IMG_1.heic and IMG_1.heif in one folder), the later one is named
/// IMG_1-heif.jpg instead. Jobs are sorted first, so re-runs choose the same names.
fn assign_outputs(jobs: &mut [Job], out: Option<&Path>) {
    jobs.sort_by(|a, b| a.src.cmp(&b.src));
    let mut taken = std::collections::HashSet::new();
    for job in jobs.iter_mut() {
        let mut first = output_path(job, out);
        if !taken.insert(first.to_string_lossy().to_lowercase()) {
            let stem = job.src.file_stem().unwrap_or_default().to_string_lossy();
            let ext = job.src.extension().unwrap_or_default().to_string_lossy().to_lowercase();
            first = first.with_file_name(format!("{stem}-{ext}.jpg"));
            taken.insert(first.to_string_lossy().to_lowercase());
        }
        job.first = first;
    }
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
                        jobs.push(Job { src, rel, first: PathBuf::new() });
                    }
                }
            }
        } else {
            let rel = PathBuf::from(p.file_name().context("invalid file name")?);
            if seen.insert(canonical(p)) {
                jobs.push(Job { src: p.clone(), rel, first: PathBuf::new() });
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

/// Output name for image `index` of a file: NAME.jpg, then NAME-2.jpg, NAME-3.jpg...
fn indexed_path(first: &Path, index: usize) -> PathBuf {
    if index == 0 {
        return first.to_path_buf();
    }
    let stem = first.file_stem().unwrap_or_default().to_string_lossy();
    first.with_file_name(format!("{stem}-{}.jpg", index + 1))
}

#[derive(Default)]
struct Outcome {
    /// JPEGs written.
    converted: usize,
    /// JPEGs not written because they already existed.
    skipped: usize,
    /// Extra images in the file that were not converted (no --all-images).
    extra_images: usize,
}

fn convert_one(job: &Job, args: &ConvertArgs) -> Result<Outcome> {
    let first = job.first.clone();
    let mut outcome = Outcome::default();
    // Fast path: nothing to do, and no need to read the file.
    if !args.all_images && first.exists() && !args.overwrite {
        outcome.skipped = 1;
        return Ok(outcome);
    }
    let bytes = std::fs::read(&job.src)?;
    let count = heic_core::image_count(&bytes)?;
    let wanted = if args.all_images { count } else { 1 };
    if !args.all_images {
        outcome.extra_images = count.saturating_sub(1);
    }
    if let Some(parent) = first.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let decode_opts = heic_core::DecodeOptions { keep_alpha: false, skip_metadata: args.strip_metadata };
    let jpeg_opts = heic_core::JpegOptions { quality: args.quality, keep_metadata: !args.strip_metadata };

    for index in 0..wanted {
        let dst = indexed_path(&first, index);
        if dst.exists() && !args.overwrite {
            outcome.skipped += 1;
            continue;
        }
        let tmp = dst.with_extension("jpg.part");
        let result = heic_core::decode_bytes_at(&bytes, index, decode_opts)
            .and_then(|img| heic_core::write_jpeg_file(&img, &tmp, jpeg_opts))
            .map_err(anyhow::Error::from)
            .and_then(|_| {
                copy_times(&job.src, &tmp)?;
                if dst.exists() {
                    std::fs::remove_file(&dst)?;
                }
                std::fs::rename(&tmp, &dst)?;
                Ok(())
            });
        if let Err(e) = result {
            let _ = std::fs::remove_file(&tmp);
            return Err(e.context(format!("image {} of {count}", index + 1)));
        }
        outcome.converted += 1;
    }
    Ok(outcome)
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
    let mut jobs = collect(&args)?;
    assign_outputs(&mut jobs, args.out.as_deref());
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
    let multi_files = AtomicUsize::new(0);
    let extra_images = AtomicUsize::new(0);

    jobs.par_iter().for_each(|job| {
        let name = job.src.file_name().unwrap_or_default().to_string_lossy().into_owned();
        pb.set_message(name);
        match convert_one(job, &args) {
            Ok(o) => {
                converted.fetch_add(o.converted, Ordering::Relaxed);
                skipped.fetch_add(o.skipped, Ordering::Relaxed);
                if o.extra_images > 0 {
                    multi_files.fetch_add(1, Ordering::Relaxed);
                    extra_images.fetch_add(o.extra_images, Ordering::Relaxed);
                }
            }
            Err(e) => failures.lock().unwrap().push((job.src.clone(), e)),
        }
        pb.inc(1);
    });
    pb.finish_and_clear();
    let (converted, skipped) = (converted.into_inner(), skipped.into_inner());
    let (multi_files, extra_images) = (multi_files.into_inner(), extra_images.into_inner());

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
    if multi_files > 0 {
        println!(
            "Note: {multi_files} file(s) contain {extra_images} extra image(s) (bursts/collections); \
             only the main image was converted. Use --all-images to convert them all."
        );
    }
    Ok(if failures.is_empty() { 0 } else { 2 })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn indexed_names() {
        let p = Path::new(r"C:\x\IMG_1.jpg");
        assert_eq!(indexed_path(p, 0), PathBuf::from(r"C:\x\IMG_1.jpg"));
        assert_eq!(indexed_path(p, 1), PathBuf::from(r"C:\x\IMG_1-2.jpg"));
        assert_eq!(indexed_path(p, 9), PathBuf::from(r"C:\x\IMG_1-10.jpg"));
    }
}

#[cfg(test)]
mod collision_tests {
    use super::*;

    #[test]
    fn same_stem_different_extension_gets_distinct_names() {
        let mk = |s: &str| Job { src: PathBuf::from(s), rel: PathBuf::from(Path::new(s).file_name().unwrap()), first: PathBuf::new() };
        let mut jobs = vec![mk(r"C:\p\IMG.heif"), mk(r"C:\p\IMG.heic"), mk(r"C:\p\other.HEIC")];
        assign_outputs(&mut jobs, None);
        let names: Vec<_> = jobs.iter().map(|j| j.first.file_name().unwrap().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["IMG.jpg", "IMG-heif.jpg", "other.jpg"]);
    }
}
