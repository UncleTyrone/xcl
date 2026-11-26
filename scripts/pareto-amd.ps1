# amd-dev pareto bundle (maps CGBJ Play-VIctory.cmd / scripts/play.ps1 to XCL_*).
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
    # CGBJ VICTORY_SHOW_AFTER — no amd-dev switch yet; omit until texture_policy lands.
}

foreach ($k in $pareto.Keys) {
    Set-Item -Path "Env:$k" -Value $pareto[$k]
}

Write-Host "amd-dev pareto env set ($($pareto.Count) XCL_* variables)."