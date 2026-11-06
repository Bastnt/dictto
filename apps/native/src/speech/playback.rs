use std::collections::VecDeque;
use std::io::Read;
use std::mem::size_of;
use std::path::Path;
use std::time::Duration;
use windows::{core::PSTR, Win32::Media::Audio::*};

const HEADER_SIZE: u32 = size_of::<WAVEHDR>() as u32;
const BLOCK_SAMPLES: usize = 8192;

struct Buffer {
    header: Box<WAVEHDR>,
    _samples: Vec<i16>,
}

struct Output {
    device: HWAVEOUT,
    queue: VecDeque<Buffer>,
}

// WinMM can be called from different threads. The owning task serializes calls
// and retains each buffer's heap allocations until the driver releases them.
unsafe impl Send for Output {}

fn checked(result: u32) -> Result<(), String> {
    if result == 0 {
        Ok(())
    } else {
        Err(format!("Windows speech playback failed (WinMM {result})."))
    }
}

impl Output {
    fn open(spec: hound::WavSpec, device_id: u32) -> Result<Self, String> {
        if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
            return Err("Speech playback requires a 16-bit PCM WAV.".into());
        }
        let align = spec
            .channels
            .checked_mul(2)
            .filter(|v| *v > 0)
            .ok_or("Invalid speech channel count.")?;
        let format = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_PCM as u16,
            nChannels: spec.channels,
            nSamplesPerSec: spec.sample_rate,
            nAvgBytesPerSec: spec
                .sample_rate
                .checked_mul(align.into())
                .ok_or("Invalid speech sample rate.")?,
            nBlockAlign: align,
            wBitsPerSample: 16,
            cbSize: 0,
        };
        let mut device = HWAVEOUT::default();
        unsafe {
            checked(waveOutOpen(
                Some(&mut device),
                device_id,
                &format,
                None,
                None,
                CALLBACK_NULL,
            ))?;
        }
        Ok(Self {
            device,
            queue: VecDeque::with_capacity(2),
        })
    }

    fn submit(&mut self, mut samples: Vec<i16>) -> Result<(), String> {
        let mut header = Box::new(WAVEHDR {
            lpData: PSTR(samples.as_mut_ptr().cast()),
            dwBufferLength: std::mem::size_of_val(samples.as_slice()) as u32,
            ..Default::default()
        });
        unsafe {
            checked(waveOutPrepareHeader(
                self.device,
                &raw mut *header,
                HEADER_SIZE,
            ))?;
            self.queue.push_back(Buffer {
                header,
                _samples: samples,
            });
            let buffer = self.queue.back_mut().unwrap();
            checked(waveOutWrite(
                self.device,
                &raw mut *buffer.header,
                HEADER_SIZE,
            ))
        }
    }

    fn release_finished(&mut self) -> Result<bool, String> {
        let Some(buffer) = self.queue.front_mut() else {
            return Ok(false);
        };
        let status =
            unsafe { waveOutUnprepareHeader(self.device, &raw mut *buffer.header, HEADER_SIZE) };
        if status == WAVERR_STILLPLAYING {
            return Ok(false);
        }
        checked(status)?;
        self.queue.pop_front();
        Ok(true)
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        unsafe {
            if !self.queue.is_empty() {
                waveOutReset(self.device);
            }
            for mut buffer in self.queue.drain(..) {
                let status =
                    waveOutUnprepareHeader(self.device, &raw mut *buffer.header, HEADER_SIZE);
                if status != 0 {
                    // The driver may still hold pointers if it refused to release this buffer.
                    log::error!(
                        "Windows did not release a speech playback buffer (WinMM {status})."
                    );
                    std::mem::forget(buffer);
                }
            }
            waveOutClose(self.device);
        }
    }
}

async fn play_reader<R: Read>(
    mut reader: hound::WavReader<R>,
    device_id: u32,
    mut check_route: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    let spec = reader.spec();
    let mut output = Output::open(spec, device_id)?;
    let channels = usize::from(spec.channels);
    let block = (BLOCK_SAMPLES / channels).max(1) * channels;
    let mut samples = reader.samples::<i16>();
    let mut ended = false;
    loop {
        check_route()?;
        while output.queue.len() < 2 && !ended {
            let part = samples
                .by_ref()
                .take(block)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("Invalid speech samples: {e}"))?;
            ended = part.len() < block;
            if !part.is_empty() {
                output.submit(part)?;
            }
        }
        if output.queue.is_empty() {
            return Ok(());
        }
        if !output.release_finished()? {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }
}

