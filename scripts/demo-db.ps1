<#
.SYNOPSIS
  Switches Kept between your own history and the demonstration database, and
  seeds that database with demo content.

.DESCRIPTION
  Screenshots and demo videos must never show personal clips. Kept records the
  data folder it should open in `storage.json` inside its default app-data
  directory, so this script rewrites that pointer rather than copying anything:
  your real database is left exactly where it is, untouched, and the demo
  database lives on its own at %LOCALAPPDATA%\kept-demo\data.

  The demo database is kept between sessions on purpose, so a run of demos starts
  from the same history every time. Its content is only ever the generated filler
  from `-Seed`.

.PARAMETER Use
  Point Kept at the demonstration database (creating it if it is missing).

.PARAMETER Restore
  Point Kept back at the real database recorded the first time `-Use` ran.

.PARAMETER Seed
  Fill the demonstration database with generated images and a handful of text
  clips by copying them, so Kept captures them the way it captures anything else.
  Runs once: a marker file in the demo folder stops it repeating.

.PARAMETER Status
  Say which database Kept would open, and what is in the demo folder.

.PARAMETER Reset
  Throw the demonstration database away and build it again from nothing: a fresh
  database, the demo profile, and the generated seed content. Use this when a run
  of recordings has added archives, stacks or clips and the demo should start from
  the same state as last time.

.PARAMETER NoRestart
  Leave the running app alone. Kept only reads the pointer at startup, so without
  a restart a switch takes effect the next time the app starts.

.EXAMPLE
  .\scripts\demo-db.ps1 -Use

.EXAMPLE
  .\scripts\demo-db.ps1 -Reset

.EXAMPLE
  .\scripts\demo-db.ps1 -Restore
#>
[CmdletBinding(DefaultParameterSetName = "Status")]
param(
  [Parameter(ParameterSetName = "Use")][switch]$Use,
  [Parameter(ParameterSetName = "Restore")][switch]$Restore,
  [Parameter(ParameterSetName = "Seed")][switch]$Seed,
  [Parameter(ParameterSetName = "Reset")][switch]$Reset,
  [Parameter(ParameterSetName = "Status")][switch]$Status,
  [switch]$NoRestart
)

$ErrorActionPreference = "Stop"

$AppData = Join-Path $env:APPDATA "ikept.it"
$PointerPath = Join-Path $AppData "storage.json"
$DemoRoot = Join-Path $env:LOCALAPPDATA "kept-demo"
$DemoData = Join-Path $DemoRoot "data"
$RealPointerBackup = Join-Path $DemoRoot "real-pointer.json"
$SeedMarker = Join-Path $DemoRoot "seeded.json"
$DefaultData = $AppData

function Read-Pointer {
  if (-not (Test-Path $PointerPath)) { return $null }
  try { return Get-Content $PointerPath -Raw | ConvertFrom-Json } catch { return $null }
}

function Write-Pointer([string]$DataDir) {
  New-Item -ItemType Directory -Force -Path $AppData | Out-Null
  $pointer = @{ dataDir = $DataDir; previousDir = $null } | ConvertTo-Json
  [System.IO.File]::WriteAllText($PointerPath, $pointer)
  Write-Host "Kept will open $DataDir"
}

function Restart-Kept {
  if ($NoRestart) { Write-Host "Left the running app alone; the switch applies at its next start."; return }
  $running = Get-Process kept -ErrorAction SilentlyContinue
  foreach ($process in $running) { Stop-Process -Id $process.Id -Force }
  if ($running) { Start-Sleep -Seconds 3 }
  Start-Process (Join-Path $env:LOCALAPPDATA "Kept\kept.exe")
  Start-Sleep -Seconds 6
  Write-Host "Kept restarted."
}

