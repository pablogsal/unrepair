#![cfg(target_os = "linux")]

use std::collections::HashSet;
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;
use unrepair::patch::replace_needed;
use unrepair::{check_compatibility, Verdict};

fn has_tool(name: &str) -> bool {
    Command::new("sh")
        .arg("-c")
        .arg(format!("command -v {} >/dev/null 2>&1", name))
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn require_build_tools() {
    assert!(
        has_tool("cc"),
        "test requires a C compiler available as `cc`"
    );
}

fn run(cmd: &mut Command) {
    let output = cmd.output().expect("failed to spawn command");
    if !output.status.success() {
        let program = cmd.get_program().to_string_lossy();
        let args: Vec<String> = cmd
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        panic!(
            "command failed: {} {}\nstdout:\n{}\nstderr:\n{}",
            program,
            args.join(" "),
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
}

fn run_output(cmd: &mut Command) -> std::process::Output {
    cmd.output().expect("failed to spawn command")
}

fn unrepair_bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_unrepair"))
}

fn write_file(path: &Path, content: &str) {
    fs::write(path, content).expect("failed to write file");
}

fn compile_shared(c_file: &Path, out_so: &Path, soname: &str, version_script: Option<&Path>) {
    let mut cmd = Command::new("cc");
    cmd.arg("-shared")
        .arg("-fPIC")
        .arg(c_file)
        .arg("-Wl,-soname")
        .arg(format!("-Wl,{}", soname))
        .arg("-o")
        .arg(out_so);

    if let Some(script) = version_script {
        cmd.arg(format!("-Wl,--version-script={}", script.to_string_lossy()));
    }

    run(&mut cmd);
}

fn compile_extension(c_file: &Path, out_so: &Path, link_dir: &Path, link_name: &str) {
    let mut cmd = Command::new("cc");
    cmd.arg("-shared")
        .arg("-fPIC")
        .arg(c_file)
        .arg("-L")
        .arg(link_dir)
        .arg(format!("-l{}", link_name))
        .arg("-Wl,-rpath")
        .arg(format!("-Wl,{}", link_dir.to_string_lossy()))
        .arg("-o")
        .arg(out_so);
    run(&mut cmd);
}

fn parse_needed(path: &Path) -> HashSet<String> {
    let binary = lief::elf::Binary::parse(path).expect("failed to parse ELF");
    binary
        .dynamic_entries()
        .filter_map(|entry| {
            if let lief::elf::dynamic::Entries::Library(lib) = entry {
                Some(lib.name())
            } else {
                None
            }
        })
        .collect()
}

fn parse_verneed_libraries(path: &Path) -> HashSet<String> {
    let binary = lief::elf::Binary::parse(path).expect("failed to parse ELF");
    binary
        .symbols_version_requirement()
        .map(|req| req.name())
        .collect()
}

fn basename_no_lib_prefix(name: &str) -> String {
    let stem = name.strip_suffix(".so").unwrap_or(name);
    stem.strip_prefix("lib").unwrap_or(stem).to_string()
}

fn build_case(
    temp: &TempDir,
    bundled_code: &str,
    system_code: &str,
    bundled_soname: &str,
    system_soname: &str,
    bundled_version_script: Option<&str>,
    system_version_script: Option<&str>,
) -> (PathBuf, PathBuf, PathBuf) {
    let dir = temp.path();

    let ext_c = dir.join("ext.c");
    write_file(
        &ext_c,
        r#"
            extern int add(int a, int b);
            extern int multiply(int a, int b);
            extern const char* get_name(void);

            int extension_func(void) {
                return add(1, 2) + multiply(3, 4) + (int)get_name()[0];
            }
        "#,
    );

    let bundled_c = dir.join("bundled.c");
    let system_c = dir.join("system.c");
    write_file(&bundled_c, bundled_code);
    write_file(&system_c, system_code);

    let bundled_so = dir.join("libbundled.so");
    let system_so = dir.join("libsystem.so");

    let bundled_vs = bundled_version_script.map(|content| {
        let p = dir.join("bundled.map");
        write_file(&p, content);
        p
    });
    let system_vs = system_version_script.map(|content| {
        let p = dir.join("system.map");
        write_file(&p, content);
        p
    });

    compile_shared(
        &bundled_c,
        &bundled_so,
        bundled_soname,
        bundled_vs.as_deref(),
    );
    compile_shared(&system_c, &system_so, system_soname, system_vs.as_deref());

    let ext_so = dir.join("ext.so");
    let link_name = basename_no_lib_prefix(
        bundled_so
            .file_name()
            .and_then(OsStr::to_str)
            .expect("missing bundled file name"),
    );
    compile_extension(&ext_c, &ext_so, dir, &link_name);

    (ext_so, bundled_so, system_so)
}

#[test]
fn compatibility_passes_for_matching_exports() {
    require_build_tools();

    // GIVEN
    let temp = TempDir::new().expect("failed to create tempdir");
    let (ext, bundled, system) = build_case(
        &temp,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "bundled"; }
        "#,
        r#"
            int add(int a, int b) { return a + b + 100; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "system"; }
        "#,
        "libbundled.so",
        "libbundled.so",
        None,
        None,
    );

    // WHEN
    let result = check_compatibility(&ext, &bundled, &system).expect("compatibility failed");

    // THEN
    assert_eq!(result.verdict, Verdict::Compatible);
    assert!(
        result
            .diagnostics
            .iter()
            .all(|d| d.severity != unrepair::Severity::Error),
        "unexpected errors: {:?}",
        result.diagnostics
    );
}

#[test]
fn compatibility_fails_when_system_missing_symbol() {
    require_build_tools();

    // GIVEN
    let temp = TempDir::new().expect("failed to create tempdir");
    let (ext, bundled, system) = build_case(
        &temp,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "bundled"; }
        "#,
        r#"
            int add(int a, int b) { return a + b; }
            const char* get_name(void) { return "system"; }
        "#,
        "libbundled.so",
        "libbundled.so",
        None,
        None,
    );

    // WHEN
    let result = check_compatibility(&ext, &bundled, &system).expect("compatibility failed");

    // THEN
    assert_eq!(result.verdict, Verdict::Incompatible);
    assert!(result.diagnostics.iter().any(|d| {
        d.severity == unrepair::Severity::Error
            && d.symbol.as_deref() == Some("multiply")
            && d.message.contains("not exported")
    }));
}

