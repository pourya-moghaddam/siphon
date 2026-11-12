# ⚡ Siphon: High-Performance TUI Download Manager

**Siphon** is a fast, reliable, and lightweight terminal download manager with feature parity to Internet Download Manager (IDM), Advanced Download Manager (ADM), and Xtreme Download Manager (XDM).

Built in Rust, it features a decoupled **Client-Server (Daemon + TUI) Architecture** designed specifically for **home servers, Raspberry Pis, and VPS instances accessed via SSH**, as well as local desktop power-users.

---

## 🌟 Key Features

* **Client-Daemon Architecture (`siphond` + `siphon`)**:
  Downloads run inside a persistent background daemon (`siphond`). You can connect via SSH, queue tasks in the interactive TUI, and disconnect your SSH session or shut down your laptop—**your home server will keep downloading overnight uninterrupted**.
* **Dynamic Multi-Connection Segmentation**:
  Accelerates downloads by splitting files into parallel HTTP Range segments ($N$ connections). Uses **dynamic chunk stealing**: as fast connections finish, they dynamically split remaining chunks in half on the fly so all connections stay 100% utilized.
* **Link Refresh / URL Replacement**:
  Expired pre-signed URLs (Google Drive, Rapidgator, AWS S3) mid-download? Press `r` or run `siphon refresh <id> <new-url>` to update the link and seamlessly resume existing segments without losing progress.
* **Queue Management & Scheduling**:
  Create multiple queues with custom concurrency limits and speed caps. Configure automated off-peak download windows (e.g. `02:00` to `07:00`) for overnight ISP quotas.
* **Zero-Fragmentation Disk Pre-allocation**:
  Direct concurrent random-access file writing into `.siphon.part` with `fallocate`. Eliminates disk fragmentation and avoids expensive post-download file merging.
* **Automatic File Categorization**:
  Auto-routes completed downloads to categorized subfolders (`archives/`, `videos/`, `music/`, `documents/`, `programs/`, `images/`).
* **Resilient Persistence**:
  SQLite WAL database preserves byte-accurate segment progress across server reboots, power cuts, or network outages.
* **Minimal Footprint**:
  Statically compiled binary with $<15$ MB idle RAM usage.

---

## 🖥️ TUI Interactive Dashboard

Launch the TUI anytime by typing:
```bash
siphon
```

```text
┌─ ⚡ SIPHON v0.1.0 ───────── [● Daemon: Connected] ─── [Speed: ▼ 42.5 MB/s | Active: 3 tasks] ─┐
│ [1] All Tasks  [2] Downloading  [3] Completed  [4] Queues  [5] Help                           │
├───────────────────────────────────────────────────────────────────────────────────────────────┤
│ ID       │ Queue   │ Name                   │ Size     │ Speed     │ ETA    │ Status      │ Progress       │
├──────────┼─────────┼────────────────────────┼──────────┼───────────┼────────┼─────────────┼────────────────┤
│ 8a4f10c2 │ default │ ubuntu-24.04-live.iso  │ 5.80 GB  │ 24.2 MB/s │ 02m 14s│ Downloading │ [████████░░] 78%│
│ 3b2d901f │ night   │ deep_learning_book.pdf │ 142.0 MB │  1.8 MB/s │ 00m 22s│ Downloading │ [█████████░] 91%│
│ f08e2a14 │ default │ backup_archive.tar.zst │ 18.20 GB │ 16.5 MB/s │ 12m 40s│ Downloading │ [███░░░░░░░] 32%│
├───────────────────────────────────────────────────────────────────────────────────────────────┤
│ ▼ Task Details & Dynamic Segment Heatmap                                                      │
│ URL: https://releases.ubuntu.com/noble/ubuntu-24.04-desktop-amd64.iso                         │
│ Destination: /mnt/storage/downloads/archives/ubuntu-24.04-desktop-amd64.iso                  │
│ Connections: 8 | Range Resumable: Yes                                                         │
│                                                                                               │
│ [Chunk #0: 100%] [Chunk #1: 100%] [Chunk #2:  84.2%] [Chunk #3:  76.5%]                     │
│ [Chunk #4:  62.1%] [Chunk #5:  58.0%] [Chunk #6:  49.3%] [Chunk #7:  41.0%]                     │
├───────────────────────────────────────────────────────────────────────────────────────────────┤
│ [a] Add  [Space] Pause/Resume  [r] Refresh Link  [d] Delete  [q] Detach TUI  [?] Help         │
└───────────────────────────────────────────────────────────────────────────────────────────────┘
```

### Keyboard Shortcuts
| Key | Action |
| :--- | :--- |
| `a` | Add a new download URL (popup dialog) |
| `p` / `Space` | Pause or resume selected download |
| `r` | Refresh expired download link (input new URL to resume) |
| `d` | Delete selected task (optionally delete file with `f`) |
| `j` / `Down` | Move selection down |
| `k` / `Up` | Move selection up |
| `Tab` | Switch tabs (All $\to$ Downloading $\to$ Completed $\to$ Queues) |
| `1` - `5` | Jump directly to specific tab |
| `q` / `Esc` | **Detach TUI** (daemon keeps downloading in background) |
| `?` | Help modal |

---

## 🚀 Quickstart & Installation

### Option 1: Automated Installer (Linux & macOS)
```bash
git clone https://github.com/user/siphon-download-manager.git
cd siphon-download-manager
./scripts/install.sh
```

### Option 2: Build from Source
```bash
cargo build --release
cp target/release/siphond ~/.local/bin/
cp target/release/siphon ~/.local/bin/
```

---

## 🏠 Home Server Setup (Overnight SSH Downloads)

To ensure the `siphond` daemon runs 24/7 on your home server even after disconnecting your SSH session:

1. **Enable Systemd User Service**:
   ```bash
   mkdir -p ~/.config/systemd/user
   cp scripts/siphon.service ~/.config/systemd/user/
   systemctl --user daemon-reload
   systemctl --user enable --now siphon.service
   ```

2. **Enable Linger for your User**:
   ```bash
   sudo loginctl enable-linger $USER
   ```
   *(This ensures systemd does not terminate user background daemons when you log out of SSH).*

Now, whenever you SSH into your home server, simply run `siphon` to check your downloads, queue new files, and safely disconnect!

---

## 🐳 Docker & Docker Compose

Deploy on your home server or NAS using Docker:

```bash
cd docker
docker compose up -d
```

Attach to the TUI inside the container:
```bash
docker exec -it siphond siphon
```

---

## 💻 CLI Automation & Scripting

Siphon also includes a scriptable CLI interface:

```bash
# Add a download with 16 connections into a specific queue
siphon add "https://example.com/largefile.zip" --connections 16 --queue "night"

# List current downloads in terminal
siphon list

# Pause or resume a task
siphon pause 8a4f10c2
siphon resume 8a4f10c2

# Refresh an expired download link
siphon refresh 8a4f10c2 "https://example.com/new-signed-link?token=xyz"

# Delete a task and remove file from disk
siphon delete 8a4f10c2 --file

# Check daemon status
siphon status
```

---

## 📄 License
Licensed under either of Apache License, Version 2.0 or MIT license at your option.