function Invoke-Use {
  New-Item -ItemType Directory -Force -Path @($DemoData, (Join-Path $DemoData "media")) | Out-Null
  $db = Join-Path $DemoData "copypaste.db"
  if (-not (Test-Path $db)) {
    # An empty file is a valid empty SQLite database: Kept creates its schema in it.
    [System.IO.File]::WriteAllBytes($db, [byte[]]@())
    $settingsScript = @'
import sqlite3, json, sys
path = sys.argv[1]
con = sqlite3.connect(path)
con.executescript("""
CREATE TABLE IF NOT EXISTS meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS settings (key TEXT PRIMARY KEY, value TEXT NOT NULL);
""")
def put(key, value):
    con.execute("insert into settings(key, value) values(?, ?) on conflict(key) do update set value=excluded.value", (key, json.dumps(value)))
put("setupComplete", True)
put("autoUpdate", False)
put("pasteStackHotkey", "Alt+Shift+V")
put("quickPasteEnabled", True)
put("quickPasteToken", "::")
# Every sidebar section open: a recording should not have to click a chevron
# before its subject is visible.
put("sidebarCollapsed", [])
put("sidebarOpen", True)
put("detailPaneOpen", True)
con.commit()
con.close()
'@
    $tmp = Join-Path $DemoRoot "settings.py"
    [System.IO.File]::WriteAllText($tmp, $settingsScript)
    $python = Get-Command python -ErrorAction SilentlyContinue
    if ($python) {
      # Settings live in SQLite, so the demo profile is written with Python's driver.
      & $python.Source $tmp $db
      Remove-Item $tmp -Force
    } else {
      Write-Warning "Python was not found: start Kept once and answer the setup wizard instead."
    }
  }

  if (-not (Test-Path $RealPointerBackup)) {
    $current = Read-Pointer
    # Never record the demo folder as the real one.
    $real = if ($current -and $current.dataDir -and $current.dataDir -ne $DemoData) { $current.dataDir } else { $DefaultData }
    New-Item -ItemType Directory -Force -Path $DemoRoot | Out-Null
    (@{ dataDir = $real } | ConvertTo-Json) | Set-Content $RealPointerBackup -Encoding UTF8
    Write-Host "Remembered the real data folder: $real"
  }

  Write-Pointer $DemoData
  Restart-Kept
}

function Invoke-Restore {
  if (-not (Test-Path $RealPointerBackup)) {
    throw "No remembered real folder. $RealPointerBackup is missing, so there is nothing to restore to."
  }
  $real = (Get-Content $RealPointerBackup -Raw | ConvertFrom-Json).dataDir
  Write-Pointer $real
  Restart-Kept
}

function Invoke-Seed {
  if (Test-Path $SeedMarker) {
    Write-Host "The demonstration database is already seeded ($SeedMarker)."
    return
  }
  Add-Type -AssemblyName System.Drawing, System.Windows.Forms
  $seedDir = Join-Path $DemoRoot "seed"
  New-Item -ItemType Directory -Force -Path $seedDir | Out-Null

  # Five filler pictures, each with a palette worth archiving.
  $pictures = @(
    @{ Name = "aurora.png";  Colors = @("#0B1220", "#1A587A", "#24C8DB", "#E7FAFF") },
    @{ Name = "citrus.png";  Colors = @("#FFC131", "#FF8C42", "#E83E5A", "#78184C") },
    @{ Name = "forest.png";  Colors = @("#092018", "#16543A", "#4AA86E", "#D8F3DE") },
    @{ Name = "dusk.png";    Colors = @("#181430", "#4A3480", "#C678B2", "#FFD6A5") },
    @{ Name = "dashboard.png"; Colors = @("#12161F", "#6D8CFF", "#E0992D", "#4ADE80") }
  )

  foreach ($picture in $pictures) {
    $width = 900; $height = 640
    $bitmap = New-Object System.Drawing.Bitmap $width, $height
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $rect = New-Object System.Drawing.Rectangle 0, 0, $width, $height
    $brush = New-Object System.Drawing.Drawing2D.LinearGradientBrush(
      $rect,
      [System.Drawing.ColorTranslator]::FromHtml($picture.Colors[0]),
      [System.Drawing.ColorTranslator]::FromHtml($picture.Colors[2]),
      35.0)
    $blend = New-Object System.Drawing.Drawing2D.ColorBlend 3
    $blend.Colors = @(
      [System.Drawing.ColorTranslator]::FromHtml($picture.Colors[0]),
      [System.Drawing.ColorTranslator]::FromHtml($picture.Colors[1]),
      [System.Drawing.ColorTranslator]::FromHtml($picture.Colors[2])
    )
    $blend.Positions = @(0.0, 0.55, 1.0)
    $brush.InterpolationColors = $blend
    $graphics.FillRectangle($brush, $rect)

    foreach ($spot in @(@{X = 660; Y = 180; R = 150}, @{X = 220; Y = 470; R = 190})) {
      $accent = [System.Drawing.ColorTranslator]::FromHtml($picture.Colors[3])
      $spotBrush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(56, $accent))
      $graphics.FillEllipse($spotBrush, $spot.X - $spot.R, $spot.Y - $spot.R, $spot.R * 2, $spot.R * 2)
      $spotBrush.Dispose()
    }

    $bitmap.Save((Join-Path $seedDir $picture.Name), [System.Drawing.Imaging.ImageFormat]::Png)
    $brush.Dispose(); $graphics.Dispose(); $bitmap.Dispose()
  }

  if (-not (Get-Process kept -ErrorAction SilentlyContinue)) {
    Write-Warning "Kept is not running, so nothing can be captured. Start it on the demo database and run -Seed again."
    return
  }

  foreach ($picture in $pictures) {
    $image = [System.Drawing.Image]::FromFile((Join-Path $seedDir $picture.Name))
    [System.Windows.Forms.Clipboard]::SetImage($image)
    $image.Dispose()
    Start-Sleep -Milliseconds 1400
  }

  $texts = @(
    "Kept keeps everything you copy, searchable and local.",
    "https://github.com/jamesbuckley-source/kept/releases",
    "#2A414D",
    '{"feature":"paste stack","state":"demo","items":3}',
    'git commit -m "Add the new mark to the tray and the title bar"'
  )
  foreach ($text in $texts) {
    Set-Clipboard -Value $text
    Start-Sleep -Milliseconds 1200
  }

  @{ seededAt = (Get-Date).ToString("o"); pictures = $pictures.Count; texts = $texts.Count } |
    ConvertTo-Json | Set-Content $SeedMarker -Encoding UTF8
  Write-Host "Seeded the demonstration database with $($pictures.Count) pictures and $($texts.Count) text clips."
}

