@echo off
setlocal EnableExtensions DisableDelayedExpansion
if not defined XCL_GAME_DIR (
  echo Set XCL_GAME_DIR to the folder containing your title executable. 1>&2
  exit /b 2
)
if not exist "%XCL_GAME_DIR%\game_scarlett_master.exe" (
  echo game_scarlett_master.exe not found in XCL_GAME_DIR: "%XCL_GAME_DIR%" 1>&2
  exit /b 2
)
set "runner=%~dp0target\release\xcl.exe"
if not exist "%runner%" set "runner=%~dp0xcl-3d-rtx-pareto-residency.exe"
if not exist "%runner%" (
  echo Extract the Windows release or run cargo build --release -p xcl. 1>&2
  exit /b 2
)

set "XCL_CACHE_DIR=%LOCALAPPDATA%\XCL\cache-pareto"
set "XCL_AMBIENT_AIR=1.5"
set "XCL_AMBIENT=0.8"
set "XCL_TEXTURE_NO_WAIT=1"
set "XCL_RELAY=2"
set "XCL_WIDE_BACK=1"
set "XCL_CARD_REHASH_RUNS=1"
set "XCL_JOIN_COPIES=1"
set "XCL_INDIRECT_WINDOW=1"
set "XCL_RELAY_COPY=1"
set "XCL_CARD_STEPS=2"
set "XCL_INDIRECT_BATCH=2"
set "XCL_MAP_THREADS=1"
set "XCL_CARD_AWAKE=1"
set "XCL_GPU_AWAKE=1"
set "XCL_SHOW_AFTER=1"
if not defined XCL_THIN set "XCL_THIN=0"
if not defined XCL_JOIN_PARTS set "XCL_JOIN_PARTS=0"
if not defined XCL_LIBRARY_STORE_LATER set "XCL_LIBRARY_STORE_LATER=1"
if not defined XCL_PIPELINE_WAIT_MS set "XCL_PIPELINE_WAIT_MS=0"
if not defined XCL_PORT_CONSTANTS set "XCL_PORT_CONSTANTS=0"
if not defined XCL_PORT_CBV set "XCL_PORT_CBV=0"
if not defined XCL_UPLOAD_AHEAD set "XCL_UPLOAD_AHEAD=1"
set "XCL_LEVELS_ON_DEMAND=1"

:: 4 GB VRAM tuning: reduce card-buffer thrashing and driver background recompile threads
set "XCL_CARD_BUFFERS_MIN_KB=4096"
set "XCL_CARD_BUFFERS_MB=256"
set "XCL_D3D12_BACKGROUND=0"
if not defined XCL_LOG set "XCL_LOG=warn"
set "XCL_DEVICE_TYPE=lockhart"

"%runner%" run "%XCL_GAME_DIR%\game_scarlett_master.exe"
exit /b %errorlevel%
