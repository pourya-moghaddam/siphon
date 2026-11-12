mod app;
mod client;
mod ui;

use anyhow::Result;
use app::{App, ModalState, Tab};
use clap::{Parser, Subcommand};
use client::IpcClient;
use crossterm::{
    event::{self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use siphon_core::protocol::{Request, Response};
use std::io;
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser, Debug)]
#[command(name = "siphon")]
#[command(about = "High-performance, lightweight TUI & CLI download manager", version)]
struct Cli {
    /// Path to siphond Unix domain socket
    #[arg(short, long, global = true)]
    socket: Option<PathBuf>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Launch the interactive Terminal User Interface (default)
    Tui,

    /// Add a new download URL
    Add {
        /// Download URL
        url: String,

        /// Custom destination file path
        #[arg(short, long)]
        dest: Option<String>,

        /// Target queue (default: "default")
        #[arg(short, long)]
        queue: Option<String>,

        /// Number of parallel segmented connections (default: 8)
        #[arg(short, long)]
        connections: Option<usize>,
    },

    /// List all download tasks
    List,

    /// Pause a download task
    Pause {
        /// Task ID or prefix
        id: String,
    },

    /// Resume a download task
    Resume {
        /// Task ID or prefix
        id: String,
    },

    /// Delete a download task
    Delete {
        /// Task ID
        id: String,

        /// Also delete the downloaded file from disk
        #[arg(short, long)]
        file: bool,
    },

    /// Refresh an expired download URL
    Refresh {
        /// Task ID
        id: String,

        /// New active URL
        new_url: String,
    },

    /// Get daemon status
    Status,

    /// Manage download queues and schedules
    Queue {
        #[command(subcommand)]
        command: QueueCommands,
    },
}

#[derive(Subcommand, Debug)]
enum QueueCommands {
    /// List all configured queues
    List,

    /// Create or update a queue
    Create {
        /// Queue name (e.g. "night", "work", "slow")
        name: String,

        /// Maximum concurrent downloads (default: 3)
        #[arg(short, long, default_value_t = 3)]
        concurrency: usize,

        /// Speed limit in bytes per second (optional)
        #[arg(short, long)]
        speed: Option<u64>,
    },

