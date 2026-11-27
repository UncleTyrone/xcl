# nvidia-dev pareto bundle — mirrors key/efforts/CGBJ/Play-VIctory.cmd (RTX residency path).
# Build from: src/nvidia-dev  (cargo build --release -p xcl)
# Usage:
#   . .\scripts\pareto-nvidia.ps1
#   # then inject xcl.dll / xcl run with --console series-x on 8 GB cards (per cmd comments)
#
# Defaults: THIN/JOIN off, RELAY=2, ports 0; LAND/WIDE_BACK off unless you opt in below.

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
    XCL_HEAP_STRIDES            = '1'
    XCL_RESIDENCY               = '1'
    XCL_FRAME_INTERVAL_MS       = '16'
    XCL_GPU_AWAKE               = '1'
}

# Opt-in land spine (Play-VIctory.cmd does not set LAND; pareto-amd does):
# $pareto['XCL_LAND'] = '1'
# $pareto['XCL_LAND_BESIDE'] = '1'

foreach ($k in $pareto.Keys) {
    Set-Item -Path "Env:$k" -Value $pareto[$k]
}

Write-Host "nvidia-dev pareto env set ($($pareto.Count) XCL_* variables)."