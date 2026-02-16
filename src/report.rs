use crate::{AbiCheckResult, Severity};
use anyhow::Result;
use std::fmt;
use std::io::Write;
use std::str::FromStr;

#[derive(Debug, Clone, Copy)]
pub enum OutputFormat {
    Text,
    Json,
}

impl FromStr for OutputFormat {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "text" => Ok(OutputFormat::Text),
            "json" => Ok(OutputFormat::Json),
            _ => Err(format!("unknown format: {}", s)),
        }
    }
}

impl fmt::Display for OutputFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputFormat::Text => write!(f, "text"),
            OutputFormat::Json => write!(f, "json"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum ColorMode {
    Auto,
    Always,
    Never,
}

fn use_color(mode: ColorMode) -> bool {
    match mode {
        ColorMode::Always => true,
        ColorMode::Never => false,
        ColorMode::Auto => std::io::IsTerminal::is_terminal(&std::io::stderr()),
    }
}

struct StylePalette {
    error: &'static str,
    warn: &'static str,
    info: &'static str,
    symbol: &'static str,
    layer: &'static str,
    compatible: &'static str,
    incompatible: &'static str,
    reset: &'static str,
}

const COLORED: StylePalette = StylePalette {
    error: "\x1b[1;31m",        // bold red
    warn: "\x1b[1;33m",         // bold yellow
    info: "\x1b[1;34m",         // bold blue
    symbol: "\x1b[36m",         // cyan
    layer: "\x1b[2m",           // dim
    compatible: "\x1b[1;32m",   // bold green
    incompatible: "\x1b[1;31m", // bold red
    reset: "\x1b[0m",
};

const PLAIN: StylePalette = StylePalette {
    error: "",
    warn: "",
    info: "",
    symbol: "",
    layer: "",
    compatible: "",
    incompatible: "",
    reset: "",
};

pub fn print_text(result: &AbiCheckResult, verbose: bool, color_mode: ColorMode) {
    let s = if use_color(color_mode) {
        &COLORED
    } else {
        &PLAIN
    };
    let mut stderr = std::io::stderr().lock();

    let mut errors = 0usize;
    let mut warnings = 0usize;

    for diag in &result.diagnostics {
        match diag.severity {
            Severity::Error => errors += 1,
            Severity::Warning => warnings += 1,
            Severity::Info => {}
        }

        let show = match diag.severity {
            Severity::Error | Severity::Warning => true,
            Severity::Info => verbose,
        };
        if show {
            let (prefix, style) = match diag.severity {
                Severity::Error => ("ERROR", s.error),
                Severity::Warning => ("WARN ", s.warn),
                Severity::Info => ("INFO ", s.info),
            };
            let sym = diag
                .symbol
                .as_deref()
                .map(|name| format!(" {}[{}]{}", s.symbol, name, s.reset))
                .unwrap_or_default();
            let _ = writeln!(
                stderr,
                "{style}{prefix}{reset} {dim}({layer:?}){reset}{sym}: {msg}",
                style = style,
                prefix = prefix,
                reset = s.reset,
                dim = s.layer,
                layer = diag.layer,
                sym = sym,
                msg = diag.message,
            );
        }
    }

    let _ = writeln!(stderr);

    if errors > 0 || warnings > 0 {
        let _ = writeln!(stderr, "{} error(s), {} warning(s)", errors, warnings,);
    }

    let (verdict_str, verdict_style) = match result.verdict {
        crate::Verdict::Compatible => ("COMPATIBLE", s.compatible),
        crate::Verdict::Incompatible => ("INCOMPATIBLE", s.incompatible),
    };
    let _ = writeln!(
        stderr,
        "Verdict: {verdict_style}{verdict}{reset}",
        verdict_style = verdict_style,
        verdict = verdict_str,
        reset = s.reset,
    );
}

pub fn print_json(result: &AbiCheckResult) -> Result<()> {
    let json = serde_json::to_string_pretty(result)?;
    println!("{}", json);
    Ok(())
}