    /// Configure start and stop times for a queue (e.g. off-peak overnight hours)
    Schedule {
        /// Queue name
        name: String,

        /// Start time in 24-hour format (e.g. "02:00")
        #[arg(short, long)]
        start: Option<String>,

        /// Stop time in 24-hour format (e.g. "07:00")
        #[arg(short, long)]
        stop: Option<String>,

        /// Clear schedule (run 24/7)
        #[arg(long)]
        clear: bool,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let socket_path = cli.socket.unwrap_or_else(siphon_core::default_socket_path);
    let client = IpcClient::new(socket_path);

    match cli.command {
        None | Some(Commands::Tui) => run_tui(client).await,
        Some(Commands::Add {
            url,
            dest,
            queue,
            connections,
        }) => handle_add(client, url, dest, queue, connections).await,
        Some(Commands::List) => handle_list(client).await,
        Some(Commands::Pause { id }) => handle_pause(client, id).await,
        Some(Commands::Resume { id }) => handle_resume(client, id).await,
        Some(Commands::Delete { id, file }) => handle_delete(client, id, file).await,
        Some(Commands::Refresh { id, new_url }) => handle_refresh(client, id, new_url).await,
        Some(Commands::Status) => handle_status(client).await,
        Some(Commands::Queue { command }) => handle_queue(client, command).await,
    }
}

async fn handle_add(
    client: IpcClient,
    url: String,
    dest: Option<String>,
    queue: Option<String>,
    connections: Option<usize>,
) -> Result<()> {
    let resp = client
        .send_request(&Request::AddTask {
            url,
            destination: dest,
            queue,
            connections,
            speed_limit: None,
        })
        .await?;

    match resp {
        Response::TaskAdded(task) => {
            println!("✔ Added task {}: {}", task.id, task.file_name);
            println!("  Destination: {:?}", task.destination_path);
            println!("  Connections: {}", task.connections);
            println!("  Range Resumable: {}", if task.supports_range { "Yes" } else { "No" });
        }
        Response::Error(err) => {
            eprintln!("✘ Failed to add task: {}", err);
        }
        _ => {}
    }
    Ok(())
}

async fn handle_list(client: IpcClient) -> Result<()> {
    let resp = client.send_request(&Request::ListTasks).await?;
    match resp {
        Response::Tasks(tasks) => {
            if tasks.is_empty() {
                println!("No downloads found.");
                return Ok(());
            }

            println!(
                "{:<10} {:<10} {:<30} {:<12} {:<12} {:<12} {:<10}",
                "ID", "Queue", "Name", "Size", "Progress", "Speed", "Status"
            );
            println!("{}", "-".repeat(96));

            for t in tasks {
                let short_id = if t.id.len() > 8 { &t.id[..8] } else { &t.id };
                let short_name = if t.file_name.len() > 28 {
                    format!("{}...", &t.file_name[..25])
                } else {
                    t.file_name.clone()
                };
                let size_str = t
                    .total_bytes
                    .map(format_bytes)
                    .unwrap_or_else(|| "Unknown".to_string());
                let speed_str = if t.speed_bps > 0 {
                    format!("{}/s", format_bytes(t.speed_bps))
                } else {
                    "-".to_string()
                };

                println!(
                    "{:<10} {:<10} {:<30} {:<12} {:>5.1}% {:<6} {:<12} {:<10}",
                    short_id,
                    t.queue_name,
                    short_name,
                    size_str,
                    t.progress_percent(),
                    "",
                    speed_str,
                    format!("{}", t.status)
                );
            }
        }
        Response::Error(err) => eprintln!("Error: {}", err),
        _ => {}
    }
    Ok(())
}

async fn handle_pause(client: IpcClient, id: String) -> Result<()> {
    match client.send_request(&Request::PauseTask { id: id.clone() }).await? {
        Response::Ok => println!("✔ Paused task {}", id),
        Response::Error(e) => eprintln!("✘ Error: {}", e),
        _ => {}
    }
    Ok(())
}

async fn handle_resume(client: IpcClient, id: String) -> Result<()> {
    match client.send_request(&Request::ResumeTask { id: id.clone() }).await? {
        Response::Ok => println!("✔ Resumed task {}", id),
        Response::Error(e) => eprintln!("✘ Error: {}", e),
        _ => {}
    }
    Ok(())
}

async fn handle_delete(client: IpcClient, id: String, delete_file: bool) -> Result<()> {
    match client.send_request(&Request::DeleteTask { id: id.clone(), delete_file }).await? {
        Response::Ok => println!("✔ Deleted task {}", id),
        Response::Error(e) => eprintln!("✘ Error: {}", e),
        _ => {}
    }
    Ok(())
}

async fn handle_refresh(client: IpcClient, id: String, new_url: String) -> Result<()> {
    match client.send_request(&Request::RefreshTaskUrl { id: id.clone(), new_url }).await? {
        Response::Ok => println!("✔ Refreshed URL for task {}. Download will resume.", id),
        Response::Error(e) => eprintln!("✘ Refresh Error: {}", e),
        _ => {}
    }
    Ok(())
}

async fn handle_status(client: IpcClient) -> Result<()> {
    match client.send_request(&Request::GetDaemonStatus).await? {
        Response::DaemonStatus(status) => {
            println!("Siphon Daemon Status:");
            println!("  Version:          {}", status.version);
            println!("  Uptime:           {}s", status.uptime_seconds);
            println!("  Active Downloads: {}", status.active_downloads);
            println!("  Global Speed:     {}/s", format_bytes(status.global_speed_bps));
            println!("  Queues Count:     {}", status.queues_count);
        }
        Response::Error(e) => eprintln!("Error: {}", e),
        _ => {}
    }
    Ok(())
}

async fn handle_queue(client: IpcClient, cmd: QueueCommands) -> Result<()> {
    match cmd {
        QueueCommands::List => {
            let resp = client.send_request(&Request::ListQueues).await?;
            if let Response::Queues(queues) = resp {
                println!(
                    "{:<15} {:<15} {:<15} {:<20} {:<10}",
                    "Queue Name", "Concurrency", "Speed Limit", "Schedule Window", "Status"
                );
                println!("{}", "-".repeat(75));
                for q in queues {
                    let sched_str = match (&q.schedule_start, &q.schedule_end) {
                        (Some(s), Some(e)) => format!("{} - {}", s, e),
                        _ => "None (24/7)".to_string(),
                    };
                    let speed_str = q
                        .speed_limit_bps
                        .map(|b| format!("{}/s", format_bytes(b)))
                        .unwrap_or_else(|| "Unlimited".to_string());
                    println!(
                        "{:<15} {:<15} {:<15} {:<20} {:<10}",
                        q.name,
                        q.max_concurrent,
                        speed_str,
                        sched_str,
                        if q.is_active { "Active" } else { "Paused" }
                    );
                }
            }
        }
        QueueCommands::Create {
            name,
            concurrency,
            speed,
        } => {
            let resp = client
                .send_request(&Request::CreateQueue {
                    name: name.clone(),
                    max_concurrent: concurrency,
                    speed_limit: speed,
                })
                .await?;
            match resp {
                Response::Ok => println!("✔ Queue '{}' created/updated (max concurrent: {})", name, concurrency),
                Response::Error(e) => eprintln!("✘ Error: {}", e),
                _ => {}
            }
        }
        QueueCommands::Schedule {
            name,
            start,
            stop,
            clear,
        } => {
            let (start_time, stop_time) = if clear {
                (None, None)
            } else {
                (start, stop)
            };

            let resp = client
                .send_request(&Request::ConfigureSchedule {
                    queue_name: name.clone(),
                    start_time: start_time.clone(),
                    stop_time: stop_time.clone(),
                    on_complete: siphon_core::models::OnCompleteAction::None,
                })
                .await?;

            match resp {
                Response::Ok => {
                    if let (Some(s), Some(e)) = (start_time, stop_time) {
                        println!("✔ Queue '{}' scheduled for {} - {} daily", name, s, e);
                    } else {
                        println!("✔ Queue '{}' schedule cleared (will run 24/7)", name);
                    }
                }
                Response::Error(e) => eprintln!("✘ Error: {}", e),
                _ => {}
            }
        }
    }
    Ok(())
}

async fn run_tui(client: IpcClient) -> Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(client);

