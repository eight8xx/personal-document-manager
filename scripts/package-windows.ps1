$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$srcTauriRoot = Join-Path $repoRoot "src-tauri"
$releaseDir = Join-Path $repoRoot "release"

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$tauriConfigText = [System.IO.File]::ReadAllText(
    (Join-Path $srcTauriRoot "tauri.conf.json"),
    $utf8NoBom
)
$packageText = [System.IO.File]::ReadAllText(
    (Join-Path $repoRoot "package.json"),
    $utf8NoBom
)
$tauriConfig = $tauriConfigText | ConvertFrom-Json
$package = $packageText | ConvertFrom-Json
$productName = $tauriConfig.productName
$version = $package.version
$installerName = "$productName-Setup-$version.exe"
$portableName = "$productName-Portable.exe"

New-Item -ItemType Directory -Force -Path $releaseDir | Out-Null
Get-ChildItem -LiteralPath $releaseDir -File |
    Where-Object { $_.Name -like "*-Setup-*.exe" -or $_.Name -like "*-Portable.exe" } |
    Remove-Item -Force

Push-Location $repoRoot
try {
    npm run build
    if ($LASTEXITCODE -ne 0) {
        throw "Frontend release build failed with exit code $LASTEXITCODE"
    }

    npx tauri build --bundles nsis
    if ($LASTEXITCODE -ne 0) {
        throw "Tauri NSIS build failed with exit code $LASTEXITCODE"
    }
}
finally {
    Pop-Location
}

$portableCandidates = @(
    Join-Path $srcTauriRoot "target\release\personal-document-manager.exe"
    Join-Path $repoRoot "target\release\personal-document-manager.exe"
)
$portableSource = $portableCandidates |
    Where-Object { Test-Path -LiteralPath $_ } |
    Sort-Object { (Get-Item -LiteralPath $_).LastWriteTime } -Descending |
    Select-Object -First 1

$installerSource = Get-ChildItem -Path @(
    (Join-Path $srcTauriRoot "target\release\bundle\nsis")
    (Join-Path $repoRoot "target\release\bundle\nsis")
) -Filter "*.exe" -File -ErrorAction SilentlyContinue |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if (-not $portableSource) {
    throw "Portable executable was not found after the build."
}
if (-not $installerSource) {
    throw "NSIS installer was not found after the build."
}

$installerTarget = Join-Path $releaseDir $installerName
$portableTarget = Join-Path $releaseDir $portableName

Copy-Item -LiteralPath $installerSource.FullName -Destination $installerTarget -Force
Copy-Item -LiteralPath $portableSource -Destination $portableTarget -Force

Write-Host "Created:"
Write-Host "  $installerTarget"
Write-Host "  $portableTarget"
