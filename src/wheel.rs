use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::collections::{BTreeSet, HashMap, HashSet};
use std::ffi::OsStr;
use std::fs::{self, File};
use std::io::{BufWriter, Read, Write};
use std::path::{Path, PathBuf};
use tempfile::TempDir;
use unrepair::elf::soname;
use unrepair::report;
use unrepair::{check_compatibility, Verdict};
use walkdir::WalkDir;
use zip::read::ZipArchive;
use zip::write::FileOptions;
use zip::CompressionMethod;

#[derive(Debug, Serialize)]
pub struct WheelSummary {
    pub matched_pairs: usize,
    pub checked_extensions: usize,
    pub patched_extensions: usize,
    pub removed_bundled_libs: usize,
    pub skipped_checks: usize,
}

#[derive(Debug, Serialize)]
pub struct PairResult {
    pub bundled_path: String,
    pub bundled_soname: String,
    pub system_path: String,
    pub system_soname: String,
    pub checked_extensions: usize,
    pub patched_extensions: usize,
    pub skipped_extensions: usize,
    pub incompatible_extensions: usize,
}

#[derive(Debug, Serialize)]
pub struct WheelWorkflowResult {
    pub input_wheel: String,
    pub output_wheel: String,
    pub strict: bool,
    pub hard_failure: bool,
    pub failures: Vec<String>,
    pub warnings: Vec<String>,
    pub pairs: Vec<PairResult>,
    pub removed_bundled_paths: Vec<String>,
    pub summary: WheelSummary,
}

struct SystemCandidate {
    path: PathBuf,
    soname: String,
    stem: String,
}

#[derive(Clone)]
struct BundledLib {
    rel_path: PathBuf,
    abs_path: PathBuf,
    soname: String,
}

struct MappingExecution {
    pairs: Vec<PairResult>,
    warnings: Vec<String>,
    failures: Vec<String>,
    checked_extensions: usize,
    patched_extensions: usize,
    skipped_checks: usize,
    patched_bundled_sonames: HashSet<String>,
}

pub struct WheelArgs<'a> {
    pub wheel: &'a Path,
    pub output_wheel: &'a Path,
    pub system_libs: &'a [PathBuf],
    pub system_lib_dirs: &'a [PathBuf],
    pub strict: bool,
    pub color_mode: report::ColorMode,
    pub verbose: bool,
    pub workdir: Option<&'a Path>,
}

pub fn run(args: WheelArgs<'_>) -> Result<WheelWorkflowResult> {
    let tmp = create_workdir(args.workdir)?;
    let root = tmp.path().join("wheel-root");
    fs::create_dir_all(&root)?;

    stage("Discovering wheel contents", args.color_mode);
    unpack_wheel(args.wheel, &root)?;

    let record_rel = find_record_rel_path(&root)
        .ok_or_else(|| anyhow!("wheel is missing .dist-info/RECORD, cannot repackage safely"))?;
    let bundled = discover_bundled_libs(&root)?;
    let extensions = discover_extension_modules(&root)?;

    if args.verbose {
        eprintln!(
            "Found {} extension module(s) and {} bundled library file(s)",
            extensions.len(),
            bundled.len()
        );
    }

    stage("Matching vendored libs to system libs", args.color_mode);
    let systems = discover_system_candidates(args.system_libs, args.system_lib_dirs)?;
    if systems.is_empty() {
        bail!("no usable system libraries found from --system-lib/--system-lib-dir");
    }

    let mappings = build_mappings(&bundled, &systems)?;
    if mappings.is_empty() {
        bail!("no bundled libraries matched provided system libraries");
    }

    stage("Validating ABI and patching extensions", args.color_mode);
    let mut ext_needed = build_extension_needed_cache(&extensions)?;
    let exec = execute_mappings(mappings, &extensions, &mut ext_needed)?;

    stage("Removing unneeded bundled libs", args.color_mode);
    let removed =
        remove_safely_unneeded_bundled(&root, &ext_needed, &exec.patched_bundled_sonames)?;

    stage("Repacking wheel", args.color_mode);
    regenerate_record(&root, &record_rel)?;
    repackage_wheel(&root, args.output_wheel)?;

    let hard_failure = false;
    let matched_pairs = exec.pairs.len();

    Ok(WheelWorkflowResult {
        input_wheel: args.wheel.display().to_string(),
        output_wheel: args.output_wheel.display().to_string(),
        strict: args.strict,
        hard_failure,
        failures: exec.failures,
        warnings: exec.warnings,
        pairs: exec.pairs,
        removed_bundled_paths: removed.clone(),
        summary: WheelSummary {
            matched_pairs,
            checked_extensions: exec.checked_extensions,
            patched_extensions: exec.patched_extensions,
            removed_bundled_libs: removed.len(),
            skipped_checks: exec.skipped_checks,
        },
    })
}

