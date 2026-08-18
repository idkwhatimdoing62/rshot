[CmdletBinding()]
param(
    [string]$OutputDirectory = "$(Join-Path $PSScriptRoot '..\fixtures\ocr')"
)

$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Path $OutputDirectory -Force | Out-Null

function New-OcrFixture {
    param(
        [Parameter(Mandatory)][string]$Name,
        [Parameter(Mandatory)][string]$Text,
        [Parameter(Mandatory)][string]$FontFamily
    )

    $bitmap = [System.Drawing.Bitmap]::new(640, 160, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    try {
        $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
        try {
            $graphics.Clear([System.Drawing.Color]::White)
            $graphics.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
            $font = [System.Drawing.Font]::new($FontFamily, 42, [System.Drawing.FontStyle]::Regular, [System.Drawing.GraphicsUnit]::Pixel)
            try {
                $graphics.DrawString($Text, $font, [System.Drawing.Brushes]::Black, 28, 42)
            }
            finally {
                $font.Dispose()
            }
        }
        finally {
            $graphics.Dispose()
        }
        $path = Join-Path $OutputDirectory $Name
        $bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    }
    finally {
        $bitmap.Dispose()
    }
}

$chineseText = -join ([char[]](0x622A, 0x56FE, 0x8BC6, 0x522B, 0x6D4B, 0x8BD5))
$mixedText = 'RShot ' + (-join ([char[]](0x622A, 0x56FE))) + ' OCR 2026'

New-OcrFixture -Name 'english.png' -Text 'RSHOT 2026' -FontFamily 'Segoe UI'
New-OcrFixture -Name 'chinese.png' -Text $chineseText -FontFamily 'Microsoft YaHei UI'
New-OcrFixture -Name 'mixed.png' -Text $mixedText -FontFamily 'Microsoft YaHei UI'
