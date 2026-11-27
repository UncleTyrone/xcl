# amd-dev play env (CGBJ play.ps1 / play-env-amd shaped; NOT Play-XCL.cmd NVIDIA defaults).
#
# Usage:
#   . .\scripts\play-env-amd.ps1            # safe (omit residency/frame-cap; better presents/2s)
#   . .\scripts\play-env-amd.ps1 -Full      # denser HEAP_STRIDES/RESIDENCY/FRAME_INTERVAL/GPU_AWAKE
#   . .\scripts\play-env-amd.ps1 -Dev       # play.ps1 non-safe already is default thin/join here
#   cargo build --release -p xcl   (from src/amd-dev)

param(
    [switch]$Full,
    [switch]$Dev
)

$xclEnv = @{
    XCL_THIN                    = '1'
    XCL_JOIN_PARTS              = '8'
    XCL_ASYNC_SUBMIT            = '1'
    XCL_RELAY                   = '2'
    XCL_RELAY_COPY              = '1'
    XCL_LAND                    = '1'
    XCL_WIDE_BACK               = '1'
    XCL_CARD_STEPS              = '2'
    XCL_CARD_REHASH_RUNS        = '1'
    XCL_JOIN_COPIES             = '1'
    XCL_INDIRECT_BATCH          = '2'
    XCL_INDIRECT_WINDOW         = '1'
    XCL_MAP_THREADS             = '1'
    XCL_CARD_AWAKE              = '1'
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
    XCL_D3D12_DRED              = 'light'
    XCL_TITLE_PRIORITIES        = '0'
    XCL_SKIP_PIPELINES          = 'none'
    XCL_LIBRARY_STORE_LATER     = '1'
    XCL_PIPELINE_WAIT_MS        = '0'
    XCL_CARD_BUFFERS_MIN_KB     = '64'
}

if ($Dev) {
    # Same as default amd dense path; explicit for operators mirroring play.ps1 -Safe off.
    $xclEnv['XCL_THIN'] = '1'
    $xclEnv['XCL_JOIN_PARTS'] = '8'
    $xclEnv['XCL_UNDER_WIDE'] = '1'
    $xclEnv['XCL_LAND_ECHO'] = '1'
}

if ($Full) {
    $xclEnv['XCL_HEAP_STRIDES'] = '1'
    $xclEnv['XCL_RESIDENCY'] = '1'
    $xclEnv['XCL_FRAME_INTERVAL_MS'] = '16'
}

foreach ($k in $xclEnv.Keys) {
    Set-Item -Path "Env:$k" -Value $xclEnv[$k]
}

$label = if ($Dev) { 'dev' } elseif ($Full) { 'full' } else { 'safe' }
Write-Host "amd-dev $label play env set ($($xclEnv.Count) XCL_* variables)."
