# Starts the title with the developer play-env settings (mirrors CGBJ scripts/play.ps1 knobs as XCL_*).
#   scripts\play.ps1 -Exe <path of your own copy's executable>
# -Safe         without the newer, faster ways (first thing to try when something is wrong)
# -RayTracing   title ray-traced indirect light (BVH builder)
# -Build        target\release (default) — XCL has no separate diag profile name; use release
# -More         other settings: -More @{XCL_PANEL='1'}
# Run from a normal window, not as administrator: the driver then keeps its shader cache elsewhere.
param(
    [Parameter(Mandatory)][string]$Exe,
    [switch]$Safe,
    [switch]$RayTracing,
    [ValidateSet('release', 'debug')][string]$Build = 'release',
    [hashtable]$More = @{}
)
$settings = @{
    XCL_TEXTURE_NO_WAIT     = '1'
    XCL_RELAY               = '2'
    XCL_WIDE_BACK           = '1'
    XCL_CARD_REHASH_RUNS    = '1'
    XCL_JOIN_COPIES         = '1'
    XCL_INDIRECT_WINDOW     = '1'
    XCL_RELAY_COPY          = '1'
    XCL_CARD_STEPS          = '2'
    XCL_INDIRECT_BATCH      = '2'
    XCL_MAP_THREADS         = '1'
    XCL_TITLE_PRIORITIES    = '0'
    XCL_CARD_AWAKE          = '1'
    XCL_GPU_AWAKE           = '1'
    XCL_SHOW_AFTER          = '1'
    XCL_AMBIENT             = '0.8'
    XCL_D3D12_DRED          = 'light'
    XCL_SKIP_PIPELINES      = 'none'
}
if ($Safe) {
    $settings.XCL_UNDER_WIDE = '0'
    $settings.XCL_LAND_ECHO = '0'
} else {
    $settings.XCL_THIN = '1'
    $settings.XCL_JOIN_PARTS = '8'
    $settings.XCL_UPLOAD_AHEAD = '1'
}
if ($RayTracing) { $settings.XCL_BVH_BUILDER = '1' }
foreach ($k in $More.Keys) { $settings[$k] = $More[$k] }

$xcl = Join-Path $PSScriptRoot "..\target\$Build\xcl.exe"
if (-not (Test-Path $xcl)) { Write-Host "No $xcl. Build it first (BUILDING.md)."; return }
if (-not (Test-Path $Exe)) { Write-Host "No $Exe."; return }
# Clear leftover XCL_* in this window; keep path overrides.
$paths = 'XCL_CONFIG', 'XCL_GAME_DIR', 'XCL_CACHE_DIR', 'XCL_DATA_DIR', 'XCL_DATA_ROOT', 'XCL_DEVICE_TYPE'
Get-ChildItem Env:XCL_* | Where-Object { $_.Name -notin $paths } | ForEach-Object { Remove-Item "Env:$($_.Name)" }
foreach ($k in $settings.Keys) { Set-Item "Env:$k" $settings[$k] }
$Exe = (Resolve-Path $Exe).Path
Push-Location (Join-Path $PSScriptRoot '..')
try { & $xcl run $Exe } finally { Pop-Location }
