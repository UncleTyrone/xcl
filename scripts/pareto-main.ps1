# src/xcl (main) pareto env — profile split for present-rate vs CGBJ-dense.
#
# Usage:
#   . .\scripts\pareto-main.ps1            # safe (rate-friendly)
#   . .\scripts\pareto-main.ps1 -Full      # denser residency/heap/awake (may lower presents/2s)
#   cargo build --release -p xcl

param([switch]$Full)

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
}

if ($Full) {
    $pareto['XCL_HEAP_STRIDES'] = '1'
    $pareto['XCL_RESIDENCY'] = '1'
    $pareto['XCL_FRAME_INTERVAL_MS'] = '16'
    $pareto['XCL_GPU_AWAKE'] = '1'
}

foreach ($k in $pareto.Keys) {
    Set-Item -Path "Env:$k" -Value $pareto[$k]
}

$label = if ($Full) { 'full' } else { 'safe' }
Write-Host "src/xcl $label pareto env set ($($pareto.Count) XCL_* variables)."
