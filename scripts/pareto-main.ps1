# src/xcl (main) — safe minimal env (thin/join off). Frame-path modules are present;
# enable XCL_RELAY=2 / LAND / CARD_STEPS / WIDE_BACK explicitly, or use amd-dev
# (pareto-amd.ps1) / nvidia-dev (pareto-nvidia.ps1) for full CGBJ-shaped bundles.
#
# Usage:
#   . .\scripts\pareto-main.ps1
#   # build from src/xcl: cargo build --release -p xcl

$pareto = @{
    XCL_THIN                    = '0'
    XCL_JOIN_PARTS              = '0'
    XCL_ASYNC_SUBMIT            = '0'
    XCL_TEXTURE_NO_WAIT         = '1'
    XCL_SHOW_AFTER              = '1'
    XCL_UPLOAD_AHEAD            = '1'
    XCL_TEXTURE_BACK            = '1'
    XCL_INDIRECT_REST           = '0'
    XCL_D3D12_DRED              = 'light'
    XCL_TEXTURE_GUARD           = '1'
    XCL_PORT_CONSTANTS          = '0'
    XCL_PORT_CBV                = '0'
    XCL_HEAP_STRIDES            = '1'
    XCL_RESIDENCY               = '1'
    XCL_FRAME_INTERVAL_MS       = '16'
    XCL_GPU_AWAKE               = '1'
}

foreach ($k in $pareto.Keys) {
    Set-Item -Path "Env:$k" -Value $pareto[$k]
}

Write-Host "src/xcl safe pareto env set ($($pareto.Count) XCL_* variables)."
