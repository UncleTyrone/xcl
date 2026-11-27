# amd-dev pareto bundle (maps CGBJ scripts/play.ps1 + extended ports; NOT Play-VIctory.cmd NVIDIA defaults).
# Build from: src/amd-dev  (cargo build --release -p xcl)
# Usage:
#   . .\scripts\pareto-amd.ps1
#   # then launch your title with xcl.dll injected / xcl run ...
#
# Safe/minimal: comment out THIN / JOIN / async if skin or video regress.

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
    XCL_HEAP_STRIDES            = '1'
    XCL_GPU_AWAKE               = 'auto'
    XCL_RESIDENCY               = '1'
    XCL_FRAME_INTERVAL_MS       = '16'
}

foreach ($k in $pareto.Keys) {
    Set-Item -Path "Env:$k" -Value $pareto[$k]
}

Write-Host "amd-dev pareto env set ($($pareto.Count) XCL_* variables)."