pub(super) async fn play(wav: Vec<u8>) -> Result<(), String> {
    let reader =
        hound::WavReader::new(wav.as_slice()).map_err(|e| format!("Invalid speech WAV: {e}"))?;
    play_reader(reader, WAVE_MAPPER, || Ok(())).await
}

pub(super) async fn play_file(path: &Path) -> Result<(), String> {
    let reader =
        hound::WavReader::open(path).map_err(|e| format!("Cannot read cached speech WAV: {e}"))?;
    play_reader(reader, WAVE_MAPPER, || Ok(())).await
}

pub(super) fn device_id_for_endpoint(endpoint: &str) -> Result<u32, String> {
    // mmddk.h: query the Core Audio endpoint ID for each WinMM device.
    const QUERY_ID: u32 = 0x0811;
    const QUERY_ID_SIZE: u32 = 0x0812;
    unsafe {
        for id in 0..waveOutGetNumDevs() {
            let device = HWAVEOUT(id as usize as *mut _);
            let mut bytes = 0u32;
            if waveOutMessage(
                Some(device),
                QUERY_ID_SIZE,
                &mut bytes as *mut u32 as usize,
                0,
            ) != 0
                || bytes < 2
                || bytes > 65536
                || bytes % 2 != 0
            {
                continue;
            }
            let mut name = vec![0u16; bytes as usize / 2];
            if waveOutMessage(
                Some(device),
                QUERY_ID,
                name.as_mut_ptr() as usize,
                bytes as usize,
            ) == 0
                && String::from_utf16_lossy(
                    &name[..name.iter().position(|c| *c == 0).unwrap_or(name.len())],
                )
                .eq_ignore_ascii_case(endpoint)
            {
                return Ok(id);
            }
        }
    }
    Err("VB-CABLE's playback endpoint is not available to Windows audio playback.".into())
}

pub(super) async fn play_to<R: Read>(
    reader: hound::WavReader<R>,
    device_id: u32,
    check_route: impl FnMut() -> Result<(), String>,
) -> Result<(), String> {
    play_reader(reader, device_id, check_route).await
}

#[cfg(test)]
mod tests {
    use super::*;
    fn silence(frames: usize) -> Vec<u8> {
        let mut data = std::io::Cursor::new(Vec::new());
        let mut writer = hound::WavWriter::new(
            &mut data,
            hound::WavSpec {
                sample_rate: 16000,
                channels: 1,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )
        .unwrap();
        for _ in 0..frames {
            writer.write_sample(0_i16).unwrap();
        }
        writer.finalize().unwrap();
        data.into_inner()
    }

    #[tokio::test]
    async fn invalid_cache_file_reports_an_error_before_opening_a_device() {
        let path =
            std::env::temp_dir().join(format!("dictto-invalid-audio-{}.wav", std::process::id()));
        std::fs::write(&path, b"Invalid WAV").unwrap();
        let result = play_file(&path).await;
        std::fs::remove_file(&path).unwrap();
        assert!(result
            .unwrap_err()
            .contains("Cannot read cached speech WAV"));
    }

    #[tokio::test]
    #[ignore = "requires a default Windows audio output"]
    async fn silent_playback_completes_and_can_be_cancelled() {
        use std::future::Future;
        let file =
            std::env::temp_dir().join(format!("dictto-silent-playback-{}.wav", std::process::id()));
        std::fs::write(&file, silence(16000 * 20)).unwrap();
        let mut long = Box::pin(play_file(&file));
        std::future::poll_fn(|cx| {
            if let std::task::Poll::Ready(result) = long.as_mut().poll(cx) {
                panic!("Unexpected immediate playback result: {result:?}");
            }
            std::task::Poll::Ready(())
        })
        .await;
        drop(long);
        std::fs::write(&file, silence(BLOCK_SAMPLES * 3)).unwrap();
        tokio::time::timeout(Duration::from_secs(4), play_file(&file))
            .await
            .unwrap()
            .unwrap();
        std::fs::remove_file(file).unwrap();
        tokio::time::timeout(Duration::from_secs(3), play(silence(1600)))
            .await
            .unwrap()
            .unwrap();
    }
}
