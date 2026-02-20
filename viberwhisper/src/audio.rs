use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{WavSpec, WavWriter};

pub struct AudioRecorder {
    recording: Arc<Mutex<bool>>,
    buffer: Arc<Mutex<Vec<i16>>>,
    stream: Option<cpal::Stream>,
    sample_count: Arc<AtomicUsize>,
    gain: f32,
    sample_rate: u32,
}

impl AudioRecorder {
    pub fn new(gain: f32) -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();

        println!("[Audio] 默认输入设备: {}",
            host.default_input_device()
                .and_then(|d| d.name().ok())
                .unwrap_or_else(|| "(未找到)".to_string())
        );

        println!("[Audio] 所有可用输入设备:");
        match host.input_devices() {
            Ok(devices) => {
                for (i, device) in devices.enumerate() {
                    let name = device.name().unwrap_or_else(|_| "(未知)".to_string());
                    println!("  [{}] {}", i, name);
                }
            }
            Err(e) => eprintln!("[Audio] 枚举设备失败: {}", e),
        }

        println!("[Audio] 麦克风增益: {}x", gain);

        Ok(AudioRecorder {
            recording: Arc::new(Mutex::new(false)),
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
            sample_count: Arc::new(AtomicUsize::new(0)),
            gain,
            sample_rate: 44100,
        })
    }

    pub fn start_recording(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        if *self.recording.lock().unwrap() {
            println!("DEBUG: Already recording, ignoring duplicate start request");
            return Ok(());
        }

        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or("No input device available")?;
        let config = device.default_input_config()?;

        let sample_rate = config.sample_rate().0;
        let channels = config.channels() as usize;
        let sample_format = config.sample_format();

        println!("[Audio] 设备采样率: {} Hz，声道数: {}，格式: {:?}", sample_rate, channels, sample_format);

        self.sample_rate = sample_rate;

        let recording = Arc::clone(&self.recording);
        let buffer = Arc::clone(&self.buffer);
        let sample_count = Arc::clone(&self.sample_count);
        let gain = self.gain;

        buffer.lock().unwrap().clear();
        sample_count.store(0, Ordering::Relaxed);

        // 先设为 true，再启动流，避免初始帧被丢弃
        *self.recording.lock().unwrap() = true;

        let stream = match sample_format {
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if *recording.lock().unwrap() {
                        let mono: Vec<i16> = data
                            .chunks(channels)
                            .map(|ch| {
                                let avg = ch.iter().map(|&s| s as f32).sum::<f32>() / channels as f32;
                                (avg * gain).clamp(i16::MIN as f32, i16::MAX as f32) as i16
                            })
                            .collect();
                        let len = mono.len();
                        buffer.lock().unwrap().extend_from_slice(&mono);
                        let total = sample_count.fetch_add(len, Ordering::Relaxed) + len;
                        if total % (sample_rate as usize / 2) < len {
                            println!("[DEBUG] 已录制 {} 帧 (~{}s)", total, total / sample_rate as usize);
                        }
                    }
                },
                move |err| eprintln!("Stream error: {}", err),
                None,
            )?,
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if *recording.lock().unwrap() {
                        let mono: Vec<i16> = data
                            .chunks(channels)
                            .map(|ch| {
                                let avg = ch.iter().sum::<f32>() / channels as f32;
                                (avg * gain).clamp(-1.0, 1.0) * i16::MAX as f32
                            })
                            .map(|s| s as i16)
                            .collect();
                        let len = mono.len();
                        buffer.lock().unwrap().extend_from_slice(&mono);
                        let total = sample_count.fetch_add(len, Ordering::Relaxed) + len;
                        if total % (sample_rate as usize / 2) < len {
                            println!("[DEBUG] 已录制 {} 帧 (~{}s)", total, total / sample_rate as usize);
                        }
                    }
                },
                move |err| eprintln!("Stream error: {}", err),
                None,
            )?,
            _ => {
                *self.recording.lock().unwrap() = false;
                return Err("Unsupported sample format".into());
            }
        };

        stream.play()?;
        self.stream = Some(stream);

        println!("Recording started...");
        Ok(())
    }

    pub fn stop_recording(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        // Check if actually recording
        if !*self.recording.lock().unwrap() {
            println!("DEBUG: Not recording, ignoring stop request");
            return Err("Not currently recording".into());
        }

        println!("DEBUG: Stopping recording...");
        *self.recording.lock().unwrap() = false;

        // Wait a bit for pending callbacks to complete
        thread::sleep(Duration::from_millis(200));

        drop(self.stream.take());
        println!("DEBUG: Stream stopped");

        let buffer = self.buffer.lock().unwrap();
        println!("DEBUG: Buffer size: {} samples", buffer.len());

        if buffer.is_empty() {
            return Err("No audio data recorded".into());
        }

        // Save to ./tmp folder
        let mut path = PathBuf::from("./tmp");

        // Create directory if it doesn't exist
        if let Err(e) = std::fs::create_dir_all(&path) {
            eprintln!("WARNING: Failed to create directory: {}", e);
        }

        // Generate filename with timestamp
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs();
        path.push(format!("recording_{}.wav", timestamp));

        let filename = path.to_string_lossy().to_string();
        println!("DEBUG: Saving to: {}", filename);

        // Write WAV file
        let spec = WavSpec {
            channels: 1,
            sample_rate: self.sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut writer = match WavWriter::create(&path, spec) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("ERROR: Failed to create WAV writer: {}", e);
                return Err(e.into());
            }
        };

        for (i, &sample) in buffer.iter().enumerate() {
            if let Err(e) = writer.write_sample(sample) {
                eprintln!("ERROR: Failed to write sample {}: {}", i, e);
                return Err(e.into());
            }
        }

        if let Err(e) = writer.finalize() {
            eprintln!("ERROR: Failed to finalize WAV file: {}", e);
            return Err(e.into());
        }

        println!("Recording saved to: {}", filename);

        self.cleanup_old_recordings("./tmp", 10);

        Ok(filename)
    }

    fn cleanup_old_recordings(&self, dir: &str, keep: usize) {
        let mut entries: Vec<_> = match std::fs::read_dir(dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .filter(|e| {
                    e.path()
                        .extension()
                        .and_then(|ext| ext.to_str())
                        .map(|ext| ext == "wav")
                        .unwrap_or(false)
                })
                .collect(),
            Err(_) => return,
        };

        if entries.len() <= keep {
            return;
        }

        entries.sort_by_key(|e| e.metadata().and_then(|m| m.modified()).ok());

        for entry in &entries[..entries.len() - keep] {
            if let Err(e) = std::fs::remove_file(entry.path()) {
                eprintln!("[Audio] 清理旧录音失败 {:?}: {}", entry.path(), e);
            } else {
                println!("[Audio] 已删除旧录音: {:?}", entry.path());
            }
        }
    }

    pub fn is_recording(&self) -> bool {
        *self.recording.lock().unwrap()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_audio_recorder_creation() {
        let recorder = AudioRecorder::new(1.0);
        assert!(recorder.is_ok());
    }

    #[test]
    fn test_recorder_not_recording_initially() {
        let recorder = AudioRecorder::new(1.0).unwrap();
        assert!(!recorder.is_recording());
    }
}