fn execute_mappings(
    mappings: Vec<(&BundledLib, &SystemCandidate)>,
    extensions: &[PathBuf],
    ext_needed: &mut [HashSet<String>],
) -> Result<MappingExecution> {
    let mut pairs = Vec::new();
    let mut warnings = Vec::new();
    let mut failures = Vec::new();
    let mut checked_extensions = 0usize;
    let mut patched_extensions = 0usize;
    let mut skipped_checks = 0usize;
    let mut patched_bundled_sonames = HashSet::new();

    for (bundled_lib, system_lib) in mappings {
        let old_needed = bundled_lib.soname.clone();
        let new_needed = system_lib.soname.clone();
        let mut pair = PairResult {
            bundled_path: rel_string(&bundled_lib.rel_path),
            bundled_soname: old_needed.clone(),
            system_path: system_lib.path.display().to_string(),
            system_soname: new_needed.clone(),
            checked_extensions: 0,
            patched_extensions: 0,
            skipped_extensions: 0,
            incompatible_extensions: 0,
        };

        for (idx, ext) in extensions.iter().enumerate() {
            if !ext_needed[idx].contains(&old_needed) {
                pair.skipped_extensions += 1;
                continue;
            }

            pair.checked_extensions += 1;
            checked_extensions += 1;

            let check_result = check_compatibility(ext, &bundled_lib.abs_path, &system_lib.path)
                .with_context(|| format!("compatibility check failed for {}", ext.display()))?;

            if check_result.verdict == Verdict::Compatible {
                unrepair::patch::replace_needed(ext, ext, &old_needed, &new_needed).with_context(
                    || {
                        format!(
                            "failed patching {} ({} -> {})",
                            ext.display(),
                            old_needed,
                            new_needed
                        )
                    },
                )?;
                ext_needed[idx].remove(&old_needed);
                ext_needed[idx].insert(new_needed.clone());

                pair.patched_extensions += 1;
                patched_extensions += 1;
                patched_bundled_sonames.insert(old_needed.clone());
            } else {
                pair.incompatible_extensions += 1;
                skipped_checks += 1;
                failures.push(format!(
                    "{} incompatible with system {}",
                    ext.display(),
                    system_lib.path.display()
                ));
            }
        }

        if pair.checked_extensions == 0 {
            warnings.push(format!(
                "No extension depended on bundled {} ({})",
                pair.bundled_soname, pair.bundled_path
            ));
        }
        pairs.push(pair);
    }

    Ok(MappingExecution {
        pairs,
        warnings,
        failures,
        checked_extensions,
        patched_extensions,
        skipped_checks,
        patched_bundled_sonames,
    })
}

fn build_extension_needed_cache(extensions: &[PathBuf]) -> Result<Vec<HashSet<String>>> {
    extensions.iter().map(|p| read_needed(p)).collect()
}

fn remove_safely_unneeded_bundled(
    root: &Path,
    ext_needed: &[HashSet<String>],
    patched_bundled_sonames: &HashSet<String>,
) -> Result<Vec<String>> {
    let mut removed = Vec::new();
    let mut bundled_by_soname = discover_bundled_libs(root)?
        .into_iter()
        .map(|lib| (lib.soname.clone(), lib))
        .collect::<HashMap<_, _>>();

    loop {
        let current_sonames = bundled_by_soname.keys().cloned().collect::<HashSet<_>>();

        let needed_by_extensions = ext_needed
            .iter()
            .flat_map(|set| set.iter())
            .filter(|name| current_sonames.contains(*name))
            .cloned()
            .collect::<HashSet<_>>();

        let mut needed_by_bundled = HashSet::new();
        for lib in bundled_by_soname.values() {
            for needed in read_needed(&lib.abs_path)? {
                if current_sonames.contains(&needed) {
                    needed_by_bundled.insert(needed);
                }
            }
        }

        let removable = bundled_by_soname
            .iter()
            .filter(|(soname, _)| patched_bundled_sonames.contains(*soname))
            .filter(|(soname, _)| {
                !needed_by_extensions.contains(*soname) && !needed_by_bundled.contains(*soname)
            })
            .map(|(soname, lib)| (soname.clone(), lib.clone()))
            .collect::<Vec<_>>();

        if removable.is_empty() {
            break;
        }

        for (soname, lib) in removable {
            fs::remove_file(&lib.abs_path)?;
            removed.push(rel_string(&lib.rel_path));
            bundled_by_soname.remove(&soname);
        }
    }

    removed.sort();
    Ok(removed)
}

