param([switch]$Test, [string]$OutputDirectory = 'dist')
$ErrorActionPreference = 'Stop'
$cargoCommand = (Get-Command cargo -ErrorAction Stop).Source
Push-Location $PSScriptRoot
try {
    if ($Test) {
        & $cargoCommand test --release --locked
    } else {
        & $cargoCommand build --release --locked
    }
    if ($LASTEXITCODE -ne 0) { throw "Cargo failed with exit code $LASTEXITCODE" }
    if (-not $Test) {
        New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null
        Copy-Item -LiteralPath 'target\release\mandrill-sender.exe' -Destination (Join-Path $OutputDirectory 'Mandrill Sender.exe') -Force
    }
} finally {
    Pop-Location
}
