use anyhow::Result;
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{ColorChoice, Parser, ValueEnum, ValueHint};
use std::path::PathBuf;
use std::process;

use unrepair::{check_compatibility, report, Verdict};

const STYLES: Styles = Styles::styled()
    .header(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .usage(AnsiColor::Green.on_default().effects(Effects::BOLD))
    .literal(AnsiColor::Cyan.on_default().effects(Effects::BOLD))
    .placeholder(AnsiColor::Cyan.on_default());

#[derive(Copy, Clone, Debug, Eq, PartialEq, ValueEnum)]
enum PatchNeededFrom {
    Soname,
    SystemPath,
}

#[derive(Parser, Debug)]
#[command(
    name = "unrepair",
    version,
    about = "Undo auditwheel vendoring for one shared library in an extension module",
    long_about = "unrepair is for 'unvendoring' one bundled shared library from an \
                  auditwheel-repaired extension module so it can link against a system \
                  library instead.\n\n\
                  It first performs ABI safety checks using ELF metadata — symbol tables, \
                  version definitions, and SONAMEs — to determine whether the system \
                  library can safely replace the bundled one at runtime. If compatible, \
                  it can patch the extension's DT_NEEDED entry to point to the system \
                  library.",
    after_long_help = "\x1b[1;32mExamples:\x1b[0m\n  \
                       Check compatibility:\n    \
                       $ unrepair --extension myext.cpython-313-x86_64-linux-gnu.so \\\n      \
                              --bundled vendor/libfoo.so.3 \\\n      \
                              --system /usr/lib/libfoo.so.3\n\n  \
                       Check and patch in one step:\n    \
                       $ unrepair --extension myext.so --bundled vendor/libfoo.so.3 \\\n      \
                              --system /usr/lib/libfoo.so.3 --patch\n\n  \
                       JSON output for CI:\n    \
                       $ unrepair --extension myext.so --bundled vendor/libfoo.so.3 \\\n      \
                              --system /usr/lib/libfoo.so.3 --format json\n\n  \
                       Verbose output with colors:\n    \
                       $ unrepair --extension myext.so --bundled vendor/libfoo.so.3 \\\n      \
                              --system /usr/lib/libfoo.so.3 --verbose --color=always",
    styles = STYLES,
)]
struct Cli {
    #[arg(
        long,
        value_name = "FILE",
        value_hint = ValueHint::FilePath,
        help = "Path to the extension module (.so) that imports symbols",
        long_help = "Path to the extension module (.so) that imports symbols. \
                     This is the shared object whose DT_NEEDED entries and \
                     imported symbols are inspected.",
        display_order = 1,
    )]
    extension: PathBuf,

    #[arg(
        long,
        value_name = "FILE",
        value_hint = ValueHint::FilePath,
        help = "Path to the bundled shared library",
        long_help = "Path to the bundled shared library that the extension was \
                     originally linked against. Its exported symbols and version \
                     definitions serve as the baseline for comparison.",
        display_order = 2,
    )]
    bundled: PathBuf,

    #[arg(
        long,
        value_name = "FILE",
        value_hint = ValueHint::FilePath,
        help = "Path to the system shared library to check against",
        long_help = "Path to the system shared library to check against. \
                     unrepair verifies that every symbol the extension needs \
                     is present in this library with a compatible version.",
        display_order = 3,
    )]
    system: PathBuf,

    #[arg(
        long,
        help = "Patch the extension's DT_NEEDED entry to use the system library",
        long_help = "Patch the extension's DT_NEEDED entry to use the system \
                     library. Only takes effect when the verdict is COMPATIBLE. \
                     Rewrites the SONAME reference in-place (or to --output).",
        display_order = 4
    )]
    patch: bool,

    #[arg(
        long,
        value_name = "SOURCE",
        default_value = "soname",
        requires = "patch",
        help = "How to derive the replacement DT_NEEDED value for --patch",
        long_help = "How to derive the replacement DT_NEEDED value for --patch. \
                     'soname' (default) uses the system library SONAME. \
                     'system-path' uses the exact --system path as DT_NEEDED.",
        display_order = 5
    )]
    patch_needed_from: PatchNeededFrom,

    #[arg(
        long,
        value_name = "FILE",
        value_hint = ValueHint::FilePath,
        help = "Output path for the patched extension (defaults to overwriting in place)",
        display_order = 6,
    )]
    output: Option<PathBuf>,

    #[arg(
        long,
        short,
        help = "Enable verbose output",
        long_help = "Enable verbose output. Shows INFO-level diagnostics in \
                     addition to warnings and errors.",
        display_order = 7
    )]
    verbose: bool,

    #[arg(
        long,
        default_value = "text",
        help = "Output format",
        display_order = 8
    )]
    format: report::OutputFormat,

    #[arg(
        long,
        value_name = "WHEN",
        default_value = "auto",
        help = "Control colored output",
        long_help = "Control colored output. 'auto' enables color when stderr \
                     is a terminal, 'always' forces color on, 'never' disables it.",
        display_order = 9
    )]
    color: ColorChoice,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    let color_choice = match cli.color {
        ColorChoice::Auto => report::ColorMode::Auto,
        ColorChoice::Always => report::ColorMode::Always,
        ColorChoice::Never => report::ColorMode::Never,
    };

    let result = check_compatibility(&cli.extension, &cli.bundled, &cli.system)?;

    match cli.format {
        report::OutputFormat::Text => report::print_text(&result, cli.verbose, color_choice),
        report::OutputFormat::Json => report::print_json(&result)?,
    }

    if cli.patch && result.verdict == Verdict::Compatible {
        let bundled_soname = unrepair::elf::soname::extract_soname(&cli.bundled)?;
        let old_lib = bundled_soname.unwrap_or_default();
        if old_lib.is_empty() {
            eprintln!("Error: Cannot patch — missing SONAME in bundled library");
            process::exit(1);
        }

        let new_lib = match cli.patch_needed_from {
            PatchNeededFrom::Soname => {
                let system_soname = unrepair::elf::soname::extract_soname(&cli.system)?;
                let soname = system_soname.unwrap_or_default();
                if soname.is_empty() {
                    eprintln!("Error: Cannot patch with --patch-needed-from=soname — missing SONAME in system library");
                    process::exit(1);
                }
                soname
            }
            PatchNeededFrom::SystemPath => cli.system.to_string_lossy().to_string(),
        };

        let output_path = cli.output.as_ref().unwrap_or(&cli.extension);
        unrepair::patch::replace_needed(&cli.extension, output_path, &old_lib, &new_lib)?;
        eprintln!("Patched DT_NEEDED: {} -> {}", old_lib, new_lib);
    }

    let exit_code = match result.verdict {
        Verdict::Compatible => 0,
        Verdict::Incompatible => 1,
    };

    process::exit(exit_code);
}