pub fn print_text(result: &WheelWorkflowResult, color_mode: report::ColorMode) {
    let color = use_color(color_mode);
    let (green, red, yellow, reset) = if color {
        ("\x1b[1;32m", "\x1b[1;31m", "\x1b[1;33m", "\x1b[0m")
    } else {
        ("", "", "", "")
    };

    for warning in &result.warnings {
        eprintln!("{yellow}WARN{reset}: {warning}");
    }
    for failure in &result.failures {
        eprintln!("{red}FAIL{reset}: {failure}");
    }

    eprintln!();
    eprintln!("Wheel: {}", result.input_wheel);
    eprintln!("Output: {}", result.output_wheel);
    eprintln!("Matched pairs: {}", result.summary.matched_pairs);
    eprintln!("Checked extensions: {}", result.summary.checked_extensions);
    eprintln!("Patched extensions: {}", result.summary.patched_extensions);
    eprintln!(
        "Removed bundled libs: {}",
        result.summary.removed_bundled_libs
    );
    eprintln!(
        "Skipped/incompatible checks: {}",
        result.summary.skipped_checks
    );

    let strict_failure = result.strict && !result.failures.is_empty();
    if result.hard_failure || strict_failure {
        eprintln!("{red}Result: INCOMPLETE{reset}");
    } else {
        eprintln!("{green}Result: COMPLETE{reset}");
    }
}

pub fn print_json(result: &WheelWorkflowResult) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(result)?);
    Ok(())
}

pub fn exit_code(result: &WheelWorkflowResult) -> i32 {
    i32::from(result.hard_failure || (result.strict && !result.failures.is_empty()))
}

fn use_color(mode: report::ColorMode) -> bool {
    match mode {
        report::ColorMode::Always => true,
        report::ColorMode::Never => false,
        report::ColorMode::Auto => std::io::IsTerminal::is_terminal(&std::io::stderr()),
    }
}

fn stage(name: &str, color_mode: report::ColorMode) {
    if use_color(color_mode) {
        eprintln!("\x1b[1;34m==>\x1b[0m {}", name);
    } else {
        eprintln!("==> {}", name);
    }
}

fn create_workdir(base: Option<&Path>) -> Result<TempDir> {
    match base {
        Some(dir) => {
            fs::create_dir_all(dir)?;
            tempfile::Builder::new()
                .prefix("unrepair-wheel-")
                .tempdir_in(dir)
                .context("creating temporary workdir")
        }
        None => tempfile::Builder::new()
            .prefix("unrepair-wheel-")
            .tempdir()
            .context("creating temporary workdir"),
    }
}

fn unpack_wheel(wheel: &Path, out_root: &Path) -> Result<()> {
    let file = File::open(wheel).with_context(|| format!("opening wheel {}", wheel.display()))?;
    let mut archive = ZipArchive::new(file).context("parsing wheel zip")?;
    for i in 0..archive.len() {
        let mut entry = archive.by_index(i)?;
        let outpath = out_root.join(entry.name());
        if entry.is_dir() {
            fs::create_dir_all(&outpath)?;
            continue;
        }
        if let Some(parent) = outpath.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut outfile = File::create(&outpath)?;
        std::io::copy(&mut entry, &mut outfile)?;
    }
    Ok(())
}

fn repackage_wheel(root: &Path, output: &Path) -> Result<()> {
    if let Some(parent) = output.parent() {
        fs::create_dir_all(parent)?;
    }
    let file = File::create(output)?;
    let mut zip = zip::ZipWriter::new(file);
    let options = FileOptions::default().compression_method(CompressionMethod::Deflated);

    for rel in collect_files(root, false)? {
        let src = root.join(&rel);
        zip.start_file(rel_string(&rel), options)?;
        let mut f = File::open(src)?;
        std::io::copy(&mut f, &mut zip)?;
    }
    zip.finish()?;
    Ok(())
}

fn find_record_rel_path(root: &Path) -> Option<PathBuf> {
    WalkDir::new(root)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .find_map(|entry| {
            let rel = entry.path().strip_prefix(root).ok()?;
            let is_record = rel.file_name() == Some(OsStr::new("RECORD"));
            let in_dist_info = rel
                .components()
                .any(|c| c.as_os_str().to_string_lossy().ends_with(".dist-info"));
            if is_record && in_dist_info {
                Some(rel.to_path_buf())
            } else {
                None
            }
        })
}

fn regenerate_record(root: &Path, record_rel: &Path) -> Result<()> {
    let record_abs = root.join(record_rel);
    let mut out = BufWriter::new(File::create(record_abs)?);

    for rel in collect_files(root, false)? {
        let rel_str = rel_string(&rel);
        if rel == record_rel {
            writeln!(out, "{},,", rel_str)?;
            continue;
        }

        let mut buf = Vec::new();
        File::open(root.join(&rel))?.read_to_end(&mut buf)?;
        let digest = Sha256::digest(&buf);
        let hash = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(digest);
        writeln!(out, "{},sha256={},{}", rel_str, hash, buf.len())?;
    }
    out.flush()?;
    Ok(())
}

