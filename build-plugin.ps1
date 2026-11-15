# Builds the release exe and copies it into the plugin folder (plugin/bin is git-ignored).
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot
cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
New-Item -ItemType Directory -Force plugin/bin | Out-Null
Copy-Item target/release/agent-islands.exe plugin/bin/agent-islands.exe -Force
