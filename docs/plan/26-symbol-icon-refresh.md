# 26 - Symbol Icon Refresh

**Status: Implemented.** The placeholder application artwork and runtime status dots now use the
selected five-bar V-shaped waveform. Automated tests, macOS release/bundle creation, bundled icon
inspection, and Windows-target compilation pass. Native menu-bar and notification-area appearance
still require interactive macOS and Windows smoke checks.

## Goal

Give ViberWhisper one compact, recognizable visual identity across its packaged application icon,
the macOS menu bar, and the Windows notification area. Replace the solid-blue placeholder bundle
assets and the generated gray/red circles with a simple voice-input symbol that remains legible at
small status-icon sizes.

## Scope

1. Replace the existing 32, 128, and 256 px application PNGs with a coordinated symbol icon.
2. Replace the runtime-generated circle with idle and recording variants of the same symbol.
3. Preserve all existing status-icon interactions: left click toggles recording, right click opens
   the menu, and Exit remains available.
4. Preserve the existing status meanings: neutral/brand color means idle and red means recording.
5. Keep macOS as a menu-bar-only app (`LSUIElement=true`) and retain the Windows notification-area
   icon.

The feature does not add a window, Dock icon, configuration option, animation, or new recording
state.

## Visual Direction

Use the selected option 2 concept: exactly five separate vertical bars with fully rounded ends. The
bars descend symmetrically toward the center so their lower endpoints form a `V`, while their upper
endpoints read as a restrained voice waveform. The mark has no enclosing `V`, microphone, speech
bubble, text, or other component. Broad strokes and even spacing keep it legible at 16–32 px.

- **Application icon:** the five-bar mark in white on a clean rounded-square blue-to-violet brand
  field. Avoid photorealism, fine detail, shadows, lettering, and decorative borders.
- **Idle status icon:** the isolated five-bar mark on transparency. On macOS it is presented as an
  AppKit template image so the system supplies the correct light/dark menu-bar color; Windows uses
  the supplied neutral brand-color pixels.
- **Recording status icon:** the same five bars in red on transparency. On macOS it is explicitly
  non-template so recording remains visibly red; Windows uses the same red asset.

The user-selected generated option 2 is the visual reference, not the production asset. Reconstruct
its simple geometry as a precise vector source so all five bars are centered, symmetric, evenly
spaced, and consistently rounded. Final exports must have clean transparency, consistent padding,
and crisp results at every committed size.

## Technical Approach

### 1. Generate and normalize the source artwork

Use the selected option 2 `logo-brand` preview as the design reference. Reconstruct the five-bar
symbol as a project-owned vector master rather than cropping or shipping the AI-generated concept
board. This keeps the geometry deterministic and produces clean transparency without chroma-key
artifacts.

Export two related deliverables from that one vector geometry:

- a full application icon master for bundle sizes;
- an isolated status-symbol master for neutral and recording variants.

Downscale from those masters with high-quality resampling, then inspect the actual 32 px output. If
the generated geometry does not remain crisp, simplify and regularize the silhouette before export
instead of adding special cases to runtime drawing code.

### 2. Commit explicit runtime icon assets

Add transparent idle and recording PNGs under `assets/` and decode their embedded bytes when
constructing `TrayManager`. The application should not depend on the current working directory or
external files at runtime. Add the PNG decoder as a direct dependency because production code uses
it directly even though it is currently present only transitively.

`TrayManager::new()` will fail with a descriptive error if a committed icon cannot be decoded;
runtime state updates retain the existing log-and-continue behavior for native icon update errors.
Remove the procedural filled-circle generator after the embedded assets are in use.

### 3. Apply platform-appropriate status rendering

Build the initial tray icon with macOS template rendering enabled for the idle symbol. When
`set_recording` changes state, update the icon and its template flag together: template for idle,
non-template for the red recording variant. On non-macOS targets the template flag is ignored by
`tray-icon`, so Windows receives the explicit asset colors.

Tooltip text, menu status text, click classification, debounce, event pumping, and lifecycle effects
remain unchanged.

### 4. Replace bundle artwork

Export the approved application icon at the three sizes already referenced by
`[package.metadata.bundle]`: 32×32, 128×128, and 256×256. Keep those paths stable so packaging and
release workflows need no structural change.

## Files and Modules