#[test]
fn compatibility_fails_for_symbol_version_mismatch() {
    require_build_tools();

    // GIVEN
    let temp = TempDir::new().expect("failed to create tempdir");
    let (ext, bundled, system) = build_case(
        &temp,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "bundled"; }
        "#,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "system"; }
        "#,
        "libbundled.so",
        "libbundled.so",
        Some(
            r#"
            LIBBUNDLED_1.0 {
                global: add; multiply; get_name;
                local: *;
            };
            "#,
        ),
        Some(
            r#"
            LIBBUNDLED_2.0 {
                global: add; multiply; get_name;
                local: *;
            };
            "#,
        ),
    );

    // WHEN
    let result = check_compatibility(&ext, &bundled, &system).expect("compatibility failed");

    // THEN
    assert_eq!(result.verdict, Verdict::Incompatible);
    assert!(result.diagnostics.iter().any(|d| {
        d.severity == unrepair::Severity::Error && d.message.contains("not satisfied")
    }));
}

#[test]
fn replace_needed_updates_dt_needed_for_shorter_name() {
    require_build_tools();

    // GIVEN
    let temp = TempDir::new().expect("failed to create tempdir");
    let (ext, _bundled, _system) = build_case(
        &temp,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "bundled"; }
        "#,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "system"; }
        "#,
        "libbundled.so",
        "libsys.so",
        None,
        None,
    );

    // WHEN
    let patched = temp.path().join("ext.patched.so");
    replace_needed(&ext, &patched, "libbundled.so", "libsys.so").expect("patch should succeed");

    // THEN
    let needed = parse_needed(&patched);
    assert!(needed.contains("libsys.so"), "DT_NEEDED: {:?}", needed);
    assert!(
        !needed.contains("libbundled.so"),
        "old DT_NEEDED still present: {:?}",
        needed
    );
}

