use anyhow::Result;
use clap::builder::styling::{AnsiColor, Effects, Styles};
use clap::{ColorChoice, Parser, Subcommand, ValueEnum, ValueHint};
use std::path::{Path, PathBuf};
use std::process;

use unrepair::{check_compatibility, report, Verdict};

mod wheel;

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
    about = "Undo auditwheel vendoring for extension modules and wheels",
    styles = STYLES,
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    Check(CheckArgs),
    Wheel(WheelWorkflowArgs),
}

#[derive(Parser, Debug)]
#[command(
    about = "Check and optionally patch one extension module against one system library",
    long_about = "Check ABI compatibility between one extension module, one bundled shared \
                  library, and one system shared library. Optionally patch DT_NEEDED when \
                  the verdict is compatible."
)]
struct CheckArgs {
    #[arg(
        long,
        value_name = "FILE",
        value_hint = ValueHint::FilePath,
        help = "Path to the extension module (.so) that imports symbols",
        display_order = 1,
    )]
    extension: PathBuf,

    #[arg(
        long,
        value_name = "FILE",
        value_hint = ValueHint::FilePath,
        help = "Path to the bundled shared library",
        display_order = 2,
    )]
    bundled: PathBuf,

    #[arg(
        long,
        value_name = "FILE",
        value_hint = ValueHint::FilePath,
        help = "Path to the system shared library to check against",
        display_order = 3,
    )]
    system: PathBuf,

    #[arg(
        long,
        help = "Patch the extension's DT_NEEDED entry to use the system library",
        display_order = 4
    )]
    patch: bool,

    #[arg(
        long,
        value_name = "SOURCE",
        default_value = "soname",
        requires = "patch",
        help = "How to derive the replacement DT_NEEDED value for --patch",
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

    #[arg(long, short, help = "Enable verbose output", display_order = 7)]
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
        display_order = 9
    )]
    color: ColorChoice,
}

#[derive(Parser, Debug)]
#[command(
    about = "Full wheel workflow: discover, check, unrepair, and repackage",
    long_about = "Process a wheel end-to-end. unrepair discovers bundled vendored shared \
                  libraries, matches them against provided system libraries, checks ABI \
                  compatibility, patches extension DT_NEEDED entries, removes unneeded \
                  bundled libs, and writes a new wheel."
)]
struct WheelWorkflowArgs {
    #[arg(
        long,
        value_name = "FILE",
        value_hint = ValueHint::FilePath,
        help = "Input wheel file (.whl)"
    )]
    wheel: PathBuf,

    #[arg(
        long,
        value_name = "FILE",
        value_hint = ValueHint::FilePath,
        help = "Output wheel path (default: <input>.unrepaired.whl)"
    )]
    output_wheel: Option<PathBuf>,

    #[arg(
        long = "system-lib",
        value_name = "FILE",
        value_hint = ValueHint::FilePath,
        help = "System library candidate file (repeatable)"
    )]
    system_lib: Vec<PathBuf>,

    #[arg(
        long = "system-lib-dir",
        value_name = "DIR",
        value_hint = ValueHint::DirPath,
        help = "Directory to recursively scan for system libraries (repeatable)"
    )]
    system_lib_dir: Vec<PathBuf>,

    #[arg(
        long,
        value_name = "DIR",
        value_hint = ValueHint::DirPath,
        help = "Directory where temporary unpacked wheel data should be created"
    )]
    workdir: Option<PathBuf>,

    #[arg(
        long = "no-strict",
        action = clap::ArgAction::SetFalse,
        default_value_t = true,
        help = "Best-effort mode: return zero even if some requested unrepair actions fail"
    )]
    strict: bool,

    #[arg(long, short, help = "Enable verbose output")]
    verbose: bool,

    #[arg(long, default_value = "text", help = "Output format")]
    format: report::OutputFormat,

    #[arg(
        long,
        value_name = "WHEN",
        default_value = "auto",
        help = "Control colored output"
    )]
    color: ColorChoice,
}

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();

    match cli.command {
        Commands::Check(args) => run_check(args),
        Commands::Wheel(args) => run_wheel(args),
    }
}

fn run_check(args: CheckArgs) -> Result<()> {
    let color_choice = to_color_mode(args.color);
    let result = check_compatibility(&args.extension, &args.bundled, &args.system)?;

    match args.format {
        report::OutputFormat::Text => report::print_text(&result, args.verbose, color_choice),
        report::OutputFormat::Json => report::print_json(&result)?,
    }

    if args.patch && result.verdict == Verdict::Compatible {
        let bundled_soname = unrepair::elf::soname::extract_soname(&args.bundled)?;
        let old_lib = bundled_soname.unwrap_or_default();
        if old_lib.is_empty() {
            eprintln!("Error: Cannot patch - missing SONAME in bundled library");
            process::exit(1);
        }

        let new_lib = match args.patch_needed_from {
            PatchNeededFrom::Soname => {
                let system_soname = unrepair::elf::soname::extract_soname(&args.system)?;
                let soname = system_soname.unwrap_or_default();
                if soname.is_empty() {
                    eprintln!(
                        "Error: Cannot patch with --patch-needed-from=soname - missing SONAME in system library"
                    );
                    process::exit(1);
                }
                soname
            }
            PatchNeededFrom::SystemPath => args.system.to_string_lossy().to_string(),
        };

        let output_path = args.output.as_ref().unwrap_or(&args.extension);
        unrepair::patch::replace_needed(&args.extension, output_path, &old_lib, &new_lib)?;
        eprintln!("Patched DT_NEEDED: {} -> {}", old_lib, new_lib);
    }

    let exit_code = match result.verdict {
        Verdict::Compatible => 0,
        Verdict::Incompatible => 1,
    };
    process::exit(exit_code);
}

fn run_wheel(args: WheelWorkflowArgs) -> Result<()> {
    let color_mode = to_color_mode(args.color);
    let output_wheel = args
        .output_wheel
        .unwrap_or_else(|| default_output_wheel(&args.wheel));

    let result = wheel::run(wheel::WheelArgs {
        wheel: &args.wheel,
        output_wheel: &output_wheel,
        system_libs: &args.system_lib,
        system_lib_dirs: &args.system_lib_dir,
        strict: args.strict,
        color_mode,
        verbose: args.verbose,
        workdir: args.workdir.as_deref(),
    })?;

    match args.format {
        report::OutputFormat::Text => wheel::print_text(&result, color_mode),
        report::OutputFormat::Json => wheel::print_json(&result)?,
    }
    process::exit(wheel::exit_code(&result));
}

fn to_color_mode(choice: ColorChoice) -> report::ColorMode {
    match choice {
        ColorChoice::Auto => report::ColorMode::Auto,
        ColorChoice::Always => report::ColorMode::Always,
        ColorChoice::Never => report::ColorMode::Never,
    }
}

fn default_output_wheel(input: &Path) -> PathBuf {
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("output");
    parent.join(format!("{stem}.unrepaired.whl"))
}
