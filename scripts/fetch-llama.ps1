# Downloads the pinned llama.cpp Vulkan build, checks its sha256 (spec risk B3) and puts llama-server.exe
# and its DLLs in src-tauri/engines/win/llama/. The archive is kept in .cache/llama/ for the CI cache.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$tag = 'b11819'
$sha = 'c3b2484d18258425fe3f69d0b6d07dce01501d6e010578dad3e53150a30df557'
$name = "llama-$tag-bin-win-vulkan-x64.zip"
$root = Split-Path -Parent $PSScriptRoot
$cache = Join-Path $root '.cache/llama'
$zip = Join-Path $cache $name
New-Item -ItemType Directory -Force $cache | Out-Null
if (-not (Test-Path $zip)) {
    Invoke-WebRequest "https://github.com/ggml-org/llama.cpp/releases/download/$tag/$name" -OutFile "$zip.part"
    Move-Item -Force "$zip.part" $zip
}
$got = (Get-FileHash -Algorithm SHA256 $zip).Hash.ToLower()
if ($got -ne $sha) {
    Remove-Item $zip
    throw "sha256 mismatch for ${name}: got $got, expected $sha"
}

$tmp = Join-Path ([IO.Path]::GetTempPath()) ([Guid]::NewGuid())
Expand-Archive $zip $tmp
$server = Get-ChildItem $tmp -Recurse -Filter llama-server.exe | Select-Object -First 1
if (-not $server) { throw "llama-server.exe not found in $name" }
$out = Join-Path $root 'src-tauri/engines/win/llama'
if (Test-Path $out) { Remove-Item -Recurse -Force $out }
New-Item -ItemType Directory -Force $out | Out-Null
Copy-Item $server.FullName $out
# All DLLs, since ggml loads its backends (Vulkan, CPU variants) at run time from this folder,
# except the other tools' llama-*-impl.dll.
Get-ChildItem (Join-Path $server.DirectoryName '*.dll') |
    Where-Object { $_.Name -notlike 'llama-*-impl.dll' -or $_.Name -eq 'llama-server-impl.dll' } |
    Copy-Item -Destination $out
Copy-Item (Join-Path $server.DirectoryName 'LICENSE*') $out
# The DLLs link the MSVC runtime, which a clean Windows may lack: ship it app-local (Microsoft allows this).
foreach ($dll in 'msvcp140.dll', 'vcruntime140.dll', 'vcruntime140_1.dll') {
    Copy-Item (Join-Path $env:SystemRoot "System32\$dll") $out
}
Remove-Item -Recurse -Force $tmp
Write-Output "fetched $out"
