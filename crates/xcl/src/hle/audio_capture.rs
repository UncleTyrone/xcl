//! `XCL_AUDIO_CAPTURE=<file.wav>`: record what the title plays (48 kHz stereo
//! PCM16) through process loopback — this process tree only, never the host
//! mix. Gaps where nothing played are written as silence so the file keeps
//! wall-clock time.
//!
//! The recorder runs in a child copy of this executable: an audio client in
//! the title's process raises that process's timer resolution, which would
//! change how the title's own waits behave.

use std::fs::File;
use std::io::{Seek, SeekFrom, Write};
use std::mem::size_of_val;
use std::path::Path;
use std::time::{Duration, Instant};

use windows::core::{implement, Interface, Ref, HRESULT, IUnknown};
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    IActivateAudioInterfaceAsyncOperation, IActivateAudioInterfaceCompletionHandler,
    IActivateAudioInterfaceCompletionHandler_Impl, IAudioCaptureClient, IAudioClient,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
    WAVEFORMATEX, WAVE_FORMAT_PCM,
};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
use windows::Win32::System::Threading::{OpenProcess, Sleep, WaitForSingleObject, PROCESS_SYNCHRONIZE};

const RATE: u32 = 48_000;
const FRAME_BYTES: usize = 4;

/// Activation completes asynchronously; the recording thread polls for it.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct Activated;