function Invoke-Reset {
  Write-Host "Rebuilding the demonstration database from nothing..."
  $running = Get-Process kept -ErrorAction SilentlyContinue
  foreach ($process in $running) { Stop-Process -Id $process.Id -Force }
  if ($running) { Start-Sleep -Seconds 3 }

  foreach ($name in @("copypaste.db", "copypaste.db-wal", "copypaste.db-shm")) {
    $path = Join-Path $DemoData $name
    if (Test-Path $path) { Remove-Item $path -Force }
  }
  $media = Join-Path $DemoData "media"
  if (Test-Path $media) { Get-ChildItem $media -File | Remove-Item -Force }
  if (Test-Path $SeedMarker) { Remove-Item $SeedMarker -Force }

  Invoke-Use
  Invoke-Seed
}

function Invoke-Status {
  $pointer = Read-Pointer
  $active = if ($pointer -and $pointer.dataDir) { $pointer.dataDir } else { "$DefaultData (default)" }
  Write-Host "Kept opens: $active"
  $onDemo = $active -eq $DemoData
  Write-Host "Demonstration database: $(if ($onDemo) { 'IN USE' } else { 'not in use' })"
  if (Test-Path $DemoData) {
    $db = Join-Path $DemoData "copypaste.db"
    $size = if (Test-Path $db) { (Get-Item $db).Length } else { 0 }
    $media = @(Get-ChildItem (Join-Path $DemoData "media") -File -ErrorAction SilentlyContinue).Count
    Write-Host "  folder: $DemoData"
    Write-Host "  database: $size bytes, $media media files"
    Write-Host "  seeded: $(Test-Path $SeedMarker)"
  } else {
    Write-Host "  folder: $DemoData (not created yet)"
  }
  if (Test-Path $RealPointerBackup) {
    Write-Host "Real data folder recorded: $((Get-Content $RealPointerBackup -Raw | ConvertFrom-Json).dataDir)"
  }
}

switch ($PSCmdlet.ParameterSetName) {
  "Use" { Invoke-Use }
  "Restore" { Invoke-Restore }
  "Seed" { Invoke-Seed }
  "Reset" { Invoke-Reset }
  default { Invoke-Status }
}
