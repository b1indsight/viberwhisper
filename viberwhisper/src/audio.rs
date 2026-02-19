use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use hound::{WavSpec, WavWriter};

pub struct AudioRecorder {
    recording: Arc<Mutex<bool>>,
    buffer: Arc<Mutex<Vec<i16>>>,
    stream: Option<cpal::Stream>,
}

impl AudioRecorder {
    pub fn new() -> Result<Self, Box<dyn std::error::Error>> {
        Ok(AudioRecorder {
            recording: Arc::new(Mutex::new(false)),
            buffer: Arc::new(Mutex::new(Vec::new())),
            stream: None,
        })
    }

    pub fn start_recording(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_input_device()
            .ok_or("No input device available")?;
        let config = device.default_input_config()?;

        let recording = Arc::clone(&self.recording);
        let buffer = Arc::clone(&self.buffer);

        // Clear buffer
        buffer.lock().unwrap().clear();

        let stream = match config.sample_format() {
            cpal::SampleFormat::I16 => device.build_input_stream(
                &config.into(),
                move |data: &[i16], _: &cpal::InputCallbackInfo| {
                    if *recording.lock().unwrap() {
                        buffer.lock().unwrap().extend_from_slice(data);
                    }
                },
                move |err| eprintln!("Stream error: {}", err),
                None,
            )?,
            cpal::SampleFormat::F32 => device.build_input_stream(
                &config.into(),
                move |data: &[f32], _: &cpal::InputCallbackInfo| {
                    if *recording.lock().unwrap() {
                        let int_data: Vec<i16> = data
                            .iter()
                            .map(|&s| (s * i16::MAX as f32) as i16)
                            .collect();
                        buffer.lock().unwrap().extend_from_slice(&int_data);
                    }
                },
                move |err| eprintln!("Stream error: {}", err),
                None,
            )?,
            _ => return Err("Unsupported sample format".into()),
        };

        stream.play()?;
        *self.recording.lock().unwrap() = true;
        self.stream = Some(stream);

        println!("Recording started...");
        Ok(())
    }

    pub fn stop_recording(&mut self) -> Result<String, Box<dyn std::error::Error>> {
        println!("DEBUG: Stopping recording...");
        *self.recording.lock().unwrap() = false;
        drop(self.stream.take());
        println!("DEBUG: Stream stopped");

        let buffer = self.buffer.lock().unwrap();
        println!("DEBUG: Buffer size: {} samples", buffer.len());

        if buffer.is_empty() {
            return Err("No audio data recorded".into());
        }

        // Save to Documents/ViberWhisper folder
        let mut path = dirs::document_dir().unwrap_or_else(|| PathBuf::from("."));
        path.push("ViberWhisper");

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
            sample_rate: 16000,
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
        Ok(filename)
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
        let recorder = AudioRecorder::new();
        assert!(recorder.is_ok());
    }

    #[test]
    fn test_recorder_not_recording_initially() {
        let recorder = AudioRecorder::new().unwrap();
        assert!(!recorder.is_recording());
    }
}
