# VAD regression audio

`speech.wav` is the first three seconds (48,000 mono signed 16-bit samples at
16 kHz) of the Silero VAD v6.2 test recording:
https://github.com/snakers4/silero-vad/blob/v6.2/tests/data/test.wav

The upstream repository distributes this fixture under its MIT license, reproduced
in `assets/vad/THIRD-PARTY-NOTICES.txt`. Only the duration/WAV header was changed.
SHA-256: `70959e0d77f387a573c02c8e5d668c0346046b23fe18b8d8023d56668fb85112`.

Noise is generated deterministically in the tests. This fixture checks that the
bundled model can detect speech; it is not a representative Chinese recognition
dataset and does not establish transcription accuracy or false-rejection rates.
