# Filink

Fast file, folder and text transfer between an Android phone and a Windows PC on the same Wi-Fi.

- **PC app** (`pc/`): Rust + egui, a single ~3 MB exe with no runtime.
- **Android app** (`android/`): Kotlin, framework APIs only (no AndroidX); release APK ~125 KB.
- **Wire protocol**: [`PROTOCOL.md`](PROTOCOL.md).

## Features

- Automatic discovery over UDP broadcast; one-time pairing approved on the PC with a matching 6-digit code.
- Mutual HMAC-SHA256 authentication on every connection; only paired devices can transfer.
- Files and whole folders in both directions, with progress, speed and cancel.
- Text and links in both directions, delivered straight to the clipboard.
- Android share-sheet target ("Share → DroidTransfer").
- Optional background receiving on the phone (foreground service).

Payloads are not encrypted; the apps rely on the Wi-Fi link encryption.

## Build

### PC (Windows)

```sh
cd pc
cargo build --release
# target/release/droidtransfer.exe
cargo test
```

When Windows Firewall asks on the first run, allow access on **Private networks**.

### Android

Requires JDK 17+ and an Android SDK with `platforms;android-35` and `build-tools;35.0.0`.
Create `android/local.properties` with `sdk.dir=<path to your SDK>`, then:

```sh
cd android
./gradlew :app:testDebugUnitTest :app:assembleRelease
# app/build/outputs/apk/release/app-release.apk (signed with the debug key)
```

## First use

1. Install the APK on the phone and start `droidtransfer.exe` on the PC (same Wi-Fi).
2. On the phone, tap **Pair** next to the PC.
3. Check that both screens show the same code, then click **Accept** on the PC.
4. Optionally enable **Receive in background** on the phone.

Received files go to `Downloads\DroidTransfer` on the PC (configurable) and `Download/DroidTransfer` on the phone.
