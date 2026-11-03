use std::mem::size_of;
use std::time::Duration;
use windows::{core::PSTR, Win32::Media::Audio::*};

const HEADER_SIZE: u32 = size_of::<WAVEHDR>() as u32;

struct Output {
    device: HWAVEOUT,
    header: Box<WAVEHDR>,
    _samples: Vec<i16>,
    prepared: bool,
}

// WinMM permits calls on different threads. The driver sees only the pinned
// heap allocations, and the owning task serializes all calls on the device.
unsafe impl Send for Output {}

fn checked(result: u32) -> Result<(), String> {
    if result == 0 {
        Ok(())
    } else {
        Err(format!("Windows speech playback failed (WinMM {result})."))
    }
}

impl Output {
    fn start(wav: &[u8]) -> Result<Self, String> {
        let mut reader =
            hound::WavReader::new(wav).map_err(|e| format!("Invalid speech WAV: {e}"))?;
        let spec = reader.spec();
        if spec.sample_format != hound::SampleFormat::Int || spec.bits_per_sample != 16 {
            return Err("Speech playback requires a 16-bit PCM WAV.".into());
        }
        let mut samples = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Invalid speech samples: {e}"))?;
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
        let header = Box::new(WAVEHDR {
            lpData: PSTR(samples.as_mut_ptr().cast()),
            dwBufferLength: u32::try_from(std::mem::size_of_val(samples.as_slice()))
                .map_err(|_| "Speech WAV is too large.")?,
            ..Default::default()
        });
        let mut device = HWAVEOUT::default();
        unsafe {
            checked(waveOutOpen(
                Some(&mut device),
                WAVE_MAPPER,
                &format,
                None,
                None,
                CALLBACK_NULL,
            ))?;
            let mut output = Self {
                device,
                header,
                _samples: samples,
                prepared: false,
            };
            checked(waveOutPrepareHeader(
                device,
                &raw mut *output.header,
                HEADER_SIZE,
            ))?;
            output.prepared = true;
            checked(waveOutWrite(device, &raw mut *output.header, HEADER_SIZE))?;
            Ok(output)
        }
    }

    fn finished(&mut self) -> Result<bool, String> {
        let status =
            unsafe { waveOutUnprepareHeader(self.device, &raw mut *self.header, HEADER_SIZE) };
        if status == WAVERR_STILLPLAYING {
            return Ok(false);
        }
        checked(status)?;
        self.prepared = false;
        Ok(true)
    }
}

impl Drop for Output {
    fn drop(&mut self) {
        unsafe {
            if self.prepared {
                // Reset returns queued buffers to the caller before storage is released.
                waveOutReset(self.device);
                waveOutUnprepareHeader(self.device, &raw mut *self.header, HEADER_SIZE);
            }
            waveOutClose(self.device);
        }
    }
}

pub(super) async fn play(wav: Vec<u8>) -> Result<(), String> {
    let mut output = Output::start(&wav)?;
    drop(wav);
    while !output.finished()? {
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    Ok(())
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
    #[ignore = "requires a default Windows audio output"]
    async fn silent_playback_completes_and_can_be_cancelled() {
        use std::future::Future;
        let mut long = Box::pin(play(silence(16000 * 20)));
        std::future::poll_fn(|cx| {
            if let std::task::Poll::Ready(result) = long.as_mut().poll(cx) {
                panic!("Unexpected immediate playback result: {result:?}");
            }
            std::task::Poll::Ready(())
        })
        .await;
        drop(long);
        tokio::time::timeout(Duration::from_secs(3), play(silence(1600)))
            .await
            .unwrap()
            .unwrap();
    }
}
