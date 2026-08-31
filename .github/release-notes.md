## Distribution notes

Packages in this release support the API inference profile only. Local mode requires a source
checkout until a packaged Local runtime is provided.

- The macOS app is ad-hoc signed, not Developer ID signed, and not notarized. Gatekeeper may
  require manual confirmation before the first launch.
- Windows artifacts are not Authenticode signed. Microsoft Defender SmartScreen may require
  manual confirmation before installation or launch.
- Validate each download with `SHA256SUMS` and GitHub artifact provenance before installing it.

