# Dot-source before cargo on this machine: . .\scripts\env.ps1
# The windows-gnu toolchain needs MSYS2 binutils (dlltool + as) for crates that use raw-dylib.
$env:PATH = "$env:USERPROFILE\.cargo\bin;C:\msys64\mingw64\bin;$env:PATH"
