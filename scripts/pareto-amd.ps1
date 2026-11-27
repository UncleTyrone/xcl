# amd-dev pareto bundle (CGBJ-shaped; NOT Play-VIctory.cmd NVIDIA defaults).
#
# Usage:
#   . .\scripts\pareto-amd.ps1            # safe (omit residency/frame-cap; better presents/2s)
#   . .\scripts\pareto-amd.ps1 -Full      # denser HEAP_STRIDES/RESIDENCY/FRAME_INTERVAL/GPU_AWAKE
#   cargo build --release -p xcl   (from src/amd-dev)

param([switch]$Full)

$pareto = @{
    XCL_THIN                    = '1'
    XCL_JOIN_PARTS              = '8'
    XCL_ASYNC_SUBMIT            = '1'
    XCL_RELAY                   = '2'
    XCL_RELAY_COPY              = '1'
    XCL_LAND                    = '1'
    XCL_WIDE_BACK               = '1'
    XCL_CARD_STEPS              = '2'
    XCL_JOIN_COPIES             = '1'
    XCL_INDIRECT_BATCH          = '2'
    XCL_INDIRECT_WINDOW         = '1'
    XCL_AMBIENT                 = '0.8'
    XCL_AMBIENT_AIR             = '1.5'
    XCL_PORT_CONSTANTS          = '1'
    XCL_PORT_CONSTANTS_ORDER    = '1'
    XCL_PORT_DESCRIPTORS        = '1'
    XCL_PORT_CBV                = '1'
    XCL_PORT_GUARD              = '1'
    XCL_PORT_JOURNAL            = '1'
    XCL_TEXTURE_BACK            = '1'
    XCL_SHOW_AFTER              = '1'
    XCL_TEXTURE_NO_WAIT         = '1'
    XCL_UPLOAD_AHEAD            = '1'
    XCL_LEVELS_ON_DEMAND        = '1'
    XCL_GPU_AWAKE               = 'auto'
}

if ($Full) {
    $pareto['XCL_HEAP_STRIDES'] = '1'
    $pareto['XCL_RESIDENCY'] = '1'
    $pareto['XCL_FRAME_INTERVAL_MS'] = '16'
}

foreach ($k in $pareto.Keys) {
    Set-Item -Path "Env:$k" -Value $pareto[$k]
}

$label = if ($Full) { 'full' } else { 'safe' }
Write-Host "amd-dev $label pareto env set ($($pareto.Count) XCL_* variables)."