fn discover_extension_modules(root: &Path) -> Result<Vec<PathBuf>> {
    collect_files(root, false)?
        .into_iter()
        .filter(|rel| is_shared_object_name(rel.file_name()))
        .filter(|rel| !rel_string(rel).contains(".libs/"))
        .map(|rel| Ok(root.join(rel)))
        .collect()
}

fn discover_bundled_libs(root: &Path) -> Result<Vec<BundledLib>> {
    let mut out = Vec::new();
    for rel in collect_files(root, false)? {
        if !is_shared_object_name(rel.file_name()) {
            continue;
        }
        if !rel_string(&rel).contains(".libs/") {
            continue;
        }

        let abs = root.join(&rel);
        let son = soname::extract_soname(&abs)?
            .filter(|s| !s.is_empty())
            .or_else(|| {
                rel.file_name()
                    .and_then(OsStr::to_str)
                    .map(ToOwned::to_owned)
            })
            .unwrap_or_default();

        out.push(BundledLib {
            rel_path: rel,
            abs_path: abs,
            soname: son,
        });
    }
    out.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(out)
}

fn discover_system_candidates(
    system_libs: &[PathBuf],
    system_lib_dirs: &[PathBuf],
) -> Result<Vec<SystemCandidate>> {
    let mut paths = system_libs.iter().cloned().collect::<BTreeSet<_>>();

    for dir in system_lib_dirs {
        for entry in WalkDir::new(dir).follow_links(true) {
            let entry = entry?;
            if !entry.file_type().is_file() {
                continue;
            }
            if is_shared_object_name(entry.path().file_name()) {
                paths.insert(entry.path().to_path_buf());
            }
        }
    }

    let mut out = Vec::new();
    for path in paths {
        let son = soname::extract_soname(&path)
            .with_context(|| format!("reading SONAME from system library {}", path.display()))?;
        let Some(soname_value) = son.filter(|s| !s.is_empty()) else {
            continue;
        };
        let Some(stem) = soname_stem(&soname_value) else {
            continue;
        };

        out.push(SystemCandidate {
            path,
            soname: soname_value,
            stem,
        });
    }
    Ok(out)
}

fn build_mappings<'a>(
    bundled: &'a [BundledLib],
    systems: &'a [SystemCandidate],
) -> Result<Vec<(&'a BundledLib, &'a SystemCandidate)>> {
    let mut assigned_bundled = HashSet::<String>::new();
    let mut out = Vec::new();

    for sys in systems {
        let mut matches = bundled
            .iter()
            .filter(|bun| soname_prefix_match(&bun.soname, &sys.stem))
            .collect::<Vec<_>>();

        if matches.len() > 1 {
            let mut names = matches.iter().map(|m| m.soname.clone()).collect::<Vec<_>>();
            names.sort();
            bail!(
                "ambiguous mapping for system {} (SONAME {}): matched bundled {:?}",
                sys.path.display(),
                sys.soname,
                names
            );
        }

        if let Some(bun) = matches.pop() {
            if assigned_bundled.insert(rel_string(&bun.rel_path)) {
                out.push((bun, sys));
            }
        }
    }
    Ok(out)
}

fn collect_files(root: &Path, follow_links: bool) -> Result<Vec<PathBuf>> {
    let mut files = WalkDir::new(root)
        .follow_links(follow_links)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter_map(|e| e.path().strip_prefix(root).ok().map(Path::to_path_buf))
        .collect::<Vec<_>>();
    files.sort();
    Ok(files)
}

fn soname_stem(soname: &str) -> Option<String> {
    soname.find(".so").map(|idx| soname[..idx].to_string())
}

fn soname_prefix_match(vendored_soname: &str, stem: &str) -> bool {
    if !vendored_soname.starts_with(stem) {
        return false;
    }
    let rest = &vendored_soname[stem.len()..];
    rest.starts_with('-') || rest.starts_with(".so")
}

fn read_needed(path: &Path) -> Result<HashSet<String>> {
    let binary = lief::elf::Binary::parse(path)
        .with_context(|| format!("parsing ELF {}", path.display()))?;
    Ok(binary
        .dynamic_entries()
        .filter_map(|entry| {
            if let lief::elf::dynamic::Entries::Library(lib) = entry {
                Some(lib.name())
            } else {
                None
            }
        })
        .collect())
}

fn is_shared_object_name(name: Option<&OsStr>) -> bool {
    name.and_then(OsStr::to_str)
        .map(|n| n.ends_with(".so") || n.contains(".so."))
        .unwrap_or(false)
}

fn rel_string(path: &Path) -> String {
    path.to_string_lossy().replace('\\', "/")
}
