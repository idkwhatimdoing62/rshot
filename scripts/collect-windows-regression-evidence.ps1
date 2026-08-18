[CmdletBinding()]
param(
    [Parameter(Mandatory)]
    [string]$OutputPath
)

$ErrorActionPreference = 'Stop'

Add-Type -AssemblyName System.Windows.Forms
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

public static class RshotDpiProbe
{
    [DllImport("user32.dll")]
    private static extern uint GetDpiForSystem();

    public static uint SystemDpi()
    {
        return GetDpiForSystem();
    }
}
'@

$screens = @([System.Windows.Forms.Screen]::AllScreens | ForEach-Object {
    [ordered]@{
        device_name = $_.DeviceName
        primary = $_.Primary
        bounds = [ordered]@{
            x = $_.Bounds.X
            y = $_.Bounds.Y
            width = $_.Bounds.Width
            height = $_.Bounds.Height
        }
        working_area = [ordered]@{
            x = $_.WorkingArea.X
            y = $_.WorkingArea.Y
            width = $_.WorkingArea.Width
            height = $_.WorkingArea.Height
        }
    }
})

$evidence = [ordered]@{
    schema = 'rshot_windows_regression_environment_v1'
    collected_at_utc = [DateTime]::UtcNow.ToString('o')
    windows = [ordered]@{
        product_name = (Get-CimInstance Win32_OperatingSystem).Caption
        version = [Environment]::OSVersion.Version.ToString()
        architecture = [Runtime.InteropServices.RuntimeInformation]::OSArchitecture.ToString()
    }
    display = [ordered]@{
        system_dpi = [RshotDpiProbe]::SystemDpi()
        system_scale_percent = [Math]::Round(([RshotDpiProbe]::SystemDpi() / 96.0) * 100)
        screen_count = $screens.Count
        screens = $screens
    }
}

$parent = Split-Path -Parent $OutputPath
if ($parent) {
    New-Item -ItemType Directory -Path $parent -Force | Out-Null
}
$evidence | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $OutputPath -Encoding utf8
Write-Host "Windows regression evidence written to $OutputPath"
