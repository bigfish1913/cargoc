#requires -Version 5.1
# Installs only cargoc.exe. Never edits Cargo, PATH, configuration or shims.
[CmdletBinding()]
param([string]$Version = '', [string]$Dir = '', [switch]$DryRun)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'
$PreviousTls = [Net.ServicePointManager]::SecurityProtocol
$TempDir = $null
$Stage = $null
try {
    [Net.ServicePointManager]::SecurityProtocol = $PreviousTls -bor [Net.SecurityProtocolType]::Tls12
    $Arch = $env:PROCESSOR_ARCHITECTURE
    if ($env:PROCESSOR_ARCHITEW6432) { $Arch = $env:PROCESSOR_ARCHITEW6432 }
    switch ($Arch) {
        'AMD64' { $Target = 'x86_64-pc-windows-msvc' }
        'ARM64' { $Target = 'aarch64-pc-windows-msvc' }
        default { throw "Unsupported Windows architecture: $Arch" }
    }
    if (!$Dir) {
        if (!$env:LOCALAPPDATA) { throw 'LOCALAPPDATA is missing; pass -Dir.' }
        $Dir = Join-Path $env:LOCALAPPDATA 'Programs\cargoc'
    }
    $Dir = $ExecutionContext.SessionState.Path.GetUnresolvedProviderPathFromPSPath($Dir)
    $Headers = @{ 'User-Agent' = 'cargoc-installer'; Accept = 'application/vnd.github+json' }
    if (!$Version) {
        $Release = Invoke-RestMethod -Uri 'https://api.github.com/repos/bigfish1913/cargoc/releases/latest' -Headers $Headers -TimeoutSec 30
        if ($Release.draft -or $Release.prerelease) { throw 'No stable release was returned.' }
        $Version = $Release.tag_name
    }
    $Version = $Version -replace '^v', ''
    if ($Version -cne $Version.Trim() -or $Version -cnotmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$') { throw 'Invalid version; pass -Version X.Y.Z.' }
    $Root = "cargoc-v$Version-$Target"
    $Asset = "$Root.zip"
    $Url = "https://github.com/bigfish1913/cargoc/releases/download/v$Version/$Asset"
    Write-Host "platform: $Target`nversion: $Version`nurl: $Url`ndir: $Dir"
    if ($DryRun) { return }
    $Destination = Join-Path $Dir 'cargoc.exe'
    if (Test-Path -LiteralPath $Destination) { throw 'cargoc already exists here; use cargoc self-update or your package manager.' }
    # Refuse junctions/symlinks rather than writing through another installation.
    $Parent = $Dir
    while ($Parent) {
        if (Test-Path -LiteralPath $Parent) {
            if ((Get-Item -LiteralPath $Parent -Force).Attributes -band [IO.FileAttributes]::ReparsePoint) { throw "Refusing reparse point: $Parent" }
        }
        $Parent = [IO.Path]::GetDirectoryName($Parent)
    }
    $TempDir = Join-Path ([IO.Path]::GetTempPath()) ('cargoc-install-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $TempDir | Out-Null
    $Archive = Join-Path $TempDir $Asset
    $Checksum = Join-Path $TempDir 'checksum.sha256'
    Invoke-WebRequest -UseBasicParsing -Uri "$Url.sha256" -OutFile $Checksum -TimeoutSec 60
    $Text = (Get-Content -LiteralPath $Checksum -Raw).Trim()
    if ($Text -cnotmatch '^([0-9a-fA-F]{64})\s+\*?([^\s]+)$' -or $Matches[2] -cne $Asset) { throw 'Invalid checksum or wrong archive name.' }
    $Expected = $Matches[1]
    Invoke-WebRequest -UseBasicParsing -Uri $Url -OutFile $Archive -TimeoutSec 120
    if ((Get-FileHash -LiteralPath $Archive -Algorithm SHA256).Hash -ne $Expected) { throw 'SHA-256 verification failed.' }
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $Zip = [IO.Compression.ZipFile]::OpenRead($Archive)
    $Binary = Join-Path $TempDir 'cargoc.exe'
    try {
        $Members = @($Zip.Entries | Where-Object { $_.FullName.Replace('\', '/') -ceq "$Root/cargoc.exe" })
        if ($Members.Count -ne 1) { throw 'Missing or duplicate expected binary.' }
        $Member = $Members[0]
        $Mode = ($Member.ExternalAttributes -shr 16) -band 0xF000
        if (($Mode -ne 0 -and $Mode -ne 0x8000) -or $Member.Length -le 0 -or $Member.Length -gt 64MB) { throw 'Invalid binary entry.' }
        $InputStream = $Member.Open()
        $OutputStream = [IO.File]::Open($Binary, [IO.FileMode]::CreateNew, [IO.FileAccess]::Write, [IO.FileShare]::None)
        try { $InputStream.CopyTo($OutputStream) } finally { $OutputStream.Dispose(); $InputStream.Dispose() }
    } finally { $Zip.Dispose() }
    $ActualVersion = & $Binary --version
    if ($LASTEXITCODE -ne 0 -or $ActualVersion -cne "cargoc $Version") { throw 'Binary version mismatch.' }
    New-Item -ItemType Directory -Path $Dir -Force | Out-Null
    $Stage = Join-Path $Dir ('.cargoc-install-' + [guid]::NewGuid().ToString('N') + '.exe')
    [IO.File]::Copy($Binary, $Stage, $false)
    # The two-argument Move refuses a concurrently created destination.
    [IO.File]::Move($Stage, $Destination)
    $Stage = $null
    Write-Host "Installed cargoc $Version to $Destination"
    if (($env:PATH -split ';') -notcontains $Dir) {
        $Quoted = $Dir.Replace("'", "''")
        Write-Host "Add this directory to your user PATH. For this session:"
        Write-Host "  `$env:PATH = '$Quoted;' + `$env:PATH"
    }
    Write-Host 'Optional: cargoc setup --enable (requires Cargo 1.91+)'
} finally {
    if ($Stage -and (Test-Path -LiteralPath $Stage)) { Remove-Item -LiteralPath $Stage -Force }
    if ($TempDir -and (Test-Path -LiteralPath $TempDir)) { Remove-Item -LiteralPath $TempDir -Recurse -Force }
    [Net.ServicePointManager]::SecurityProtocol = $PreviousTls
}
