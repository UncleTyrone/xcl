# src/xcl (main) play env — Play-XCL / play.ps1 knobs with safe vs full split.
#
# Usage:
#   . .\scripts\play-env-main.ps1            # safe (rate-friendly; thin/join off like Play-XCL.cmd)
#   . .\scripts\play-env-main.ps1 -Full      # denser residency/heap/awake (may lower presents/2s)
#   . .\scripts\play-env-main.ps1 -Dev       # play.ps1 non-safe: THIN=1 JOIN_PARTS=8
#   cargo build --release -p xcl

param(
    [switch]$Full,
    [switch]$Dev
)

$xclEnv = @{
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
    XCL_ASYNC_SUBMIT            = '0'
    XCL_PORT_CONSTANTS          = '0'
    XCL_PORT_CBV                = '0'
    XCL_LIBRARY_STORE_LATER     = '1'
    XCL_PIPELINE_WAIT_MS        = '0'
    XCL_UPLOAD_AHEAD            = '1'
    XCL_LEVELS_ON_DEMAND        = '1'
    XCL_CARD_BUFFERS_MIN_KB     = '64'
    XCL_TEXTURE_BACK            = '1'
    XCL_INDIRECT_REST           = '0'
    XCL_D3D12_DRED              = 'light'
    XCL_TITLE_PRIORITIES        = '0'
    XCL_SKIP_PIPELINES          = 'none'
}

if ($Dev) {
    $xclEnv['XCL_THIN'] = '1'
    $xclEnv['XCL_JOIN_PARTS'] = '8'
    $xclEnv['XCL_AMBIENT'] = '0.8'
}

if ($Full) {
    $xclEnv['XCL_HEAP_STRIDES'] = '1'
    $xclEnv['XCL_RESIDENCY'] = '1'
    $xclEnv['XCL_FRAME_INTERVAL_MS'] = '16'
    $xclEnv['XCL_GPU_AWAKE'] = '1'
}

foreach ($k in $xclEnv.Keys) {
    Set-Item -Path "Env:$k" -Value $xclEnv[$k]
}

$label = if ($Dev) { 'dev' } elseif ($Full) { 'full' } else { 'safe' }
Write-Host "src/xcl $label play env set ($($xclEnv.Count) XCL_* variables)."