impl IActivateAudioInterfaceCompletionHandler_Impl for Activated_Impl {
    fn ActivateCompleted(
        &self,
        _: Ref<IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
}

/// Names the process to record, in the recorder's environment.
const TARGET: &str = "XCL_AUDIO_CAPTURE_OF";

/// Starts the recorder if `XCL_AUDIO_CAPTURE` (or `VICTORY_AUDIO_CAPTURE`) names a file.
pub fn start() {
    let path = std::env::var_os("XCL_AUDIO_CAPTURE").or_else(|| std::env::var_os("VICTORY_AUDIO_CAPTURE"));
    if path.is_none() {
        return;
    }
    if tracing_or_log_timing_probe() {
        let _ = std::thread::Builder::new()
            .name("xcl timing probe".into())
            .spawn(|| loop {
                std::thread::sleep(Duration::from_secs(1));
                let at = Instant::now();
                // SAFETY: plain wait.
                unsafe { Sleep(1) };
                let ms = at.elapsed().as_secs_f32() * 1000.0;
                crate::xlog_info!("Sleep(1) took {ms:.3} ms");
            });
    }
    if let Err(e) = std::env::current_exe().and_then(|exe| {
        std::process::Command::new(exe)
            .env(TARGET, std::process::id().to_string())
            .env("XCL_AUDIO_CAPTURE", path.as_ref().unwrap())
            .spawn()
    }) {
        crate::xlog_warn!("audio capture not started: {e}");
    } else {
        crate::xlog_info!("XCL_AUDIO_CAPTURE: recorder child started");
    }
}

fn tracing_or_log_timing_probe() -> bool {
    std::env::var("XCL_TRACE_APU").is_ok_and(|v| !v.is_empty() && v != "0")
        || std::env::var("XCL_LOG").is_ok_and(|v| v.to_ascii_lowercase().contains("debug"))
}

/// In the recorder (a child started by [`start`]): records until the target
/// exits and returns true; false in any other process.
pub fn child() -> bool {
    let pid = std::env::var(TARGET).ok().and_then(|p| p.parse().ok());
    let path = std::env::var_os("XCL_AUDIO_CAPTURE").or_else(|| std::env::var_os("VICTORY_AUDIO_CAPTURE"));
    let (Some(pid), Some(path)) = (pid, path) else {
        return false;
    };
    if let Err(e) = record(pid, Path::new(&path)) {
        crate::xlog_warn!("audio capture stopped: {e}");
    }
    true
}

fn header(frames: u64) -> [u8; 44] {
    let data = u32::try_from(frames * FRAME_BYTES as u64).unwrap_or(u32::MAX - 36);
    let mut h = [0u8; 44];
    let fields: [(&[u8], usize); 11] = [
        (b"RIFF", 0),
        (&(36 + data).to_le_bytes(), 4),
        (b"WAVEfmt ", 8),
        (&16u32.to_le_bytes(), 16),
        (&1u16.to_le_bytes(), 20),
        (&2u16.to_le_bytes(), 22),
        (&RATE.to_le_bytes(), 24),
        (&(RATE * FRAME_BYTES as u32).to_le_bytes(), 28),
        (&(FRAME_BYTES as u16).to_le_bytes(), 32),
        (&16u16.to_le_bytes(), 34),
        (b"data", 36),
    ];
    for (bytes, at) in fields {
        h[at..at + bytes.len()].copy_from_slice(bytes);
    }
    h[40..].copy_from_slice(&data.to_le_bytes());
    h
}

fn record(pid: u32, path: &Path) -> Result<(), Box<dyn std::error::Error>> {
    // SAFETY: COM calls with parameters built here; buffers are used only
    // between `GetBuffer` and `ReleaseBuffer`.
    unsafe {
        CoInitializeEx(None, COINIT_MULTITHREADED).ok()?;
        let target = OpenProcess(PROCESS_SYNCHRONIZE, false, pid)?;
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        /// A `PROPVARIANT` holding a blob (`VT_BLOB`).
        #[repr(C)]
        struct Blob {
            vt: u16,
            reserved: [u16; 3],
            size: u32,
            data: *const AUDIOCLIENT_ACTIVATION_PARAMS,
        }
        let blob = Blob {
            vt: 0x41,
            reserved: [0; 3],
            size: size_of_val(&params) as u32,
            data: &params,
        };
        let handler: IActivateAudioInterfaceCompletionHandler = Activated.into();
        let operation = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some((&raw const blob).cast()),
            &handler,
        )?;
        let client = loop {
            let (mut hr, mut client) = (HRESULT(0), None::<IUnknown>);
            // Fails (`E_ILLEGAL_METHOD_CALL`) until activation completes.
            if operation.GetActivateResult(&mut hr, &mut client).is_ok() {
                hr.ok()?;
                break client.ok_or("no audio client")?.cast::<IAudioClient>()?;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: 2,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * FRAME_BYTES as u32,
            nBlockAlign: FRAME_BYTES as u16,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM,
            10_000_000,
            0,
            &format,
            None,
        )?;
        let capture: IAudioCaptureClient = client.GetService()?;
        let mut file = File::create(path)?;
        file.write_all(&header(0))?;
        client.Start()?;
        crate::xlog_info!("recording the title's audio to {}", path.display());

        let (mut written, mut first) = (0u64, None);
        let mut patched = Instant::now();
        loop {
            std::thread::sleep(Duration::from_millis(10));
            let ended = WaitForSingleObject(target, 0) == WAIT_OBJECT_0;
            while capture.GetNextPacketSize()? > 0 {
                let (mut data, mut frames, mut flags, mut qpc) = (std::ptr::null_mut(), 0, 0, 0);
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc))?;
                let at = (qpc - *first.get_or_insert(qpc)) * u64::from(RATE) / 10_000_000;
                if at > written + u64::from(RATE / 500) {
                    crate::xlog_info!(
                        "audio capture: nothing played for {} ms",
                        (at - written) * 1000 / u64::from(RATE)
                    );
                    let silence = vec![0u8; (at - written) as usize * FRAME_BYTES];
                    file.write_all(&silence)?;
                    written = at;
                }
                let bytes = frames as usize * FRAME_BYTES;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
                    file.write_all(&vec![0u8; bytes])?;
                } else {
                    file.write_all(std::slice::from_raw_parts(data, bytes))?;
                }
                if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 && written > 0 {
                    crate::xlog_info!("audio capture: discontinuity");
                }
                written += u64::from(frames);
                capture.ReleaseBuffer(frames)?;
            }
            if ended || patched.elapsed() >= Duration::from_secs(1) {
                file.seek(SeekFrom::Start(0))?;
                file.write_all(&header(written))?;
                file.seek(SeekFrom::End(0))?;
                patched = Instant::now();
            }
            if ended {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wav_header_sizes_match_pcm_stereo() {
        let h = header(48_000);
        assert_eq!(&h[0..4], b"RIFF");
        assert_eq!(&h[8..16], b"WAVEfmt ");
        assert_eq!(u16::from_le_bytes([h[22], h[23]]), 2);
        assert_eq!(u32::from_le_bytes([h[24], h[25], h[26], h[27]]), RATE);
        assert_eq!(u32::from_le_bytes([h[40], h[41], h[42], h[43]]), 48_000 * FRAME_BYTES as u32);
    }

    #[test]
    fn child_requires_target_pid_env() {
        let had = std::env::var_os(TARGET);
        std::env::remove_var(TARGET);
        assert!(!child());
        match had {
            Some(v) => std::env::set_var(TARGET, v),
            None => std::env::remove_var(TARGET),
        }
    }
}
