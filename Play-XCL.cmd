@echo off
setlocal EnableExtensions DisableDelayedExpansion
if not "%~1"=="" if /i not "%~1"=="pareto-residency" (
  echo Usage: Play-XCL.cmd pareto-residency 1^>&2
  exit /b 2
)
if not defined XCL_GAME_DIR (
  echo Set XCL_GAME_DIR to the folder containing your title executable. 1>&2
  exit /b 2
)
if not exist "%XCL_GAME_DIR%\game_scarlett_master.exe" (
  echo game_scarlett_master.exe not found in XCL_GAME_DIR: "%XCL_GAME_DIR%" 1>&2
  exit /b 2
)
set "runner=%~dp0xcl-3d-rtx-pareto-residency.exe"
if not exist "%runner%" set "runner=%~dp0target\release\xcl.exe"
if not exist "%runner%" (
  echo Extract the Windows release or run cargo build --release -p xcl. 1>&2
  exit /b 2
)
if not defined XCL_AMBIENT_AIR set "XCL_AMBIENT_AIR=1.5"
if not defined XCL_CACHE_DIR set "XCL_CACHE_DIR=%LOCALAPPDATA%\XCL\cache-pareto"
if not defined XCL_TEXTURE_NO_WAIT set "XCL_TEXTURE_NO_WAIT=1"
if not defined XCL_RELAY set "XCL_RELAY=2"
if not defined XCL_WIDE_BACK set "XCL_WIDE_BACK=1"
if not defined XCL_CARD_REHASH_RUNS set "XCL_CARD_REHASH_RUNS=1"
if not defined XCL_JOIN_COPIES set "XCL_JOIN_COPIES=1"
if not defined XCL_INDIRECT_WINDOW set "XCL_INDIRECT_WINDOW=1"
if not defined XCL_RELAY_COPY set "XCL_RELAY_COPY=1"
if not defined XCL_CARD_STEPS set "XCL_CARD_STEPS=2"
if not defined XCL_INDIRECT_BATCH set "XCL_INDIRECT_BATCH=2"
if not defined XCL_MAP_THREADS set "XCL_MAP_THREADS=1"
if not defined XCL_CARD_AWAKE set "XCL_CARD_AWAKE=1"
if not defined XCL_GPU_AWAKE if defined XCL_CARD_AWAKE set "XCL_GPU_AWAKE=%XCL_CARD_AWAKE%"
if not defined XCL_SHOW_AFTER set "XCL_SHOW_AFTER=1"
rem Flat ambient: frame-estimate washout on bright doorway transitions.
if not defined XCL_AMBIENT set "XCL_AMBIENT=constant:0.8"
rem Texture guard off by default on NVIDIA-shaped path (colour noise on skin/phone otherwise).
if not defined XCL_TEXTURE_GUARD set "XCL_TEXTURE_GUARD=0"
rem Thin / join-parts stay off on NVIDIA-shaped defaults (head/skin and opening-video issues).
if not defined XCL_THIN set "XCL_THIN=0"
if not defined XCL_JOIN_PARTS set "XCL_JOIN_PARTS=0"
rem Renderer ports 1 and 5 stay opt-in on 8 GB cards.
if not defined XCL_PORT_CONSTANTS set "XCL_PORT_CONSTANTS=0"
if not defined XCL_PORT_CBV set "XCL_PORT_CBV=0"
if not defined XCL_LIBRARY_STORE_LATER set "XCL_LIBRARY_STORE_LATER=1"
if not defined XCL_PIPELINE_WAIT_MS set "XCL_PIPELINE_WAIT_MS=0"
if not defined XCL_UPLOAD_AHEAD set "XCL_UPLOAD_AHEAD=1"
if not defined XCL_LEVELS_ON_DEMAND set "XCL_LEVELS_ON_DEMAND=1"
if not defined XCL_CARD_BUFFERS_MIN_KB set "XCL_CARD_BUFFERS_MIN_KB=64"
if not defined XCL_LOG if not defined RUST_LOG set "XCL_LOG=warn"
if not defined XCL_DEVICE_TYPE set "XCL_DEVICE_TYPE=anaconda"
"%runner%" run "%XCL_GAME_DIR%\game_scarlett_master.exe"
exit /b %errorlevel%
