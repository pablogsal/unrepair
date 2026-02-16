use anyhow::{bail, Context, Result};
use lief::elf::Binary;
use std::path::Path;

pub fn replace_needed(
    elf_path: &Path,
    output_path: &Path,
    old_lib: &str,
    new_lib: &str,
) -> Result<()> {
    if old_lib.is_empty() || new_lib.is_empty() {
        bail!("library names must be non-empty");
    }

    let mut elf =
        Binary::parse(elf_path).with_context(|| format!("parsing ELF {}", elf_path.display()))?;

    let mut needed = elf
        .get_library(old_lib)
        .with_context(|| format!("DT_NEEDED entry '{}' not found", old_lib))?;
    needed.set_name(new_lib);

    // Also patch the VERNEED entry so the dynamic linker can match version
    // requirements to the new library name. Without this, ld.so fails with
    // "Assertion `needed != NULL' failed" because it cannot find the library
    // referenced by the stale VERNEED entry.
    if let Some(mut verneed) = elf.find_version_requirement(old_lib) {
        verneed.set_name(new_lib);
    }

    elf.write(output_path);

    std::fs::metadata(output_path)
        .with_context(|| format!("writing patched binary to {}", output_path.display()))?;

    Ok(())
}
