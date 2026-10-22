# MouseX installer for Windows.
#   irm https://github.com/actus7/mousex/releases/latest/download/install.ps1 | iex
# Optional environment variables:
#   MOUSEX_VERSION  release tag to install, e.g. v1.0.0 (default: latest)
#   MOUSEX_ROLE     server | client (default: the setup dialog asks)
# Works in Windows PowerShell 5.1. Keep this file ASCII-only: it is run through iex.
# Nothing here calls exit, because under iex that would close the user's shell.

function Invoke-MouseXInstall {
    $ErrorActionPreference = 'Stop'
    $ProgressPreference = 'SilentlyContinue'
    [Net.ServicePointManager]::SecurityProtocol = [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

    $repo = 'https://github.com/actus7/mousex/releases'
    $zipName = 'MouseX-windows-x64.zip'
    $sumsName = 'SHA256SUMS'

    $version = $env:MOUSEX_VERSION
    $role = $env:MOUSEX_ROLE
    if ($version -and $version -notmatch '^v\d+\.\d+\.\d+([-.][0-9A-Za-z.]+)?$') {
        throw "MOUSEX_VERSION '$version' is not a tag such as v1.0.0."
    }
    if ($role -and $role -notin @('server', 'client')) {
        throw "MOUSEX_ROLE must be 'server' or 'client'."
    }

    $base = if ($version) { "$repo/download/$version" } else { "$repo/latest/download" }

    # A brand-new directory every run, so nothing stale or planted is ever trusted.
    $tmp = Join-Path ([System.IO.Path]::GetTempPath()) ('mousex-install-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $tmp | Out-Null
    try {
        Write-Host 'Downloading MouseX...'
        $zipPath = Join-Path $tmp $zipName
        $sumsPath = Join-Path $tmp $sumsName
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$zipName" -OutFile $zipPath
        Invoke-WebRequest -UseBasicParsing -Uri "$base/$sumsName" -OutFile $sumsPath

        Write-Host 'Checking the download...'
        $expected = Get-ExpectedSha256 -SumsPath $sumsPath -FileName $zipName
        $actual = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($actual -ne $expected) {
            throw "SHA-256 mismatch for $zipName (expected $expected, got $actual). Nothing was installed."
        }

        $extract = Join-Path $tmp 'package'
        Expand-Archive -LiteralPath $zipPath -DestinationPath $extract
        $exe = Join-Path $extract 'mousex.exe'
        if (-not (Test-Path -LiteralPath $exe)) { throw 'mousex.exe is missing from the package.' }

        Start-MouseXSetup -Exe $exe -WorkDir $extract -Role $role
        Write-Host 'Done. MouseX is installed. Use the tray icon to see it running.'
    }
    finally {
        Remove-Item -LiteralPath $tmp -Recurse -Force -ErrorAction SilentlyContinue
    }
}

function Get-ExpectedSha256 {
    param([string]$SumsPath, [string]$FileName)
    # Lines look like "<sha256>  <file>"; some tools prefix the name with '*'.
    foreach ($line in Get-Content -LiteralPath $SumsPath) {
        $parts = $line.Trim() -split '\s+', 2
        if ($parts.Count -eq 2 -and $parts[1].TrimStart('*') -eq $FileName) {
            return $parts[0].ToLowerInvariant()
        }
    }
    throw "SHA256SUMS has no entry for $FileName."
}

function Start-MouseXSetup {
    param([string]$Exe, [string]$WorkDir, [string]$Role)
    # The elevated process runs in its own session, so use -Wait and check its exit code.
    $arguments = if ($Role) { @('install', '--role', $Role) } else { @('setup') }
    Write-Host 'Requesting administrator permission (a Windows prompt will appear)...'
    try {
        $process = Start-Process -FilePath $Exe -ArgumentList $arguments -WorkingDirectory $WorkDir `
            -Verb RunAs -Wait -PassThru
    }
    catch {
        throw 'MouseX needs administrator permission to install. The prompt was cancelled or denied.'
    }
    if ($process.ExitCode -ne 0) {
        throw "MouseX setup failed (exit code $($process.ExitCode))."
    }
}

try {
    Invoke-MouseXInstall
}
catch {
    Write-Host ''
    Write-Host ('MouseX installation failed: ' + $_.Exception.Message) -ForegroundColor Red
}
