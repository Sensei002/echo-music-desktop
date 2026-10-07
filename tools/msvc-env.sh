# Source this file from Git Bash before running cargo on Windows:
#   source tools/msvc-env.sh
#
# Why this exists:
#   Git for Windows puts its own `usr\bin` (GNU coreutils) on PATH. That
#   directory contains `link.exe`, a *text* utility that gets picked up
#   instead of the MSVC linker, producing:
#      link: extra operand '....rcgu.o'
#   The fix is to drop every Git-for-Windows directory from PATH and point
#   cargo at the real MSVC toolchain.

# --- 1. Drop Git for Windows dirs from PATH ---------------------------------
# Also drop ~/.cargo/bin: on this box it contains 0-byte proxy stubs for
# cargo/cargo-clippy/cargo-fmt/rustfmt. `cargo fmt` resolves the subcommand
# relative to cargo's own directory first, finds the stub, and dies with
#   could not execute process `...cargo-fmt.exe` (os error 193)
#   %1 is not a valid Win32 application.
_msvc_new_path=""
IFS=':' read -r -a _msvc_entries <<< "$PATH"
for _entry in "${_msvc_entries[@]}"; do
  case "$_entry" in
    *"/Program Files/Git/"*|*"/Program Files (x86)/Git/"*|*/mingw64/bin|*/usr/bin|*/usr/local/bin)
      continue
      ;;
    */.cargo/bin|*"/.cargo/bin")
      continue
      ;;
  esac
  _msvc_new_path="${_msvc_new_path:+$_msvc_new_path:}$_entry"
done
export PATH="$_msvc_new_path"

# --- 2. Put the rustup toolchain and MSVC linker first ----------------------
export PATH="$HOME/.rustup/toolchains/stable-x86_64-pc-windows-msvc/bin:$PATH"

_MSVC_VC="C:/Program Files (x86)/Microsoft Visual Studio/2022/BuildTools/VC/Tools/MSVC/14.44.35207"
if [ -d "$_MSVC_VC/bin/Hostx64/x64" ]; then
  export PATH="$_MSVC_VC/bin/Hostx64/x64:$PATH"
  export LIB="$_MSVC_VC/lib/x64;C:/Program Files (x86)/Windows Kits/10/Lib/10.0.26100.0/ucrt/x64;C:/Program Files (x86)/Windows Kits/10/Lib/10.0.26100.0/um/x64"
  export INCLUDE="$_MSVC_VC/include;C:/Program Files (x86)/Windows Kits/10/Include/10.0.26100.0/ucrt;C:/Program Files (x86)/Windows Kits/10/Include/10.0.26100.0/um;C:/Program Files (x86)/Windows Kits/10/Include/10.0.26100.0/shared"
fi

# `cargo clippy` / `cargo fmt` resolve their subcommand binary **next to
# cargo.exe**, not via PATH, so the ~/.cargo/bin stubs above are reached anyway.
# Call the real binaries directly instead:
#   cargo-clippy.exe clippy --workspace --all-targets --locked -- -D warnings
#   cargo-fmt.exe    fmt --all -- --check
unset CARGO_HOME_REMOVED 2>/dev/null || true

echo "[msvc-env] link -> $(which link 2>/dev/null || echo 'not found')"
echo "[msvc-env] rustc -> $(which rustc 2>/dev/null || echo 'not found')"
