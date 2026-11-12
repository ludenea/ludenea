# The independent PNG decoder of the Windows cells: the PNG codec of GDI+,
# reached through System.Drawing, which ships with Windows PowerShell.
#
#   decode  <png> <output>   writes the width and the height as two 32-bit
#                            little-endian integers, then the pixels as GDI+
#                            holds them in memory: blue, green, red, alpha.
#   rewrite <png> <output>   writes the picture again as a PNG file with the
#                            codec's own compression.
param(
    [Parameter(Mandatory = $true)][ValidateSet('decode', 'rewrite')][string]$Mode,
    [Parameter(Mandatory = $true)][string]$Source,
    [Parameter(Mandatory = $true)][string]$Destination
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing

$bitmap = New-Object System.Drawing.Bitmap($Source)
try {
    if ($Mode -eq 'rewrite') {
        $bitmap.Save($Destination, [System.Drawing.Imaging.ImageFormat]::Png)
        exit 0
    }
    $whole = New-Object System.Drawing.Rectangle(0, 0, $bitmap.Width, $bitmap.Height)
    $locked = $bitmap.LockBits($whole, [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    try {
        if ($locked.Stride -ne $bitmap.Width * 4) {
            throw "a row of $($locked.Stride) bytes for $($bitmap.Width) pixels"
        }
        $pixels = New-Object byte[] ($locked.Stride * $bitmap.Height)
        [System.Runtime.InteropServices.Marshal]::Copy($locked.Scan0, $pixels, 0, $pixels.Length)
    }
    finally {
        $bitmap.UnlockBits($locked)
    }
    $stream = [System.IO.File]::Create($Destination)
    try {
        $stream.Write([System.BitConverter]::GetBytes([uint32]$bitmap.Width), 0, 4)
        $stream.Write([System.BitConverter]::GetBytes([uint32]$bitmap.Height), 0, 4)
        $stream.Write($pixels, 0, $pixels.Length)
    }
    finally {
        $stream.Dispose()
    }
}
finally {
    $bitmap.Dispose()
}
