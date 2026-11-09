# TailscaleNotify

A lightweight, native desktop notification and quick-action overlay for **Tailscale Taildrop**.

Tailscale delivers files between devices seamlessly, but incoming transfers can easily go unnoticed. **TailscaleNotify** runs quietly in the system tray, monitors incoming Taildrop transfers in real-time, and presents a sleek, unobtrusive floating alert with file previews and quick actions.

---

## ✨ Features

- **Instant Taildrop Notifications**: Detects incoming files the moment they finish transferring and displays an overlay alert.
- **Rich In-App Previews**: Generates embedded previews for images (PNG, JPEG, WebP, GIF, SVG) up to 12 MB, with clean fallbacks for documents and binary files.
- **Smart Transfer Filtering**:
  - **macOS**: Checks `com.apple.quarantine` extended attributes so standard browser/app downloads in your `Downloads` directory are ignored and only Taildrop transfers trigger alerts.
  - **Windows**: Inspects NTFS Mark-of-the-Web (`:Zone.Identifier`) alternate data streams to filter out browser and web downloads.
  - **Linux**: Watches an isolated `~/TaildropInbox` folder populated by an integrated background receiver.
- **Non-Intrusive macOS Full-Screen Support**:
  - Leverages AppKit `NSPanel` (`FullScreenAuxiliary` & `CanJoinAllSpaces`) at `NSStatusWindowLevel` to float seamlessly over full-screen apps and multiple spaces without stealing keyboard focus or interrupting your work.
- **Safety First**:
  - **Restricted File Opener**: The webview frontend can only request opening the specific file verified and announced by the Rust backend—preventing arbitrary local path execution.
  - **Accidental Execution Prevention**: Dismiss with `Escape` or the close button; typing `Enter` while an alert pops up will never accidentally launch an untrusted incoming file.
- **System Tray Integration**:
  - Runs quietly as a background/menu bar accessory app.
  - Menu options to **Show Latest Alert**, toggle **Launch at Login**, and **Quit**.
- **Modern Acrylic / Liquid Glass UI**:
  - Designed with native macOS dark/light mode aesthetics, translucent glass backdrop blur, and smooth arrival animations.

---

## 🛠️ How It Works

| Platform | Incoming Location | Detection & Filtering Mechanism |
| :--- | :--- | :--- |
| **macOS** | `~/Downloads` | Monitors file creation & writes with debounced quiet periods (300ms). Checks extended attributes (`xattr -p com.apple.quarantine`) to exclude downloads originating from browsers, AirDrop, or messaging apps. |
| **Windows** | `Downloads` | Monitors file changes. Checks for `:Zone.Identifier` NTFS Alternate Data Streams (`HostUrl=` / `ReferrerUrl=`) to filter out browser downloads. |
| **Linux** | `~/TaildropInbox` | Automatically manages a background `tailscale file get --wait --loop --conflict=rename` process and watches the inbox directory. |

---

## 🚀 Getting Started

### Prerequisites

1. **[Tailscale](https://tailscale.com/)** installed and signed in on your machine.
   - *Linux users*: Make sure your user account has Tailscale operator privileges so the app can fetch incoming files without root:
     ```bash
     sudo tailscale set --operator=$USER
     ```
2. **[Node.js](https://nodejs.org/)** (v20+ or v22 LTS recommended) and `npm`.
3. **[Rust](https://www.rust-lang.org/)** (stable toolchain) and Cargo.
4. **Platform dependencies** for Tauri v2:
   - Refer to the [Tauri Prerequisites Guide](https://v2.tauri.app/start/prerequisites/) for your operating system.

---

### Installation & Development

1. **Clone the repository**:
   ```bash
   git clone https://github.com/kwincheah/tailscaleNotify.git
   cd tailscaleNotify
   ```

2. **Install frontend dependencies**:
   ```bash
   npm install
   ```

3. **Run in development mode**:
   ```bash
   npm run tauri dev
   ```
   This starts the Vite development server and launches the Tauri desktop application with hot-reloading.

---

### Building for Production

#### macOS
```bash
# Build standalone application bundle (.app)
npm run build:mac

# Or via standard Tauri CLI:
npx tauri build --bundles app
```
The resulting `.app` bundle will be located under `src-tauri/target/release/bundle/macos/`.

#### Windows
```bash
# Build installer (.exe / NSIS)
npx tauri build --bundles nsis
```
The installer will be placed under `src-tauri/target/release/bundle/nsis/`.

#### Linux
```bash
# Build package (deb / appimage)
npx tauri build
```

---

## ⌨️ Controls & Shortcuts

| Action | Control |
| :--- | :--- |
| **Open File** | Click **Open File** button |
| **Dismiss Alert** | Click **Dismiss** or press <kbd>Esc</kbd> |
| **Reopen Alert** | Click the tray icon and select **Show Latest Alert** |
| **Toggle Autostart** | Tray icon menu &rarr; **Launch at Login** |
| **Exit Application** | Tray icon menu &rarr; **Quit** |

---

## 📂 Project Structure

```text
tailscaleNotify/
├── index.html               # Floating notification UI with glassmorphic styling
├── src/
│   ├── main.ts              # Frontend event handling, previews, and window controls
│   └── styles.css           # Styling helpers and base styles
├── src-tauri/
│   ├── src/
│   │   ├── main.rs          # Application entrypoint
│   │   └── lib.rs           # Core watcher, platform-specific filters, tray menu & IPC
│   ├── Cargo.toml           # Rust dependencies (Tauri v2, notify, base64, autostart)
│   └── tauri.conf.json      # Tauri application configuration & overlay window settings
├── .github/workflows/
│   └── build.yml            # CI pipeline for macOS and Windows builds
└── package.json             # Scripts and frontend tooling (Vite, TypeScript, Tauri CLI)
```

---

## 🔒 Security & Privacy

- **No Remote Telemetry**: TailscaleNotify operates entirely on your local machine.
- **Scoped File Opening**: IPC commands only allow opening the exact path of the last detected Taildrop file verified by Rust.
- **Safe Hotkeys**: By design, pressing `Enter` or spacebar will never trigger file opening from the alert, avoiding accidental execution when typing during notification popups.

---

## 📄 License

MIT (or your chosen project license).
