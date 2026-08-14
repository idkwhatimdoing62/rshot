[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateNotNullOrEmpty()]
    [string]$OrtRuntimeDir
)

$ErrorActionPreference = 'Stop'

$projectRoot = (Resolve-Path -LiteralPath (Join-Path $PSScriptRoot '..')).Path
$runtimeRoot = (Resolve-Path -LiteralPath $OrtRuntimeDir).Path
$runtimeFiles = @('onnxruntime.dll', 'onnxruntime_providers_shared.dll')
$targetTriple = 'x86_64-pc-windows-msvc'

foreach ($name in $runtimeFiles) {
    $path = Join-Path $runtimeRoot $name
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "OCR runtime is missing $path"
    }
}

$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
    throw 'Visual Studio vswhere.exe was not found; Release imports cannot be audited.'
}
$visualStudio = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath
if (-not $visualStudio) {
    throw 'Visual Studio Build Tools with the x64 C++ toolchain was not found.'
}
$dumpbin = Get-ChildItem -LiteralPath (Join-Path $visualStudio 'VC\Tools\MSVC') -Directory |
    Sort-Object Name -Descending |
    ForEach-Object { Join-Path $_.FullName 'bin\Hostx64\x64\dumpbin.exe' } |
    Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } |
    Select-Object -First 1
if (-not $dumpbin) {
    throw 'x64 dumpbin.exe was not found; Release imports cannot be audited.'
}

function Get-DumpbinOutput([string]$Binary, [string]$Option) {
    $output = (& $dumpbin $Option $Binary 2>&1 | Out-String)
    if ($LASTEXITCODE -ne 0) {
        throw "dumpbin could not inspect $Binary"
    }
    return $output
}

function Assert-X64Pe([string]$Binary) {
    $headers = Get-DumpbinOutput $Binary '/HEADERS'
    if ($headers -notmatch '(?im)^\s*8664 machine \(x64\)\s*$') {
        throw "$Binary is not an x64 PE image."
    }
}

function Assert-SystemOnlyImports([string]$Binary) {
    $importText = (Get-DumpbinOutput $Binary '/DEPENDENTS') + "`n" + (Get-DumpbinOutput $Binary '/IMPORTS')
    $allowed = @(
        'advapi32.dll', 'bcrypt.dll', 'bcryptprimitives.dll', 'cabinet.dll',
        'cfgmgr32.dll', 'combase.dll', 'crypt32.dll', 'dbghelp.dll', 'dwmapi.dll',
        'dxgi.dll', 'gdi32.dll', 'imm32.dll', 'iphlpapi.dll', 'kernel32.dll',
        'kernelbase.dll', 'ntdll.dll', 'ole32.dll', 'oleaut32.dll', 'powrprof.dll',
        'rpcrt4.dll', 'secur32.dll', 'setupapi.dll', 'shell32.dll', 'shlwapi.dll',
        'user32.dll', 'uxtheme.dll', 'version.dll', 'winmm.dll', 'ws2_32.dll'
    )
    $imports = [regex]::Matches($importText, '(?im)^\s*([A-Z0-9_.-]+\.dll)\s*$') |
        ForEach-Object { $_.Groups[1].Value.ToLowerInvariant() } |
        Sort-Object -Unique
    foreach ($import in $imports) {
        $isApiSet = $import.StartsWith('api-ms-win-') -or $import.StartsWith('ext-ms-win-')
        if (-not $isApiSet -and $allowed -notcontains $import) {
            throw "$Binary imports non-system or unapproved dependency $import"
        }
    }
}

function Assert-OrtRuntime([string]$Binary) {
    Assert-X64Pe $Binary
    Assert-SystemOnlyImports $Binary
    $version = (Get-Item -LiteralPath $Binary).VersionInfo.ProductVersion
    if (-not $version) {
        $version = (Get-Item -LiteralPath $Binary).VersionInfo.FileVersion
    }
    if ($version -notmatch '^1\.28\.0(?:\D|$)') {
        throw "$Binary does not report ONNX Runtime 1.28.0 (reported: $version)."
    }
    $exports = Get-DumpbinOutput $Binary '/EXPORTS'
    if ($exports -notmatch '(?m)\bOrtGetApiBase\b') {
        throw "$Binary does not export OrtGetApiBase."
    }
}

$ortDll = Join-Path $runtimeRoot 'onnxruntime.dll'
$providerDll = Join-Path $runtimeRoot 'onnxruntime_providers_shared.dll'
Assert-OrtRuntime $ortDll
Assert-X64Pe $providerDll
Assert-SystemOnlyImports $providerDll

$manifestLines = foreach ($name in $runtimeFiles) {
    $digest = (Get-FileHash -Algorithm SHA256 -LiteralPath (Join-Path $runtimeRoot $name)).Hash.ToLowerInvariant()
    "$digest  $name"
}
$manifestPath = Join-Path $runtimeRoot 'rshot-ocr-runtime.sha256'
$utf8WithoutBom = New-Object System.Text.UTF8Encoding($false)
[System.IO.File]::WriteAllText($manifestPath, (($manifestLines -join "`n") + "`n"), $utf8WithoutBom)

$previousRuntime = $env:RSHOT_OCR_RUNTIME_DIR
$previousTargetDir = $env:CARGO_TARGET_DIR
$previousBuildTarget = $env:CARGO_BUILD_TARGET
try {
    $env:RSHOT_OCR_RUNTIME_DIR = $runtimeRoot
    $env:CARGO_TARGET_DIR = Join-Path $projectRoot 'target'
    $env:CARGO_BUILD_TARGET = $null
    $hostLine = rustc -vV | Where-Object { $_ -like 'host:*' }
    if ($LASTEXITCODE -ne 0 -or $hostLine -ne 'host: x86_64-pc-windows-msvc') {
        throw "Portable Release requires the x86_64-pc-windows-msvc Rust host (reported: $hostLine)."
    }
    Push-Location $projectRoot
    try {
        cargo build --release --locked --target $targetTriple
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build --release --locked --target $targetTriple failed."
        }
    }
    finally {
        Pop-Location
    }
}
finally {
    $env:RSHOT_OCR_RUNTIME_DIR = $previousRuntime
    $env:CARGO_TARGET_DIR = $previousTargetDir
    $env:CARGO_BUILD_TARGET = $previousBuildTarget
}

$releaseExe = Join-Path $projectRoot "target\$targetTriple\release\rshot.exe"
if (-not (Test-Path -LiteralPath $releaseExe -PathType Leaf)) {
    throw "Cargo did not produce the expected Release artifact: $releaseExe"
}
Assert-X64Pe $releaseExe
Assert-SystemOnlyImports $releaseExe

$smoke = Start-Process -FilePath $releaseExe -ArgumentList '--rshot-ocr-self-test' -PassThru -WindowStyle Hidden
if (-not $smoke.WaitForExit(20000)) {
    Stop-Process -Id $smoke.Id -Force -ErrorAction SilentlyContinue
    $smoke.WaitForExit()
    throw 'OCR model/runtime smoke test exceeded 20 seconds.'
}
if ($smoke.ExitCode -ne 0) {
    throw "OCR model/runtime smoke test failed with exit code $($smoke.ExitCode)."
}

$releaseHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $releaseExe).Hash
Write-Host "Portable Release built: $releaseExe"
Write-Host "SHA-256: $releaseHash"
