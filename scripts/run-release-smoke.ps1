[CmdletBinding()]
param(
    [string]$Executable = "$(Join-Path $PSScriptRoot '..\target\release\rshot.exe')"
)

$ErrorActionPreference = 'Stop'
$resolvedExecutable = (Resolve-Path -LiteralPath $Executable).Path
$projectRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path

Push-Location $projectRoot
try {
    cargo test consecutive_capture_sessions_start_and_close_independently
    if ($LASTEXITCODE -ne 0) { throw 'Consecutive screenshot session smoke test failed.' }
    cargo test completing_pixel_capture_restores_pins_before_the_session_continues
    if ($LASTEXITCODE -ne 0) { throw 'Pin coexistence smoke test failed.' }
    cargo test model_failure_falls_back_once
    if ($LASTEXITCODE -ne 0) { throw 'OCR fallback smoke test failed.' }
}
finally {
    Pop-Location
}

function Invoke-Smoke([string]$Argument, [string]$Name) {
    $process = Start-Process -FilePath $resolvedExecutable -ArgumentList $Argument -PassThru -WindowStyle Hidden
    if (-not $process.WaitForExit(30000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        $process.WaitForExit()
        throw "$Name exceeded 30 seconds."
    }
    if ($process.ExitCode -ne 0) {
        throw "$Name failed with exit code $($process.ExitCode)."
    }
}

Invoke-Smoke '--rshot-ocr-self-test' 'OCR artifact smoke test'
Invoke-Smoke '--rshot-clipboard-self-test' 'Clipboard consumer smoke test'

$diagnosticExport = Join-Path ([System.IO.Path]::GetTempPath()) "rshot-diagnostics-$PID.txt"
try {
    $arguments = @('--export-diagnostics', $diagnosticExport)
    $process = Start-Process -FilePath $resolvedExecutable -ArgumentList $arguments -PassThru -WindowStyle Hidden
    if (-not $process.WaitForExit(30000)) {
        Stop-Process -Id $process.Id -Force -ErrorAction SilentlyContinue
        $process.WaitForExit()
        throw 'Diagnostic export smoke test exceeded 30 seconds.'
    }
    if ($process.ExitCode -ne 0 -or -not (Test-Path -LiteralPath $diagnosticExport)) {
        throw "Diagnostic export smoke test failed with exit code $($process.ExitCode)."
    }
    $report = Get-Content -LiteralPath $diagnosticExport -Raw
    if ($report -notmatch '^rshot_diagnostics_v1' -or
        $report -notmatch '(?m)^version=' -or
        $report -notmatch '(?m)^os=windows$' -or
        $report -match '(?i)(ocr_text|window_title|document_path|pixel_data)=') {
        throw 'Diagnostic export smoke test produced an invalid or privacy-unsafe report.'
    }
}
finally {
    Remove-Item -LiteralPath $diagnosticExport -Force -ErrorAction SilentlyContinue
}

Write-Host "Release smoke tests passed: $resolvedExecutable"
