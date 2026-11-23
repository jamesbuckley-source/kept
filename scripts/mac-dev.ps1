<#
.SYNOPSIS
    Rebuild Kept on the MacinCloud Mac whenever a file here changes.

.DESCRIPTION
    The Mac is the only place a macOS build runs, but the code is edited here.
    This watches the source tree, copies what changed over SSH, rebuilds the debug
    bundle there and reopens it, so the Mac's screen shows the edit within a
    minute or two.

    It uses the debug bundle on purpose: the release profile links with LTO and
    takes minutes, while a debug link takes seconds.

    Run -Once to sync and rebuild a single time and exit.

.EXAMPLE
    pwsh ./scripts/mac-dev.ps1

.EXAMPLE
    pwsh ./scripts/mac-dev.ps1 -Once
#>
[CmdletBinding()]
param(
    [string]$HostName = "NY702.macincloud.com",
    [string]$User = "user956782",
    [string]$Key = "$HOME/.ssh/kept_macincloud_ed25519",
    [string]$RemoteDir = "kept",
    [switch]$Once
)

$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
$target = "$User@$HostName"
$sshArgs = @("-i", $Key, "-o", "BatchMode=yes", "-o", "IdentitiesOnly=yes", "-o", "StrictHostKeyChecking=no")

# Everything the build reads. `.git`, `node_modules`, `dist` and `src-tauri/target`
# are left alone: they are large, and the Mac has its own.
$watched = @("src", "src-tauri\src", "src-tauri\icons", "src-tauri\Cargo.toml", "index.html", "package.json", "package-lock.json", "tsconfig.json", "vite.config.ts")

function Send-Remote {
    param([string]$Script)
    # base64 keeps quotes intact through the shell; the carriage returns a Windows
    # here-string carries would otherwise reach bash as part of each word.
    $encoded = [Convert]::ToBase64String([Text.Encoding]::UTF8.GetBytes(($Script -replace "`r", "")))
    & ssh @sshArgs $target "echo $encoded | base64 -d | bash"
    if ($LASTEXITCODE -ne 0) { throw "the remote command failed ($LASTEXITCODE)" }
}

function Sync-Path {
    param([string]$Relative)
    $local = Join-Path $repo $Relative
    if (-not (Test-Path $local)) { return }
    $remote = "$RemoteDir/" + ($Relative -replace '\\', '/')
    $parts = $remote -split '/'
    # Copy into the parent so a re-sent directory replaces its twin instead of
    # landing inside it as `src/src`.
    $parent = if ($parts.Length -gt 1) { $parts[0..($parts.Length - 2)] -join '/' } else { '.' }
    Send-Remote "mkdir -p ~/$parent"
    # A copy merges, so a file deleted here would live on there and break the
    # build (a component left behind by an older branch did exactly that). The
    # twin is removed first, which makes the remote tree match this one.
    if (Test-Path $local -PathType Container) {
        Send-Remote "rm -rf ~/$remote"
    }
    & scp @sshArgs -r $local "${target}:~/$parent/" | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "copying $Relative failed ($LASTEXITCODE)" }
}

function Rebuild {
    Write-Host "building on the Mac…" -ForegroundColor Cyan
    Send-Remote @"
set -euo pipefail
export PATH="`$HOME/.local/node/bin:`$HOME/.cargo/bin:`$PATH"
cd "`$HOME/$RemoteDir"
npm run tauri build -- --debug --bundles app --config '{"bundle":{"createUpdaterArtifacts":false}}' > ~/dev-build.log 2>&1 || { tail -n 40 ~/dev-build.log; exit 1; }
pkill -f "MacOS/kept" 2>/dev/null || true
sleep 1
open "`$HOME/$RemoteDir/src-tauri/target/debug/bundle/macos/Kept.app"
echo rebuilt
"@
    Write-Host "rebuilt and reopened" -ForegroundColor Green
}

Write-Host "syncing to ${target}:~/$RemoteDir" -ForegroundColor Cyan
foreach ($path in $watched) { Sync-Path $path }
Rebuild

if ($Once) { return }

$script:pending = [System.Collections.Generic.HashSet[string]]::new()
$script:lock = [object]::new()
$script:watchers = foreach ($path in @("src", "src-tauri\src")) {
    $w = [System.IO.FileSystemWatcher]::new((Join-Path $repo $path))
    $w.IncludeSubdirectories = $true
    $w.NotifyFilter = [System.IO.NotifyFilters]::LastWrite -bor [System.IO.NotifyFilters]::FileName
    $handler = {
        param($sender, $eventArgs)
        [System.Threading.Monitor]::Enter($script:lock)
        try { [void]$script:pending.Add($eventArgs.FullPath) } finally { [System.Threading.Monitor]::Exit($script:lock) }
    }
    Register-ObjectEvent -InputObject $w -EventName Changed -Action $handler | Out-Null
    Register-ObjectEvent -InputObject $w -EventName Created -Action $handler | Out-Null
    $w.EnableRaisingEvents = $true
    $w
}

Write-Host "watching $($script:watchers.Count) trees - Ctrl+C to stop" -ForegroundColor Yellow
try {
    while ($true) {
        Start-Sleep -Milliseconds 900
        $files = $null
        [System.Threading.Monitor]::Enter($script:lock)
        try {
            if ($script:pending.Count -gt 0) {
                $files = @($script:pending)
                $script:pending.Clear()
            }
        } finally { [System.Threading.Monitor]::Exit($script:lock) }
        if (-not $files) { continue }
        foreach ($file in $files) {
            $relative = [System.IO.Path]::GetRelativePath($repo, $file)
            Write-Host "  $relative" -ForegroundColor DarkGray
            Sync-Path $relative
        }
        Rebuild
    }
} finally {
    foreach ($w in $script:watchers) { $w.EnableRaisingEvents = $false; $w.Dispose() }
    Get-EventSubscriber | Unregister-Event -ErrorAction SilentlyContinue
}
