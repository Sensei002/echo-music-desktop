#!/usr/bin/env bash
# Compile a crate's doc-comment examples without `cargo test --doc`.
#
# WHY THIS EXISTS
# `cargo test --doc` spawns one `rustc` process per doctest from the test
# harness. On this Windows machine that reliably dies with:
#
#   Failed to spawn "...\rustc.exe": Os { code: 231,
#       kind: Uncategorized, message: "All pipe instances are busy." }
#
# 231 is ERROR_PIPE_BUSY -- the harness exhausts its per-process pipe/handle
# quota long before it runs out of memory or disk. It is an environment
# limitation, not a defect in the documented example. This script exercises the
# exact same code through a *single* rustc invocation instead.
#
# HOW
#   1. Extract every ```rust / ```no_run / ```ignore fenced block out of a
#      crate's `src/**/*.rs` doc comments.
#   2. Wrap them in a `fn main()`.
#   3. Compile with the crate's own dependency graph passed as `--extern`.
#
# Usage:  source tools/msvc-env.sh && bash tools/doc-check.sh crates/<crate>
set -euo pipefail

crate_dir="${1:?usage: doc-check.sh <crate-dir>}"
crate="$(basename "$crate_dir")"
# TMPDIR here is `C:\Users\...\Temp` (backslashes), so `mktemp -d` returns a
# mixed `C:\...\Temp/tmp.XXX` path. rustc's @file expansion chokes on that
# mixture, so normalise to the forward-slash form straight away.
work="$(cygpath -m "$(mktemp -d)")"
trap 'rm -rf "$work"' EXIT

repo="$(cygpath -m "$PWD")"

# ---- 1. extract + wrap the doctest bodies -----------------------------------
python - "$crate_dir" "$work/main.rs" <<'PY'
import pathlib, re, sys

crate_dir, dest = sys.argv[1], sys.argv[2]
src = pathlib.Path(crate_dir) / "src"

# A doc-comment fence: `///```rust` (or //! ), body, then the closing fence.
FENCE = re.compile(
    r"^[ \t]*(?://[/!])[ \t]*```(?:rust|no_run|ignore)[ \t]*$"
    r"(?P<body>.*?)"
    r"^[ \t]*(?://[/!])[ \t]*```[ \t]*$",
    re.S | re.M,
)

blocks, n = [], 0
for f in sorted(src.rglob("*.rs")):
    text = f.read_text(encoding="utf-8", errors="replace")
    for m in FENCE.finditer(text):
        body = "\n".join(
            re.sub(r"^[ \t]*//[/!][ \t]?", "", ln)
            for ln in m.group("body").splitlines()
        ).strip()
        if body and "fn main" not in body:
            n += 1
            blocks.append(f"// ---- {f.name} :: block {n} ----\n{body}\n")

if not blocks:
    pathlib.Path(dest).write_text("fn main() {}\n", encoding="utf-8")
    print(f"no doctest blocks found in {src}")
    sys.exit(0)

pathlib.Path(dest).write_text(
    "fn main() {\n" + "\n".join(blocks) + "}\n", encoding="utf-8"
)
print(f"extracted {n} doctest block(s) from {src}")
PY

# ---- 2. build the extern list -----------------------------------------------
# Done in Python: shelling out to `ls -t` once per crate name is ~350
# subprocesses, which this environment occasionally kills mid-loop.
python - "$repo" "$work/args.rsp" "$work" <<'PY'
import pathlib, sys

repo, dest, work = sys.argv[1], sys.argv[2], sys.argv[3]
deps = pathlib.Path(repo) / "target" / "debug" / "deps"

# `deps/` keeps one rlib per crate *per rebuild*, so stale hashes from earlier
# compiles are still present. Passing two rlibs for the same crate makes rustc
# fail with E0464 ("multiple candidates for rlib dependency"), so keep only the
# most recently written file per crate name.
newest: dict[str, pathlib.Path] = {}
for f in deps.glob("lib*.rlib"):
    stem = f.name[len("lib"):-len(".rlib")]
    cname = stem.rsplit("-", 1)[0]          # libfoo-<hash>.rlib -> foo
    if "-" not in stem or not cname:
        continue
    prev = newest.get(cname)
    if prev is None or f.stat().st_mtime > prev.stat().st_mtime:
        newest[cname] = f

lines = [
    "--edition=2021",
    "--crate-type=bin",
    f"{work}/main.rs",
    "-o",
    f"{work}/doc-check.exe",
    "-L",
    f"{repo}/target/debug/deps",
]
for cname in sorted(newest):
    lines.append(f"--extern={cname}={newest[cname].as_posix()}")

# The `windows` / `windows-sys` crates do not ship their import libraries inside
# their own package: they re-export them from an arch-specific helper crate
# (`windows_x86_64_msvc-<ver>`), and cargo adds that directory via `-L`. Calling
# rustc directly skips that, so the linker dies with
#   LNK1181: cannot open input file 'windows.0.48.5.lib'
# Add every such directory we can find in the registry.
cargo_home = pathlib.Path.home() / ".cargo" / "registry" / "src"
for lib_dir in sorted(cargo_home.glob("*/windows_x86_64_msvc-*/lib")):
    lines += ["-L", lib_dir.as_posix()]

pathlib.Path(dest).write_text("\n".join(lines) + "\n", encoding="utf-8")
print(f"resolved {len(newest)} extern crate(s)")
PY

# ---- 3. compile --------------------------------------------------------------
# Backslash form for the @file argument: the CRT expanding it on Windows is
# picky about a forward-slash-rooted path.
if ! rustc "@$(cygpath -w "$work/args.rsp")"; then
  echo "doc-check: FAILED" >&2
  exit 1
fi

echo "doc-check: OK ($crate)"
