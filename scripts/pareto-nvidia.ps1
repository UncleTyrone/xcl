# nvidia-dev pareto bundle — Play-XCL.cmd-shaped with safe/full/dev split (play.ps1 knobs covered).
#
# Usage:
#   . .\scripts\pareto-nvidia.ps1            # safe (rate-friendly; no RESIDENCY/frame-cap)
#   . .\scripts\pareto-nvidia.ps1 -Full      # denser residency/heap/awake (may lower presents/2s)
#   . .\scripts\pareto-nvidia.ps1 -Dev       # play.ps1 non-safe: THIN=1 JOIN_PARTS=8
#   cargo build --release -p xcl   (from src/nvidia-dev)

param(
    [switch]$Full,
    [switch]$Dev
)

$pareto = @{
    XCL_TEXTURE_NO_WAIT         = '1'
    XCL_RELAY                   = '2'
    XCL_RELAY_COPY              = '1'
    XCL_WIDE_BACK               = '1'
    XCL_CARD_REHASH_RUNS        = '1'
    XCL_JOIN_COPIES             = '1'
    XCL_INDIRECT_WINDOW         = '1'
    XCL_CARD_STEPS              = '2'
    XCL_INDIRECT_BATCH          = '2'
    XCL_MAP_THREADS             = '1'
    XCL_CARD_AWAKE              = '1'
    XCL_SHOW_AFTER              = '1'
    XCL_AMBIENT_AIR             = '1.5'
    XCL_AMBIENT                 = 'constant:0.8'
    XCL_TEXTURE_GUARD           = '0'
    XCL_THIN                    = '0'
    XCL_JOIN_PARTS              = '0'
    XCL_PORT_CONSTANTS          = '0'
    XCL_PORT_CBV                = '0'
    XCL_LIBRARY_STORE_LATER     = '1'
    XCL_PIPELINE_WAIT_MS        = '0'
    XCL_UPLOAD_AHEAD            = '1'
    XCL_LEVELS_ON_DEMAND        = '1'
    XCL_CARD_BUFFERS_MIN_KB     = '64'
    XCL_D3D12_DRED              = 'light'
    XCL_TITLE_PRIORITIES        = '0'
    XCL_SKIP_PIPELINES          = 'none'
}

if ($Dev) {
    $pareto['XCL_THIN'] = '1'
    $pareto['XCL_JOIN_PARTS'] = '8'
    $pareto['XCL_AMBIENT'] = '0.8'
}

if ($Full) {
    $pareto['XCL_HEAP_STRIDES'] = '1'
    $pareto['XCL_RESIDENCY'] = '1'
    $pareto['XCL_FRAME_INTERVAL_MS'] = '16'
    $pareto['XCL_GPU_AWAKE'] = '1'
}

# Opt-in land spine (Play-XCL.cmd does not set LAND; pareto-amd does):
# $pareto['XCL_LAND'] = '1'
# $pareto['XCL_LAND_BESIDE'] = '1'

foreach ($k in $pareto.Keys) {
    Set-Item -Path "Env:$k" -Value $pareto[$k]
}

$label = if ($Dev) { 'dev' } elseif ($Full) { 'full' } else { 'safe' }
Write-Host "nvidia-dev $label pareto env set ($($pareto.Count) XCL_* variables)."