#[test]
fn replace_needed_accepts_longer_name() {
    require_build_tools();

    // GIVEN
    let temp = TempDir::new().expect("failed to create tempdir");
    let (ext, _bundled, _system) = build_case(
        &temp,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "bundled"; }
        "#,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "system"; }
        "#,
        "liba.so",
        "libthis_name_is_way_too_long_for_in_place_patch.so",
        None,
        None,
    );

    // WHEN
    let patched = temp.path().join("ext.patched.so");
    replace_needed(
        &ext,
        &patched,
        "liba.so",
        "libthis_name_is_way_too_long_for_in_place_patch.so",
    )
    .expect("patch should succeed for longer name with LIEF");

    // THEN
    let needed = parse_needed(&patched);
    assert!(
        needed.contains("libthis_name_is_way_too_long_for_in_place_patch.so"),
        "DT_NEEDED: {:?}",
        needed
    );
    assert!(
        !needed.contains("liba.so"),
        "old DT_NEEDED still present: {:?}",
        needed
    );
}

#[test]
fn compatibility_reports_warning_for_soname_mismatch() {
    require_build_tools();

    // GIVEN
    let temp = TempDir::new().expect("failed to create tempdir");
    let (ext, bundled, system) = build_case(
        &temp,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "bundled"; }
        "#,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "system"; }
        "#,
        "libbundled.so",
        "librenamed-system.so",
        None,
        None,
    );

    // WHEN
    let result = check_compatibility(&ext, &bundled, &system).expect("compatibility failed");

    // THEN
    assert_eq!(result.verdict, Verdict::Compatible);
    assert!(result.diagnostics.iter().any(|d| {
        d.severity == unrepair::Severity::Warning && d.message.contains("SONAME mismatch")
    }));
}

#[test]
fn replace_needed_rejects_empty_library_names() {
    require_build_tools();

    // GIVEN
    let temp = TempDir::new().expect("failed to create tempdir");
    let (ext, _bundled, _system) = build_case(
        &temp,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "bundled"; }
        "#,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "system"; }
        "#,
        "libbundled.so",
        "libsys.so",
        None,
        None,
    );
    let patched = temp.path().join("ext.patched.so");

    // WHEN
    let err = replace_needed(&ext, &patched, "", "libsys.so")
        .expect_err("empty old library name should be rejected");

    // THEN
    assert!(
        format!("{err:#}").contains("non-empty"),
        "unexpected error: {err:#}"
    );

    // WHEN
    let err = replace_needed(&ext, &patched, "libbundled.so", "")
        .expect_err("empty new library name should be rejected");

    // THEN
    assert!(
        format!("{err:#}").contains("non-empty"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn replace_needed_fails_when_old_needed_not_found() {
    require_build_tools();

    // GIVEN
    let temp = TempDir::new().expect("failed to create tempdir");
    let (ext, _bundled, _system) = build_case(
        &temp,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "bundled"; }
        "#,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "system"; }
        "#,
        "libbundled.so",
        "libsys.so",
        None,
        None,
    );

    // WHEN
    let patched = temp.path().join("ext.patched.so");
    let err = replace_needed(&ext, &patched, "libdoesnotexist.so", "libsys.so")
        .expect_err("missing old DT_NEEDED should be rejected");

    // THEN
    assert!(
        format!("{err:#}").contains("DT_NEEDED entry 'libdoesnotexist.so' not found"),
        "unexpected error: {err:#}"
    );
}

#[test]
fn cli_patch_can_use_system_path_for_dt_needed() {
    require_build_tools();

    // GIVEN
    let temp = TempDir::new().expect("failed to create tempdir");
    let (ext, bundled, system) = build_case(
        &temp,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "bundled"; }
        "#,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "system"; }
        "#,
        "libbundled.so",
        "libsystem-soname.so",
        None,
        None,
    );
    let patched = temp.path().join("ext.patched.so");

    // WHEN
    let output = run_output(
        Command::new(unrepair_bin())
            .arg("check")
            .arg("--extension")
            .arg(&ext)
            .arg("--bundled")
            .arg(&bundled)
            .arg("--system")
            .arg(&system)
            .arg("--patch")
            .arg("--patch-needed-from")
            .arg("system-path")
            .arg("--output")
            .arg(&patched),
    );

    // THEN
    assert!(
        output.status.success(),
        "command failed:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let needed = parse_needed(&patched);
    let system_path = system.to_string_lossy().to_string();
    assert!(needed.contains(&system_path), "DT_NEEDED: {:?}", needed);
    assert!(
        !needed.contains("libbundled.so"),
        "old DT_NEEDED still present: {:?}",
        needed
    );
}

#[test]
fn replace_needed_also_patches_verneed_entries() {
    require_build_tools();

    // GIVEN
    let temp = TempDir::new().expect("failed to create tempdir");
    let version_script = r#"
        LIBBUNDLED_1.0 {
            global: add; multiply; get_name;
            local: *;
        };
    "#;
    let (ext, _bundled, _system) = build_case(
        &temp,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "bundled"; }
        "#,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "system"; }
        "#,
        "libbundled.so",
        "libsystem.so",
        Some(version_script),
        None,
    );

    // Verify the extension has a VERNEED entry for the bundled library
    let verneed_before = parse_verneed_libraries(&ext);
    assert!(
        verneed_before.contains("libbundled.so"),
        "precondition: extension should have VERNEED for libbundled.so, got: {:?}",
        verneed_before
    );

    // WHEN
    let patched = temp.path().join("ext.patched.so");
    replace_needed(&ext, &patched, "libbundled.so", "libsystem.so").expect("patch should succeed");

    // THEN — both DT_NEEDED and VERNEED should reference the new name
    let needed = parse_needed(&patched);
    assert!(needed.contains("libsystem.so"), "DT_NEEDED: {:?}", needed);
    assert!(
        !needed.contains("libbundled.so"),
        "old DT_NEEDED still present: {:?}",
        needed
    );

    let verneed_after = parse_verneed_libraries(&patched);
    assert!(
        verneed_after.contains("libsystem.so"),
        "VERNEED should reference libsystem.so, got: {:?}",
        verneed_after
    );
    assert!(
        !verneed_after.contains("libbundled.so"),
        "old VERNEED for libbundled.so still present: {:?}",
        verneed_after
    );
}