    // Initial fetch
    let _ = app.refresh_data().await;

    let res = tui_event_loop(&mut terminal, &mut app).await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;

    if let Err(err) = res {
        eprintln!("TUI Error: {:?}", err);
    }

    Ok(())
}

async fn tui_event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    let mut last_tick = Instant::now();
    let tick_rate = Duration::from_millis(500);

    loop {
        terminal.draw(|f| ui::render(f, app))?;

        let timeout = tick_rate
            .checked_sub(last_tick.elapsed())
            .unwrap_or_else(|| Duration::from_secs(0));

        if event::poll(timeout)? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    handle_key_event(app, key.code, key.modifiers).await?;
                }
            }
        }

        if last_tick.elapsed() >= tick_rate {
            let _ = app.refresh_data().await;
            last_tick = Instant::now();
        }

        if app.should_quit {
            break;
        }
    }

    Ok(())
}

async fn handle_key_event(app: &mut App, code: KeyCode, modifiers: KeyModifiers) -> Result<()> {
    // If in modal, handle modal keystrokes
    match &mut app.modal {
        ModalState::AddDownload {
            ref mut url_input,
            ref mut dest_input,
            ref mut queue_input,
            ref mut focused_field,
        } => match code {
            KeyCode::Esc => app.close_modal(),
            KeyCode::Tab => {
                *focused_field = (*focused_field + 1) % 3;
            }
            KeyCode::BackTab => {
                *focused_field = if *focused_field == 0 { 2 } else { *focused_field - 1 };
            }
            KeyCode::Enter => {
                app.submit_add_modal().await?;
            }
            KeyCode::Backspace => {
                let target = match *focused_field {
                    0 => url_input,
                    1 => dest_input,
                    _ => queue_input,
                };
                target.pop();
            }
            KeyCode::Char(c) => {
                let target = match *focused_field {
                    0 => url_input,
                    1 => dest_input,
                    _ => queue_input,
                };
                target.push(c);
            }
            _ => {}
        },

        ModalState::RefreshLink {
            ref mut new_url_input,
            ..
        } => match code {
            KeyCode::Esc => app.close_modal(),
            KeyCode::Enter => {
                app.submit_refresh_modal().await?;
            }
            KeyCode::Backspace => {
                new_url_input.pop();
            }
            KeyCode::Char(c) => {
                new_url_input.push(c);
            }
            _ => {}
        },

        ModalState::ConfirmDelete {
            ref mut delete_disk_file,
            ..
        } => match code {
            KeyCode::Esc => app.close_modal(),
            KeyCode::Char('f') | KeyCode::Char('F') => {
                *delete_disk_file = !*delete_disk_file;
            }
            KeyCode::Enter => {
                app.submit_delete_modal().await?;
            }
            _ => {}
        },

        ModalState::ConfigureQueue {
            ref mut name_input,
            ref mut concurrency_input,
            ref mut start_time_input,
            ref mut stop_time_input,
            ref mut focused_field,
        } => match code {
            KeyCode::Esc => app.close_modal(),
            KeyCode::Tab => {
                *focused_field = (*focused_field + 1) % 4;
            }
            KeyCode::BackTab => {
                *focused_field = if *focused_field == 0 { 3 } else { *focused_field - 1 };
            }
            KeyCode::Enter => {
                app.submit_queue_modal().await?;
            }
            KeyCode::Backspace => {
                let target = match *focused_field {
                    0 => name_input,
                    1 => concurrency_input,
                    2 => start_time_input,
                    _ => stop_time_input,
                };
                target.pop();
            }
            KeyCode::Char(c) => {
                let target = match *focused_field {
                    0 => name_input,
                    1 => concurrency_input,
                    2 => start_time_input,
                    _ => stop_time_input,
                };
                target.push(c);
            }
            _ => {}
        },

        ModalState::Help => {
            if code == KeyCode::Esc || code == KeyCode::Char('q') || code == KeyCode::Enter {
                app.close_modal();
            }
        }

        ModalState::None => {
            // Normal navigation
            match code {
                KeyCode::Char('q') | KeyCode::Esc => {
                    app.should_quit = true;
                }
                KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => {
                    app.should_quit = true;
                }
                KeyCode::Tab => {
                    let next_tab = (app.current_tab as usize + 1) % 5;
                    app.current_tab = Tab::from_index(next_tab);
                    app.selected_index = 0;
                }
                KeyCode::Char('1') => app.current_tab = Tab::All,
                KeyCode::Char('2') => app.current_tab = Tab::Downloading,
                KeyCode::Char('3') => app.current_tab = Tab::Completed,
                KeyCode::Char('4') => app.current_tab = Tab::Queues,
                KeyCode::Char('5') | KeyCode::Char('?') => app.modal = ModalState::Help,

                KeyCode::Down | KeyCode::Char('j') => {
                    app.select_next();
                }
                KeyCode::Up | KeyCode::Char('k') => {
                    app.select_prev();
                }

                KeyCode::Char('a') => {
                    app.open_add_modal();
                }
                KeyCode::Char('c') => {
                    app.open_queue_modal();
                }
                KeyCode::Char('r') => {
                    app.open_refresh_modal();
                }
                KeyCode::Char('d') => {
                    app.open_delete_modal();
                }
                KeyCode::Char('p') | KeyCode::Char(' ') => {
                    app.toggle_pause_selected().await?;
                }
                _ => {}
            }
        }
    }

    Ok(())
}

fn format_bytes(bytes: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = KB * 1024;
    const GB: u64 = MB * 1024;
    const TB: u64 = GB * 1024;

    if bytes >= TB {
        format!("{:.2} TB", bytes as f64 / TB as f64)
    } else if bytes >= GB {
        format!("{:.2} GB", bytes as f64 / GB as f64)
    } else if bytes >= MB {
        format!("{:.2} MB", bytes as f64 / MB as f64)
    } else if bytes >= KB {
        format!("{:.2} KB", bytes as f64 / KB as f64)
    } else {
        format!("{} B", bytes)
    }
}
