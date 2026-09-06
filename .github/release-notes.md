## Distribution notes

Packages in this release use configurable OpenAI-compatible endpoints for transcription and
optional text post-processing, including user-managed services on localhost.

- The macOS app is ad-hoc signed, not Developer ID signed, and not notarized. Gatekeeper may
  require manual confirmation before the first launch.
- Windows artifacts are not Authenticode signed. Microsoft Defender SmartScreen may require
  manual confirmation before installation or launch.
- Validate each download with `SHA256SUMS` and GitHub artifact provenance before installing it.
