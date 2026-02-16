use crate::elf::{soname, symbols, versioning};
use crate::{Diagnostic, Layer, Severity};
use anyhow::{Context, Result};
use lief::elf::Binary;
use std::collections::HashSet;
use std::path::Path;

pub fn check_elf_compatibility(
    extension: &Path,
    bundled: &Path,
    system: &Path,
) -> Result<(HashSet<String>, Vec<Diagnostic>)> {
    let mut diagnostics = Vec::new();

    let ext_binary = Binary::parse(extension)
        .with_context(|| format!("parsing extension ELF {}", extension.display()))?;
    let bun_binary = Binary::parse(bundled)
        .with_context(|| format!("parsing bundled ELF {}", bundled.display()))?;
    let sys_binary = Binary::parse(system)
        .with_context(|| format!("parsing system ELF {}", system.display()))?;

    let bun_header = bun_binary.header();
    let sys_header = sys_binary.header();
    if bun_header.identity_class() != sys_header.identity_class()
        || bun_header.identity_data() != sys_header.identity_data()
        || bun_header.identity_os_abi() != sys_header.identity_os_abi()
        || bun_header.machine_type() != sys_header.machine_type()
    {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            layer: Layer::Elf,
            symbol: None,
            message: "ELF header mismatch between bundled and system library".to_string(),
        });
    }

    let ext_imports = symbols::extract_imports(&ext_binary);
    let bun_exports = symbols::extract_exports(&bun_binary);
    let sys_exports = symbols::extract_exports(&sys_binary);

    let used_symbols = symbols::compute_used_symbols(&ext_imports, &bun_exports);

    log::info!(
        "Extension imports {} symbols, bundled exports {}, system exports {}, used = {}",
        ext_imports.len(),
        bun_exports.len(),
        sys_exports.len(),
        used_symbols.len()
    );

    let missing: Vec<&String> = used_symbols
        .iter()
        .filter(|s| !sys_exports.contains(*s))
        .collect();

    for sym in &missing {
        diagnostics.push(Diagnostic {
            severity: Severity::Error,
            layer: Layer::Elf,
            symbol: Some(sym.to_string()),
            message: format!(
                "Symbol '{}' needed by extension but not exported by system library",
                sym
            ),
        });
    }

    let bun_exports_info = symbols::extract_exports_with_info(&bun_binary);
    let sys_exports_info = symbols::extract_exports_with_info(&sys_binary);
    for sym in &used_symbols {
        if let (Some(bun_info), Some(sys_info)) =
            (bun_exports_info.get(sym), sys_exports_info.get(sym))
        {
            if bun_info.symbol_type != sys_info.symbol_type {
                diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    layer: Layer::Elf,
                    symbol: Some(sym.clone()),
                    message: format!(
                        "Symbol type mismatch: bundled exports '{}' as {:?} but system exports as {:?}",
                        sym, bun_info.symbol_type, sys_info.symbol_type
                    ),
                });
            }
        }
    }

    let reqs_by_symbol =
        versioning::extract_symbol_version_requirements(&ext_binary, &used_symbols);

    let bun_soname = soname::extract_soname_from_binary(&bun_binary);
    let mut bundled_ids: HashSet<String> = HashSet::new();
    if let Some(ref s) = bun_soname {
        if !s.is_empty() {
            bundled_ids.insert(s.clone());
        }
    }
    if let Some(base) = bundled.file_name().and_then(|s| s.to_str()) {
        if !base.is_empty() {
            bundled_ids.insert(base.to_string());
        }
    }

    let filtered: Vec<(String, versioning::VersionRequirement)> = reqs_by_symbol
        .into_iter()
        .filter(|(_, req)| bundled_ids.contains(&req.library))
        .collect();

    if !filtered.is_empty() {
        let required_syms: HashSet<String> = filtered.iter().map(|(s, _)| s.clone()).collect();
        let sys_versions = versioning::extract_defined_symbol_versions(&sys_binary, &required_syms);

        for (sym, req) in filtered {
            match sys_versions.get(&sym) {
                None => diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    layer: Layer::Elf,
                    symbol: Some(sym),
                    message: format!(
                        "System library does not provide required symbol version '{}' (from '{}')",
                        req.version, req.library
                    ),
                }),
                Some(got) if got != &req.version => diagnostics.push(Diagnostic {
                    severity: Severity::Error,
                    layer: Layer::Elf,
                    symbol: Some(sym),
                    message: format!(
                        "Required symbol version '{}' (from '{}') not satisfied by system (got '{}')",
                        req.version, req.library, got
                    ),
                }),
                Some(_) => {}
            }
        }
    }

    let sys_soname = soname::extract_soname_from_binary(&sys_binary);

    if let Some(msg) = soname::check_soname(&bun_soname, &sys_soname) {
        diagnostics.push(Diagnostic {
            severity: Severity::Warning,
            layer: Layer::Elf,
            symbol: None,
            message: msg,
        });
    }

    Ok((used_symbols, diagnostics))
}