| Path | Planned change |
| --- | --- |
| `assets/icon-source.svg` | Add the precise five-bar vector source used for all exports |
| `assets/icon-32x32.png` | Replace placeholder bundle artwork |
| `assets/icon-128x128.png` | Replace placeholder bundle artwork |
| `assets/icon-256x256.png` | Replace placeholder bundle artwork |
| `assets/status-idle.png` | Add transparent idle status-symbol asset |
| `assets/status-recording.png` | Add transparent red recording status-symbol asset |
| `Cargo.toml` / `Cargo.lock` | Declare the PNG decoder used by runtime asset loading |
| `src/input/tray.rs` | Decode embedded assets, remove circle generation, and manage macOS template state |
| `README.md` | Describe the symbolic idle/recording indicator without changing the workflow |
| `docs/architecture/input.md` | Document embedded icon assets and platform rendering policy |
| `changelog` | Record the user-visible icon refresh |

`assets/Info.plist.ext` and the release workflows remain unchanged because the menu-bar-only policy
and bundle asset paths are preserved.

Implementation matched the planned scope. `Cargo.lock` did not need a content change because
`png 0.17` was already locked as a transitive dependency; `Cargo.toml` now declares it directly for
the production decoder. Native macOS and Windows visual smoke checks remain the only unperformed
validation and are called out in the status above and in the pull request.

After implementation, the user simplified the application icon background from the planned
blue-to-violet gradient to a Ghostty-inspired solid gray-blue `#282c34`. The five-bar geometry and
all status icons remain unchanged.

## Implementation Order

1. Reconstruct the selected option 2 preview as a precise five-bar vector master and visually
   confirm that its lower endpoints form a clear `V` without reading as a flat equalizer.
2. Produce the three bundle PNGs plus idle/recording status PNGs and verify their dimensions,
   color modes, transparency, padding, and 32 px legibility.
3. Add focused failing tests for embedded PNG decoding and expected dimensions/alpha coverage.
4. Implement embedded PNG loading and switch `TrayManager` to the new assets.
5. Apply macOS template rendering for idle and explicit red rendering for recording without
   changing tray action behavior.
6. Remove the procedural circle code and update current-truth documentation.
7. Run automated checks, build the macOS bundle, inspect its generated icon resources, and perform
   platform UI checks where available.

## Test Strategy

### Automated

- Decode every embedded status PNG and assert its exact dimensions and RGBA output.
- Assert the status assets contain meaningful transparent and opaque pixels so an accidentally flat
  or opaque replacement fails tests.
- Retain the existing click debounce and action classification tests unchanged to prove the visual
  replacement does not alter recording controls.
- Run `cargo fmt --check`, `cargo test`, `cargo clippy`, and `cargo build --release`.
- Run `cargo bundle --release` on macOS and confirm the produced bundle contains the replacement
  icon resources.

### Visual and Platform Checks

- Inspect 32 px exports at native scale, not only enlarged.
- On macOS, verify idle visibility in both light and dark menu bars, red recording state, tooltip,
  left-click toggle, right-click menu, and absence of a Dock icon.
- On Windows, verify the symbol remains recognizable in the notification area at normal DPI and
  high DPI, including idle/red state changes and the existing context menu.

Windows UI verification may require a Windows runner or maintainer smoke test; cross-compilation or
CI compilation alone cannot establish native notification-area appearance.

## Documentation Impact

- Update `README.md` because the visible idle/recording indicator changes from colored dots to a
  branded symbol, while interaction instructions remain the same.
- Update `docs/architecture/input.md` because `TrayManager` changes from procedural circle drawing
  to embedded PNG decoding and gains explicit macOS template-image behavior.
- Add a concise `changelog` entry for the visible asset refresh.
- Add this plan to `docs/README.md` during planning so it is discoverable; mark it Done only after
  implementation and validation.
- Do not modify `config.example.json`, configuration docs, platform text-injection docs, or the
  historical packaging plan because no configuration, typing behavior, packaging path, or past
  decision changes.

## Acceptance Criteria

- The three placeholder blue bundle icons are replaced by the selected five-bar V-shaped waveform.
- Every production variant uses the same symmetric five-bar geometry, with no enclosing `V`,
  microphone, speech bubble, or additional decoration.
- macOS shows the idle symbol with native template tinting and the recording symbol in red.
- Windows shows the same recognizable symbol in neutral/brand color when idle and red while
  recording.
- Both runtime icons have transparent backgrounds and remain legible at their committed native
  size.
- Existing tray clicks, hotkeys, menu status, Exit, tooltip behavior, and recording lifecycle are
  unchanged.
- Packaged macOS builds remain menu-bar-only with no Dock icon.
- Automated checks and available platform smoke tests pass, with any unperformed Windows visual
  verification stated explicitly in the PR.
