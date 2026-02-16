pub mod compare;
pub mod elf;
pub mod patch;
pub mod report;

use anyhow::Result;
use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Verdict {
    Compatible,
    Incompatible,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub enum Severity {
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Layer {
    Elf,
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    pub layer: Layer,
    pub symbol: Option<String>,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct AbiCheckResult {
    pub verdict: Verdict,
    pub diagnostics: Vec<Diagnostic>,
}

pub fn check_compatibility(
    extension: &Path,
    bundled: &Path,
    system: &Path,
) -> Result<AbiCheckResult> {
    let mut diagnostics = Vec::new();

    let (used_symbols, elf_diags) =
        compare::symbols::check_elf_compatibility(extension, bundled, system)?;
    diagnostics.extend(elf_diags);

    if diagnostics.iter().any(|d| d.severity == Severity::Error) {
        return Ok(AbiCheckResult {
            verdict: Verdict::Incompatible,
            diagnostics,
        });
    }

    let _ = used_symbols;
    Ok(AbiCheckResult {
        verdict: Verdict::Compatible,
        diagnostics,
    })
}
