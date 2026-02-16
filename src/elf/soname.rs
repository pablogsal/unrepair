use anyhow::{Context, Result};
use lief::elf::dynamic::Entries;
use lief::elf::Binary;
use std::path::Path;

pub fn extract_soname(path: &Path) -> Result<Option<String>> {
    let binary = Binary::parse(path).with_context(|| format!("parsing ELF {}", path.display()))?;
    Ok(extract_soname_from_binary(&binary))
}

pub fn extract_soname_from_binary(binary: &Binary) -> Option<String> {
    for entry in binary.dynamic_entries() {
        if let Entries::SharedObject(so) = entry {
            return Some(so.name());
        }
    }
    None
}

pub fn check_soname(
    bundled_soname: &Option<String>,
    system_soname: &Option<String>,
) -> Option<String> {
    match (bundled_soname, system_soname) {
        (Some(b), Some(s)) if b != s => Some(format!(
            "SONAME mismatch: bundled has '{}', system has '{}'",
            b, s
        )),
        (Some(b), None) => Some(format!(
            "Bundled library has SONAME '{}' but system library has no SONAME",
            b
        )),
        (None, Some(s)) => Some(format!(
            "Bundled library has no SONAME but system library has SONAME '{}'",
            s
        )),
        _ => None,
    }
}
