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
        *self.recording.lock().unwrap() = false;
        drop(self.stream.take());

        let buffer = self.buffer.lock().unwrap();
        if buffer.is_empty() {
            return Err("No audio data recorded".into());
        }

        // Generate filename with timestamp
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)?
            .as_secs();
        let filename = format!("recording_{}.wav", timestamp);

        // Write WAV file
        let spec = WavSpec {
            channels: 1,
            sample_rate: 16000,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };

        let mut writer = WavWriter::create(&filename, spec)?;
        for &sample in buffer.iter() {
            writer.write_sample(sample)?;
        }
        writer.finalize()?;

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
