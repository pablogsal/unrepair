use lief::elf::Binary;
use lief::generic::Symbol;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct VersionRequirement {
    pub library: String,
    pub version: String,
}

pub fn extract_symbol_version_requirements(
    binary: &Binary,
    used_symbols: &HashSet<String>,
) -> HashMap<String, VersionRequirement> {
    let mut ver_to_lib: HashMap<String, String> = HashMap::new();
    for req in binary.symbols_version_requirement() {
        let lib = req.name();
        for aux in req.auxiliary_symbols() {
            ver_to_lib.insert(aux.name(), lib.clone());
        }
    }

    let mut reqs = HashMap::new();
    for sym in binary.imported_symbols() {
        let sym_name = sym.name();
        if sym_name.is_empty() || !used_symbols.contains(&sym_name) {
            continue;
        }
        if let Some(sv) = sym.symbol_version() {
            if let Some(sva) = sv.symbol_version_auxiliary() {
                let ver_name = sva.name();
                if let Some(lib_name) = ver_to_lib.get(&ver_name) {
                    reqs.insert(
                        sym_name,
                        VersionRequirement {
                            library: lib_name.clone(),
                            version: ver_name,
                        },
                    );
                }
            }
        }
    }

    reqs
}

pub fn extract_version_definitions(binary: &Binary) -> HashSet<String> {
    let mut defs = HashSet::new();
    for vd in binary.symbols_version_definition() {
        for aux in vd.auxiliary_symbols() {
            let name = aux.name();
            if !name.is_empty() {
                defs.insert(name);
            }
        }
    }
    defs
}

pub fn extract_defined_symbol_versions(
    binary: &Binary,
    symbols: &HashSet<String>,
) -> HashMap<String, String> {
    let mut out = HashMap::new();
    for sym in binary.exported_symbols() {
        let sym_name = sym.name();
        if sym_name.is_empty() || !symbols.contains(&sym_name) {
            continue;
        }
        if let Some(sv) = sym.symbol_version() {
            if let Some(sva) = sv.symbol_version_auxiliary() {
                out.insert(sym_name, sva.name());
            }
        }
    }
    out
}

fn parse_glibc_version(version: &str) -> Option<(u32, u32)> {
    let rest = version.strip_prefix("GLIBC_")?;
    let mut parts = rest.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    Some((major, minor))
}

pub fn check_version_compatibility(
    reqs: &HashSet<VersionRequirement>,
    system_defs: &HashSet<String>,
) -> Vec<String> {
    let mut errors = Vec::new();

    let max_system_glibc: Option<(u32, u32)> = system_defs
        .iter()
        .filter_map(|d| parse_glibc_version(d))
        .max();

    for req in reqs {
        if let Some(req_ver) = parse_glibc_version(&req.version) {
            if let Some(max_ver) = max_system_glibc {
                if req_ver <= max_ver {
                    continue;
                }
            }
            errors.push(format!(
                "Required version {} not provided by system library (max GLIBC: {})",
                req.version,
                max_system_glibc
                    .map(|(a, b)| format!("GLIBC_{}.{}", a, b))
                    .unwrap_or_else(|| "none".to_string())
            ));
        } else if !system_defs.contains(&req.version) {
            errors.push(format!(
                "Required version '{}' (from '{}') not defined by system library",
                req.version, req.library
            ));
        }
    }

    errors
}
