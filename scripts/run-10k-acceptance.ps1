$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$tauriRoot = Join-Path $repoRoot "src-tauri"
$outputDir = Join-Path $repoRoot "target\acceptance"
$outputFile = Join-Path $outputDir "10k-latest.log"

New-Item -ItemType Directory -Force -Path $outputDir | Out-Null

Push-Location $tauriRoot
try {
    $previousErrorActionPreference = $ErrorActionPreference
    $ErrorActionPreference = "Continue"
    try {
        cargo test --offline --test ten_thousand_acceptance -- --ignored --nocapture --test-threads=1 2>&1 |
            Tee-Object -FilePath $outputFile
        $exitCode = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $previousErrorActionPreference
    }
    if ($exitCode -ne 0) {
        throw "10k acceptance command failed with exit code $exitCode"
    }
}
finally {
    Pop-Location
}

Write-Host "10k acceptance log: $outputFile"
