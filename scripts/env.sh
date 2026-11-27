# Source before cargo on this machine: `. scripts/env.sh`
# The windows-gnu toolchain needs MSYS2 binutils (dlltool + as) for crates that use raw-dylib.
export PATH="$HOME/.cargo/bin:/c/msys64/mingw64/bin:$PATH"
