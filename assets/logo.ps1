param([string]$Out = $PSScriptRoot, [string]$LookAt)
# Paints Bến's logo (the same shapes as logo.svg) as logo.png, 256 pixels a side, and as
# ben.ico, which holds every size Windows asks for and is what build.rs puts into ben.exe.
# Run it again after changing the logo:
#
#   powershell -ExecutionPolicy Bypass -File assets\logo.ps1
#
# Each size is painted four times as large and brought down, so its edges are smooth. The
# smallest sizes leave the three apps out and make the two pieces of glass a little larger,
# so that they still read. With -LookAt <file.png> it also saves a sheet of every size.
Add-Type -AssemblyName System.Drawing

function Rounded([single]$x, [single]$y, [single]$w, [single]$h, [single]$r) {
  $path = New-Object System.Drawing.Drawing2D.GraphicsPath
  $d = [Math]::Min(2 * $r, [Math]::Min($w, $h))
  $path.AddArc($x, $y, $d, $d, 180, 90)
  $path.AddArc($x + $w - $d, $y, $d, $d, 270, 90)
  $path.AddArc($x + $w - $d, $y + $h - $d, $d, $d, 0, 90)
  $path.AddArc($x, $y + $h - $d, $d, $d, 90, 90)
  $path.CloseFigure()
  return $path
}

function Paint([int]$size) {
  $ss = 4
  $big = New-Object System.Drawing.Bitmap ($size * $ss), ($size * $ss), ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $g = [System.Drawing.Graphics]::FromImage($big)
  $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
  $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
  $g.Clear([System.Drawing.Color]::Transparent)
  $k = [single]($size * $ss / 256.0)
  $g.ScaleTransform($k, $k)

  # The screen: water, with a sheen over its upper half.
  $tile = Rounded 0 0 256 256 58
  $water = New-Object System.Drawing.Drawing2D.LinearGradientBrush (New-Object System.Drawing.PointF 0, 0), (New-Object System.Drawing.PointF 0, 256), ([System.Drawing.Color]::FromArgb(255, 0x43, 0xB4, 0xFF)), ([System.Drawing.Color]::FromArgb(255, 0x0B, 0x4F, 0xCF))
  $g.FillPath($water, $tile)
  $g.SetClip($tile)
  $sheen = New-Object System.Drawing.Drawing2D.LinearGradientBrush (New-Object System.Drawing.PointF 0, -1), (New-Object System.Drawing.PointF 0, 128), ([System.Drawing.Color]::FromArgb(66, 255, 255, 255)), ([System.Drawing.Color]::FromArgb(0, 255, 255, 255))
  $g.FillRectangle($sheen, 0, 0, 256, 127)
  $g.ResetClip()

  # The island at the top; the dock at the bottom with three apps on it, the middle one
  # swelling as the one under the cursor does.
  $glass = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(245, 255, 255, 255))
  $app = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 0x0B, 0x63, 0xDB))
  if ($size -ge 40) {
    $g.FillPath($glass, (Rounded 86 44 84 30 15))
    $g.FillPath($glass, (Rounded 38 156 180 58 29))
    $g.FillPath($app, (Rounded 74 176 21 21 6.5))
    $g.FillPath($app, (Rounded 113 167 30 30 9))
    $g.FillPath($app, (Rounded 161 176 21 21 6.5))
  } else {
    $g.FillPath($glass, (Rounded 78 36 100 40 20))
    $g.FillPath($glass, (Rounded 30 148 196 70 35))
  }
  $g.Dispose()

  $small = New-Object System.Drawing.Bitmap $size, $size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $s = [System.Drawing.Graphics]::FromImage($small)
  $s.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
  $s.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
  $s.CompositingQuality = [System.Drawing.Drawing2D.CompositingQuality]::HighQuality
  $attributes = New-Object System.Drawing.Imaging.ImageAttributes
  $attributes.SetWrapMode([System.Drawing.Drawing2D.WrapMode]::TileFlipXY)
  $s.DrawImage($big, (New-Object System.Drawing.Rectangle 0, 0, $size, $size), 0, 0, $big.Width, $big.Height, [System.Drawing.GraphicsUnit]::Pixel, $attributes)
  $s.Dispose(); $big.Dispose()
  return $small
}

