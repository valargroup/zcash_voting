# keystone-qr

CLI tool for displaying and scanning Keystone-style animated QR codes in the terminal.

Uses [URKit](https://github.com/BlockchainCommons/URKit) fountain codes to split large payloads across multiple QR frames that cycle in the terminal — the same protocol Keystone hardware wallets use.

## Build

```
swift build
```

## Usage

### Static QR

```
swift run keystone-qr display "any string here"
echo "piped input" | swift run keystone-qr display
```

### Animated (fountain-coded)

For payloads that need multi-frame encoding. Input is hex, `--ur-type` sets the UR type tag:

```
swift run keystone-qr display --ur-type bytes "deadbeefcafebabe..."
swift run keystone-qr display --ur-type zcash-pczt --fragment-len 100 --interval 150 "$(cat pczt.hex)"
```

- `--ur-type` — UR type identifier (triggers animated mode)
- `--fragment-len` — max bytes per fountain fragment (default: 200)
- `--interval` — milliseconds between frames (default: 200)

Ctrl-C to stop.

### Scan (planned)

```
swift run keystone-qr scan --camera   # read from webcam
swift run keystone-qr scan --screen   # read from screen region
```

## Architecture

```
Sources/keystone-qr/
├── KeystoneQR.swift        # Entry point, subcommand routing
├── DisplayCommand.swift    # Static + animated QR display
├── ScanCommand.swift       # Camera/screen scanning (stub)
└── TerminalQR.swift        # CIFilter QR generation → Unicode half-block rendering
```

**TerminalQR** generates QR modules via `CIQRCodeGenerator`, reads the pixel bitmap, and renders using `▀` half-block characters with ANSI colors — two pixel rows per terminal line. Includes a 4-module quiet zone for scannability.

**DisplayCommand** handles two modes: static (single QR from any string) and animated (UR fountain encoder cycling frames). Animated mode clears the terminal and redraws each frame.

## Dependencies

- [URKit](https://github.com/BlockchainCommons/URKit) 15.x — UR encoding with fountain codes
- [swift-argument-parser](https://github.com/apple/swift-argument-parser) — CLI framework
- CoreImage (system) — QR code generation
