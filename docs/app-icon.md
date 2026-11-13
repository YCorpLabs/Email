# App icon

The app icon is a white porcelain envelope on a black tile. `assets/email-icon.png` is the shared approved artwork, normalized to 1024×1024 without changing its composition. Linux DEB and Arch packages install this asset.

`scripts/generate-macos-icon.swift` imports the artwork instead of drawing the former cream icon. Both Mac packaging paths call it. Mac export uses an 824×824 tile centered at (100, 100) on a transparent 1024×1024 canvas with rounded corner masking. This replaces the opaque full-canvas square that the preview image produced, which looked wrong in Cmd-Tab. The envelope artwork itself is unchanged.

Mac iconset exports contain 16, 32, 64, 128, 256, 512 and 1024-pixel representations.