#[test]
fn replace_needed_works_without_verneed_entries() {
    require_build_tools();

    // GIVEN
    let temp = TempDir::new().expect("failed to create tempdir");
    let (ext, _bundled, _system) = build_case(
        &temp,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "bundled"; }
        "#,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "system"; }
        "#,
        "libbundled.so",
        "libsystem.so",
        None,
        None,
    );

    // Verify no VERNEED for the bundled library
    let verneed_before = parse_verneed_libraries(&ext);
    assert!(
        !verneed_before.contains("libbundled.so"),
        "precondition: extension should NOT have VERNEED for libbundled.so, got: {:?}",
        verneed_before
    );

    // WHEN
    let patched = temp.path().join("ext.patched.so");
    replace_needed(&ext, &patched, "libbundled.so", "libsystem.so")
        .expect("patch should succeed even without VERNEED entries");

    // THEN — DT_NEEDED is still patched correctly
    let needed = parse_needed(&patched);
    assert!(needed.contains("libsystem.so"), "DT_NEEDED: {:?}", needed);
    assert!(
        !needed.contains("libbundled.so"),
        "old DT_NEEDED still present: {:?}",
        needed
    );
}

#[test]
fn cli_patch_is_skipped_when_validation_fails() {
    require_build_tools();

    // GIVEN
    let temp = TempDir::new().expect("failed to create tempdir");
    let (ext, bundled, system) = build_case(
        &temp,
        r#"
            int add(int a, int b) { return a + b; }
            int multiply(int a, int b) { return a * b; }
            const char* get_name(void) { return "bundled"; }
        "#,
        r#"
            int add(int a, int b) { return a + b; }
            const char* get_name(void) { return "system"; }
        "#,
        "libbundled.so",
        "libbundled.so",
        None,
        None,
    );
    let patched = temp.path().join("ext.should_not_exist.so");

    // WHEN
    let output = run_output(
        Command::new(unrepair_bin())
            .arg("check")
            .arg("--extension")
            .arg(&ext)
            .arg("--bundled")
            .arg(&bundled)
            .arg("--system")
            .arg(&system)
            .arg("--patch")
            .arg("--output")
            .arg(&patched),
    );

    // THEN
    assert!(
        !output.status.success(),
        "command unexpectedly succeeded:\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !patched.exists(),
        "patch output should not be created when verdict is incompatible"
    );
}