# An icon file keeps a picture below 256 pixels as a bitmap: a header (its height counted
# twice), the pixels from the bottom row up, then a mask of one bit a pixel (set where
# nothing is). Every resource compiler takes that; only the 256 pixel one is kept as a PNG.
function AsBitmap($bmp) {
  $n = $bmp.Width
  $data = $bmp.LockBits((New-Object System.Drawing.Rectangle 0, 0, $n, $n), [System.Drawing.Imaging.ImageLockMode]::ReadOnly, [System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
  $pixels = New-Object byte[] ($n * $n * 4)
  [System.Runtime.InteropServices.Marshal]::Copy($data.Scan0, $pixels, 0, $pixels.Length)
  $bmp.UnlockBits($data)
  $ms = New-Object System.IO.MemoryStream
  $w = New-Object System.IO.BinaryWriter $ms
  $w.Write([uint32]40); $w.Write([int32]$n); $w.Write([int32]($n * 2)); $w.Write([uint16]1); $w.Write([uint16]32)
  $w.Write([uint32]0); $w.Write([uint32]($n * $n * 4)); $w.Write([int32]0); $w.Write([int32]0); $w.Write([uint32]0); $w.Write([uint32]0)
  for ($y = $n - 1; $y -ge 0; $y--) { $w.Write($pixels, $y * $n * 4, $n * 4) }
  $stride = [int]([Math]::Ceiling($n / 32.0) * 4)
  for ($y = $n - 1; $y -ge 0; $y--) {
    $row = New-Object byte[] $stride
    for ($x = 0; $x -lt $n; $x++) { if ($pixels[($y * $n + $x) * 4 + 3] -eq 0) { $row[[int][Math]::Floor($x / 8)] = $row[[int][Math]::Floor($x / 8)] -bor (0x80 -shr ($x % 8)) } }
    $w.Write($row)
  }
  $w.Flush()
  $bytes = $ms.ToArray()
  $w.Dispose()
  return , $bytes
}

New-Item -ItemType Directory -Force $Out | Out-Null
$sizes = 16, 20, 24, 32, 40, 48, 64, 256
$blobs = @()
foreach ($size in $sizes) {
  $bmp = Paint $size
  if ($size -eq 256) {
    $ms = New-Object System.IO.MemoryStream
    $bmp.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $bmp.Save((Join-Path $Out "logo.png"), [System.Drawing.Imaging.ImageFormat]::Png)
    $blobs += , $ms.ToArray()
    $ms.Dispose()
  } else {
    $blobs += , (AsBitmap $bmp)
  }
  $bmp.Dispose()
}
# The .ico: a short list of what is inside, then each picture.
$file = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $file
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$offset = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
  $side = if ($sizes[$i] -ge 256) { 0 } else { $sizes[$i] }
  $w.Write([byte]$side); $w.Write([byte]$side); $w.Write([byte]0); $w.Write([byte]0)
  $w.Write([uint16]1); $w.Write([uint16]32)
  $w.Write([uint32]$blobs[$i].Length); $w.Write([uint32]$offset)
  $offset += $blobs[$i].Length
}
foreach ($blob in $blobs) { $w.Write($blob) }
$w.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $Out "ben.ico"), $file.ToArray())
$w.Dispose()
"painted: logo.png (256 px) and ben.ico ($($sizes -join ', ') px) in $Out"

if ($LookAt) {
  # One sheet to look at: every size at its own size, and the small ones enlarged.
  $page = New-Object System.Drawing.Bitmap 760, 300
  $g = [System.Drawing.Graphics]::FromImage($page)
  $g.Clear([System.Drawing.Color]::FromArgb(255, 32, 34, 40))
  $g.FillRectangle((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 236, 238, 242))), 380, 0, 380, 300)
  $x = 16
  foreach ($size in 256, 64, 48, 32, 24, 16) {
    $bmp = Paint $size
    $g.DrawImage($bmp, $x, 16, $size, $size)
    $x += $size + 12
    $bmp.Dispose()
  }
  $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::NearestNeighbor
  $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::Half
  $x = 300
  foreach ($size in 16, 24, 32) {
    $bmp = Paint $size
    $g.DrawImage($bmp, $x, 110, $size * 5, $size * 5)
    $x += $size * 5 + 14
    $bmp.Dispose()
  }
  $g.Dispose()
  $page.Save($LookAt, [System.Drawing.Imaging.ImageFormat]::Png)
  $page.Dispose()
  "sheet saved: $LookAt"
}
