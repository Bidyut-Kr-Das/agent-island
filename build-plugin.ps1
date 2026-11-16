# Single build/update path: stop the island, build, copy exe to plugin/bin, bump the plugin patch version.
# Claude Code detects plugin updates by version, so the bump is what makes `claude plugin update` pick up the new exe.
$ErrorActionPreference = 'Stop'
Set-Location $PSScriptRoot

# Stop only island processes whose exe lives under plugin\bin or the Claude Code plugin cache.
$roots = @((Join-Path $PSScriptRoot 'plugin\bin'), (Join-Path $env:USERPROFILE '.claude\plugins')) |
    ForEach-Object { $_.TrimEnd('\') + '\' }
Get-Process agent-islands -ErrorAction SilentlyContinue | Where-Object {
    $p = $_.Path
    $p -and ($roots | Where-Object { $p.StartsWith($_, [StringComparison]::OrdinalIgnoreCase) })
} | Stop-Process -Force

cargo build --release
if ($LASTEXITCODE -ne 0) { throw 'cargo build failed' }
New-Item -ItemType Directory -Force plugin/bin | Out-Null
Copy-Item target/release/agent-islands.exe plugin/bin/agent-islands.exe -Force

$f = Join-Path $PSScriptRoot 'plugin\.claude-plugin\plugin.json'
$t = [IO.File]::ReadAllText($f)
$m = [regex]::Match($t, '"version":\s*"(\d+)\.(\d+)\.(\d+)"')
if (-not $m.Success) { throw 'version not found in plugin.json' }
$new = '{0}.{1}.{2}' -f $m.Groups[1].Value, $m.Groups[2].Value, ([int]$m.Groups[3].Value + 1)
[IO.File]::WriteAllText($f, $t.Replace($m.Value, '"version": "' + $new + '"'))
Write-Host "Built. Plugin version is now $new. Try it: claude --plugin-dir .\plugin"
