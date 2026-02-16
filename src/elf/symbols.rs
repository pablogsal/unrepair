use lief::elf::Binary;
use lief::generic::Symbol;
use std::collections::{HashMap, HashSet};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolType {
    Func,
    Object,
    Other,
}

#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub address: u64,
    pub size: u64,
    pub symbol_type: SymbolType,
}

pub fn extract_imports(binary: &Binary) -> HashSet<String> {
    binary
        .imported_symbols()
        .map(|sym| sym.name())
        .filter(|name| !name.is_empty())
        .collect()
}

pub fn extract_exports(binary: &Binary) -> HashSet<String> {
    binary
        .exported_symbols()
        .map(|sym| sym.name())
        .filter(|name| !name.is_empty())
        .collect()
}

pub fn extract_exports_with_info(binary: &Binary) -> HashMap<String, SymbolInfo> {
    let mut exports = HashMap::new();
    for sym in binary.exported_symbols() {
        let name = sym.name();
        if name.is_empty() {
            continue;
        }
        let symbol_type = match sym.get_type() {
            lief::elf::symbol::Type::FUNC => SymbolType::Func,
            lief::elf::symbol::Type::OBJECT => SymbolType::Object,
            _ => SymbolType::Other,
        };
        exports.insert(
            name,
            SymbolInfo {
                address: sym.value(),
                size: sym.size(),
                symbol_type,
            },
        );
    }
    exports
}

pub fn compute_used_symbols(
    extension_imports: &HashSet<String>,
    bundled_exports: &HashSet<String>,
) -> HashSet<String> {
    extension_imports
        .intersection(bundled_exports)
        .cloned()
        .collect()
}
