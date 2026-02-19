# unrepair
[![CI](https://github.com/pablogsal/unrepair/actions/workflows/ci.yml/badge.svg)](https://github.com/pablogsal/unrepair/actions/workflows/ci.yml)

Because sometimes `auditwheel repair` is a little *too* helpful.

`unrepair` rips out a bundled `.so` that `auditwheel` lovingly stuffed into your wheel and points your extension back at the system library where it belongs. It does its homework first — checking ELF metadata for compatibility — and then optionally patches the extension's `DT_NEEDED` entry so the linker knows what's up.

Why you would use this:
- You'd rather trust the system-provided library (GPU stack, vendor driver, site-managed `.so`) over the copy `auditwheel` dragged in.
- Your extension needs to play nice with other system libraries that all need to come from the same runtime environment.
- You're tired of shipping duplicate `.so` files and want to let the platform do its job.

## What it checks

- ELF identity compatibility between bundled and system library (`e_ident` and machine type).
- The extension's imported symbols that are actually provided by the bundled library.
- Missing symbol exports in the system library for those used symbols.
- Required symbol versions for those used symbols (when version metadata is present and tied to the bundled library).
- SONAME mismatch between bundled and system library (reported as a warning).

## Guarantees and limits

(Mostly limits, if we're being honest.)

- A `COMPATIBLE` verdict is a best-effort static check, not proof of safe execution.
- Functions may exist with compatible names/versions but different behavior.
- ABI aspects not fully represented in these checks (struct layout, calling convention edge cases, side effects, thread-safety, allocator/runtime assumptions, global state interactions) are not covered.
- Loader and environment differences (search paths, transitive dependencies, glibc/libstdc++/driver/runtime interactions) are not covered either.

## Install

```
cargo install --path .
```

## Usage

`unrepair` now has two subcommands:
- `unrepair check` for one extension/bundled/system triple
- `unrepair wheel` for full wheel workflow (discover + check + patch + remove + repackage)

```console
$ unrepair check --extension myext.cpython-313-x86_64-linux-gnu.so \
                --bundled vendor/libfoo.so.3 \
                --system /usr/lib/libfoo.so.3

Verdict: COMPATIBLE
```

When things don't line up, `unrepair` will tell you exactly why:

```console
$ unrepair check --extension myext.so \
                --bundled vendor/libfoo.so.1 \
                --system /usr/lib/libfoo.so.2
ERROR (Elf) [multiply]: Symbol 'multiply' needed by extension but not exported by system library
WARN  (Elf): SONAME mismatch: bundled has 'libfoo.so.1', system has 'libfoo.so.2'

1 error(s), 1 warning(s)
Verdict: INCOMPATIBLE
```

If everything checks out, `--patch` does the actual surgery:

```console
$ unrepair check --extension myext.so \
                --bundled vendor/libfoo.so.3 \
                --system /usr/lib/libfoo.so.3 \
                --patch

Verdict: COMPATIBLE
Patched DT_NEEDED: libfoo.so.3 -> libfoo.so.3
```

If you want GNU loader style full-path `DT_NEEDED`, use:

```console
$ unrepair check --extension myext.so \
                --bundled vendor/libfoo.so.3 \
                --system /opt/vendor/libfoo.so.3 \
                --patch --patch-needed-from system-path
```

Full wheel workflow with system library files and directories:

```console
$ unrepair wheel --wheel dist/mypkg-1.2.3-cp311-cp311-manylinux_2_28_x86_64.whl \
                --system-lib /usr/lib64/libjpeg.so.62 \
                --system-lib-dir /usr/lib64 \
                --output-wheel dist/mypkg-1.2.3.unrepaired.whl
```

## Options

`check`:

```
--extension <FILE>  Path to the extension module (.so)
--bundled <FILE>    Path to the bundled shared library
--system <FILE>     Path to the system shared library
--patch             Patch DT_NEEDED to use the system library
--patch-needed-from <SOURCE>
                    Replacement source for DT_NEEDED: soname (default) or system-path
--output <FILE>     Output path for patched extension (default: in place)
-v, --verbose       Show INFO-level diagnostics
--format <FORMAT>   Output format: text (default) or json
--color <WHEN>      Color output: auto (default), always, or never
```

`wheel`:

```
--wheel <FILE>            Input wheel file (.whl)
--output-wheel <FILE>     Output wheel path (default: <input>.unrepaired.whl)
--system-lib <FILE>       System library candidate file (repeatable)
--system-lib-dir <DIR>    Directory to recursively scan for system libs (repeatable)
--workdir <DIR>           Parent directory for temporary unpacked wheel data
--no-strict               Best-effort mode (return zero even when some checks fail)
-v, --verbose             Show additional workflow details
--format <FORMAT>         Output format: text (default) or json
--color <WHEN>            Color output: auto (default), always, or never
```

## License

See [LICENSE](LICENSE) for details.
