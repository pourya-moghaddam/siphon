use crate::app::{App, ModalState, Tab};
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{
        Block, BorderType, Borders, Clear, Paragraph, Row, Table, Tabs, Wrap,
    },
    Frame,
};
use siphon_core::models::TaskStatus;

pub fn render(f: &mut Frame, app: &mut App) {
    let size = f.area();

    // Outer layout: Header, Tabs, Main content, Footer
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // Header
            Constraint::Length(2), // Tabs
            Constraint::Min(10),   // Main table & details
            Constraint::Length(1), // Footer status & keymap
        ])
        .split(size);

    render_header(f, app, chunks[0]);
    render_tabs(f, app, chunks[1]);
    render_main_area(f, app, chunks[2]);
    render_footer(f, app, chunks[3]);

    // Render active modal on top
    match &app.modal {
        ModalState::None => {}
        ModalState::AddDownload { .. } => render_add_modal(f, app, size),
        ModalState::RefreshLink { .. } => render_refresh_modal(f, app, size),
        ModalState::ConfirmDelete { .. } => render_delete_modal(f, app, size),
        ModalState::ConfigureQueue { .. } => render_queue_modal(f, app, size),
        ModalState::Help => render_help_modal(f, size),
    }
}

fn render_header(f: &mut Frame, app: &App, area: Rect) {
    let header_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(24), // Title
            Constraint::Length(26), // Connection status
            Constraint::Min(20),   // Global speed & stats
        ])
        .split(area);

    let title = Paragraph::new(Line::from(vec![
        Span::styled("⚡ SIPHON ", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)),
        Span::styled(format!("v{}", siphon_core::version()), Style::default().fg(Color::DarkGray)),
    ]))
    .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded));

    let conn_text = Line::from(vec![
        Span::styled("● ", Style::default().fg(Color::Green)),
        Span::styled("Daemon: ", Style::default().fg(Color::DarkGray)),
        Span::styled("Connected", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
    ]);
    let conn_block = Paragraph::new(conn_text)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded));

    let (speed_str, active_count) = if let Some(ref status) = app.daemon_status {
        (
            format_speed(status.global_speed_bps),
            status.active_downloads,
        )
    } else {
        ("0 B/s".to_string(), 0)
    };

    let stats_text = Line::from(vec![
        Span::styled("Speed: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("▼ {}  ", speed_str), Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Span::styled("Active: ", Style::default().fg(Color::DarkGray)),
        Span::styled(format!("{} tasks", active_count), Style::default().fg(Color::Cyan)),
    ]);
    let stats_block = Paragraph::new(stats_text)
        .alignment(Alignment::Right)
        .block(Block::default().borders(Borders::ALL).border_type(BorderType::Rounded));

    f.render_widget(title, header_chunks[0]);
    f.render_widget(conn_block, header_chunks[1]);
    f.render_widget(stats_block, header_chunks[2]);
}

fn render_tabs(f: &mut Frame, app: &App, area: Rect) {
    let titles = Tab::titles();
    let tabs = Tabs::new(titles)
        .select(app.current_tab as usize)
        .style(Style::default().fg(Color::DarkGray))
        .highlight_style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD | Modifier::UNDERLINED))
        .divider(" | ");

    f.render_widget(tabs, area);
}

fn render_main_area(f: &mut Frame, app: &App, area: Rect) {
    match app.current_tab {
        Tab::Queues => render_queues_view(f, app, area),
        Tab::Help => render_help_view(f, area),
        _ => {
            // Split table & details panel
            let v_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(area);

            render_tasks_table(f, app, v_chunks[0]);
            render_task_details(f, app, v_chunks[1]);
        }
    }
}

fn render_tasks_table(f: &mut Frame, app: &App, area: Rect) {
    let tasks = app.filtered_tasks();

    let header_cells = ["ID", "Queue", "Name", "Size", "Speed", "ETA", "Status", "Progress"]
        .iter()
        .map(|h| Span::styled(*h, Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD)));
    let header = Row::new(header_cells).height(1).bottom_margin(0);

    let rows: Vec<Row> = tasks
        .iter()
        .enumerate()
        .map(|(i, task)| {
            let is_selected = i == app.selected_index;
            let status_style = match task.status {
                TaskStatus::Downloading => Style::default().fg(Color::Yellow),
                TaskStatus::Completed => Style::default().fg(Color::Green),
                TaskStatus::Paused => Style::default().fg(Color::DarkGray),
                TaskStatus::Expired => Style::default().fg(Color::LightRed).add_modifier(Modifier::BOLD),
                TaskStatus::Error(_) => Style::default().fg(Color::Red),
                _ => Style::default().fg(Color::White),
            };

            let progress_pct = task.progress_percent();
            let progress_bar = format_progress_bar(progress_pct, 12);

            let short_id = if task.id.len() > 8 { &task.id[..8] } else { &task.id };
            let size_str = task
                .total_bytes
                .map(format_bytes)
                .unwrap_or_else(|| "Unknown".to_string());
            let speed_str = if task.speed_bps > 0 {
                format_speed(task.speed_bps)
            } else {
                "-".to_string()
            };
            let eta_str = task
                .eta_seconds
                .map(format_eta)
                .unwrap_or_else(|| "-".to_string());

            let mut row_style = Style::default();
            if is_selected {
                row_style = row_style.bg(Color::Rgb(30, 40, 60)).add_modifier(Modifier::BOLD);
            }

            Row::new(vec![
                Span::raw(short_id.to_string()),
                Span::raw(task.queue_name.clone()),
                Span::raw(task.file_name.clone()),
                Span::raw(size_str),
                Span::raw(speed_str),
                Span::raw(eta_str),
                Span::styled(format!("{}", task.status), status_style),
                Span::raw(format!("{} {:>5.1}%", progress_bar, progress_pct)),
            ])
            .style(row_style)
            .height(1)
        })
        .collect();

    let widths = [
        Constraint::Length(10), // ID
        Constraint::Length(10), // Queue
        Constraint::Percentage(30), // Name
        Constraint::Length(12), // Size
        Constraint::Length(12), // Speed
        Constraint::Length(10), // ETA
        Constraint::Length(14), // Status
        Constraint::Length(22), // Progress
    ];

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .title(" Downloads ")
                .borders(Borders::ALL)
                .border_type(BorderType::Rounded),
        );

    f.render_widget(table, area);
}

fn render_task_details(f: &mut Frame, app: &App, area: Rect) {
    let task = app.selected_task();

    let block = Block::default()
        .title(" Task Details & Dynamic Segment Heatmap ")
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded);

    let inner = block.inner(area);
    f.render_widget(block, area);

    if let Some(t) = task {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Metadata
                Constraint::Min(4),    // Segments Heatmap
            ])
            .split(inner);

        // Metadata lines
        let meta_lines = vec![
            Line::from(vec![
                Span::styled("URL: ", Style::default().fg(Color::DarkGray)),
                Span::styled(&t.url, Style::default().fg(Color::White)),
            ]),
            Line::from(vec![
                Span::styled("Destination: ", Style::default().fg(Color::DarkGray)),
                Span::styled(t.destination_path.to_string_lossy(), Style::default().fg(Color::White)),
                Span::styled(" | Range Resumable: ", Style::default().fg(Color::DarkGray)),
                Span::styled(
                    if t.supports_range { "Yes" } else { "No" },
                    if t.supports_range { Style::default().fg(Color::Green) } else { Style::default().fg(Color::Yellow) },
                ),
                Span::styled(" | Connections: ", Style::default().fg(Color::DarkGray)),
                Span::styled(format!("{}", t.connections), Style::default().fg(Color::Cyan)),
            ]),
        ];
        let meta_par = Paragraph::new(meta_lines);
        f.render_widget(meta_par, chunks[0]);

        // Dynamic Segment Heatmap
        render_segment_heatmap(f, &app.selected_segments, chunks[1]);
    } else {
        let empty_msg = Paragraph::new("No task selected")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        f.render_widget(empty_msg, inner);
    }
}

fn render_segment_heatmap(f: &mut Frame, segments: &[siphon_core::models::Segment], area: Rect) {
    if segments.is_empty() {
        let msg = Paragraph::new("No active chunk segment data available for single-stream download")
            .style(Style::default().fg(Color::DarkGray));
        f.render_widget(msg, area);
        return;
    }

    let mut lines = Vec::new();
    let mut current_spans = Vec::new();

    for seg in segments {
        let pct = seg.progress_percent();
        let color = if seg.is_finished() {
            Color::Green
        } else if seg.active {
            Color::Yellow
        } else {
            Color::DarkGray
        };

        let badge = format!("[Chunk #{}: {:>5.1}%] ", seg.id, pct);
        current_spans.push(Span::styled(badge, Style::default().fg(color).add_modifier(Modifier::BOLD)));

        if current_spans.len() >= 4 {
            lines.push(Line::from(current_spans));
            current_spans = Vec::new();
        }
    }

    if !current_spans.is_empty() {
        lines.push(Line::from(current_spans));
    }

    let heatmap_widget = Paragraph::new(lines).wrap(Wrap { trim: true });
    f.render_widget(heatmap_widget, area);
}

fn render_queues_view(f: &mut Frame, app: &App, area: Rect) {
    let rows: Vec<Row> = app
        .queues
        .iter()
        .map(|q| {
            let active_str = if q.is_active { "Active" } else { "Paused" };
            let active_style = if q.is_active {
                Style::default().fg(Color::Green)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let sched_str = match (&q.schedule_start, &q.schedule_end) {
                (Some(s), Some(e)) => format!("{} - {}", s, e),
                _ => "None (Always)".to_string(),
            };

            Row::new(vec![
                Span::raw(q.name.clone()),
                Span::raw(format!("{}", q.max_concurrent)),
                Span::raw(q.speed_limit_bps.map(format_speed).unwrap_or_else(|| "Unlimited".to_string())),
                Span::raw(sched_str),
                Span::styled(active_str, active_style),
            ])
        })
        .collect();

    let widths = [
        Constraint::Percentage(25),
        Constraint::Percentage(15),
        Constraint::Percentage(20),
        Constraint::Percentage(25),
        Constraint::Percentage(15),
    ];

    let header = Row::new(vec!["Queue Name", "Max Concurrent", "Speed Limit", "Schedule Window", "Status"])
        .style(Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD));

    let table = Table::new(rows, widths)
        .header(header)
        .block(Block::default().title(" Configured Download Queues ").borders(Borders::ALL).border_type(BorderType::Rounded));

    f.render_widget(table, area);
}

fn render_help_view(f: &mut Frame, area: Rect) {
    let help_text = vec![
        Line::from(vec![Span::styled("Siphon Keyboard Controls", Style::default().fg(Color::Cyan).add_modifier(Modifier::BOLD))]),
        Line::from(""),
        Line::from("  [a]            Add a new download URL"),
        Line::from("  [p] / [Space]  Pause / Resume selected download"),
        Line::from("  [r]            Refresh download link (update expired pre-signed/host URL)"),
        Line::from("  [d]            Delete selected download task"),
        Line::from("  [j] / [Down]   Select next task"),
        Line::from("  [k] / [Up]     Select previous task"),
        Line::from("  [Tab]          Cycle through views (All / Downloading / Completed / Queues)"),
        Line::from("  [1 - 5]        Switch directly to view"),
        Line::from("  [q] / [Esc]    Detach TUI (daemon keeps downloading in background!)"),
        Line::from("  [?]            Show quick help modal"),
        Line::from(""),
        Line::from(vec![Span::styled("Background Operation & SSH", Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD))]),
        Line::from("  Siphon runs as a background daemon (siphond)."),
        Line::from("  You can safely close your SSH session or turn off your laptop;"),
        Line::from("  the home server will continue downloading overnight!"),
    ];

    let p = Paragraph::new(help_text)
        .block(Block::default().title(" Help & Documentation ").borders(Borders::ALL).border_type(BorderType::Rounded));
    f.render_widget(p, area);
}

fn render_footer(f: &mut Frame, app: &App, area: Rect) {
    let content = if let Some((ref msg, _)) = app.status_message {
        Line::from(vec![
            Span::styled("ℹ ", Style::default().fg(Color::Cyan)),
            Span::styled(msg, Style::default().fg(Color::White).add_modifier(Modifier::BOLD)),
        ])
    } else {
        match app.current_tab {
            Tab::Queues => Line::from(vec![
                Span::styled("[c] ", Style::default().fg(Color::Cyan)),
                Span::raw("Create/Config Queue  "),
                Span::styled("[Tab] ", Style::default().fg(Color::Cyan)),
                Span::raw("Tabs  "),
                Span::styled("[q] ", Style::default().fg(Color::Cyan)),
                Span::raw("Detach TUI  "),
                Span::styled("[?] ", Style::default().fg(Color::Cyan)),
                Span::raw("Help"),
            ]),
            _ => Line::from(vec![
                Span::styled("[a] ", Style::default().fg(Color::Cyan)),
                Span::raw("Add  "),
                Span::styled("[Space] ", Style::default().fg(Color::Cyan)),
                Span::raw("Pause/Resume  "),
                Span::styled("[r] ", Style::default().fg(Color::Cyan)),
                Span::raw("Refresh Link  "),
                Span::styled("[c] ", Style::default().fg(Color::Cyan)),
                Span::raw("Queue Config  "),
                Span::styled("[d] ", Style::default().fg(Color::Cyan)),
                Span::raw("Delete  "),
                Span::styled("[q] ", Style::default().fg(Color::Cyan)),
                Span::raw("Detach TUI  "),
                Span::styled("[?] ", Style::default().fg(Color::Cyan)),
                Span::raw("Help"),
            ]),
        }
    };

    let p = Paragraph::new(content).style(Style::default().bg(Color::Rgb(20, 20, 30)));
    f.render_widget(p, area);
}

fn render_queue_modal(f: &mut Frame, app: &App, area: Rect) {
    if let ModalState::ConfigureQueue {
        ref name_input,
        ref concurrency_input,
        ref start_time_input,
        ref stop_time_input,
        focused_field,
    } = app.modal
    {
        let modal_area = centered_rect(65, 55, area);
        f.render_widget(Clear, modal_area);

        let block = Block::default()
            .title(" Configure Download Queue & Schedule ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(Color::Rgb(15, 20, 30)));

        let inner = block.inner(modal_area);
        f.render_widget(block, modal_area);

        let fields = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // Queue Name
                Constraint::Length(3), // Concurrency
                Constraint::Length(3), // Start Time
                Constraint::Length(3), // Stop Time
                Constraint::Length(2), // Help
            ])
            .split(inner);

        let name_style = if focused_field == 0 {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let name_widget = Paragraph::new(name_input.as_str())
            .block(Block::default().title(" Queue Name (e.g. night, default, slow) ").borders(Borders::ALL).border_style(name_style));
        f.render_widget(name_widget, fields[0]);

        let conc_style = if focused_field == 1 {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let conc_widget = Paragraph::new(concurrency_input.as_str())
            .block(Block::default().title(" Max Concurrent Downloads (default: 3) ").borders(Borders::ALL).border_style(conc_style));
        f.render_widget(conc_widget, fields[1]);

        let start_style = if focused_field == 2 {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let start_widget = Paragraph::new(if start_time_input.is_empty() { "<None - Run 24/7>" } else { start_time_input.as_str() })
            .style(if start_time_input.is_empty() { Style::default().fg(Color::DarkGray) } else { Style::default() })
            .block(Block::default().title(" Schedule Start Time [HH:MM] (optional, e.g. 02:00) ").borders(Borders::ALL).border_style(start_style));
        f.render_widget(start_widget, fields[2]);

        let stop_style = if focused_field == 3 {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let stop_widget = Paragraph::new(if stop_time_input.is_empty() { "<None - Run 24/7>" } else { stop_time_input.as_str() })
            .style(if stop_time_input.is_empty() { Style::default().fg(Color::DarkGray) } else { Style::default() })
            .block(Block::default().title(" Schedule Stop Time [HH:MM] (optional, e.g. 07:00) ").borders(Borders::ALL).border_style(stop_style));
        f.render_widget(stop_widget, fields[3]);

        let help = Paragraph::new(" [Tab] Next field  |  [Enter] Save Queue  |  [Esc] Cancel ")
            .style(Style::default().fg(Color::Yellow))
            .alignment(Alignment::Center);
        f.render_widget(help, fields[4]);
    }
}

fn render_add_modal(f: &mut Frame, app: &App, area: Rect) {
    if let ModalState::AddDownload {
        ref url_input,
        ref dest_input,
        ref queue_input,
        focused_field,
    } = app.modal
    {
        let modal_area = centered_rect(65, 45, area);
        f.render_widget(Clear, modal_area);

        let block = Block::default()
            .title(" Add New Download ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(Color::Rgb(15, 20, 30)));

        let inner = block.inner(modal_area);
        f.render_widget(block, modal_area);

        let fields = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(3), // URL
                Constraint::Length(3), // Destination
                Constraint::Length(3), // Queue
                Constraint::Length(2), // Help
            ])
            .split(inner);

        // URL field
        let url_style = if focused_field == 0 {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let url_widget = Paragraph::new(url_input.as_str())
            .block(Block::default().title(" Download URL (required) ").borders(Borders::ALL).border_style(url_style));
        f.render_widget(url_widget, fields[0]);

        // Dest field
        let dest_style = if focused_field == 1 {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let dest_widget = Paragraph::new(if dest_input.is_empty() { "<Default Auto-Categorized Folder>" } else { dest_input.as_str() })
            .style(if dest_input.is_empty() { Style::default().fg(Color::DarkGray) } else { Style::default() })
            .block(Block::default().title(" Custom Destination (optional) ").borders(Borders::ALL).border_style(dest_style));
        f.render_widget(dest_widget, fields[1]);

        // Queue field
        let q_style = if focused_field == 2 {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::DarkGray)
        };
        let q_widget = Paragraph::new(queue_input.as_str())
            .block(Block::default().title(" Queue Name ").borders(Borders::ALL).border_style(q_style));
        f.render_widget(q_widget, fields[2]);

        let help = Paragraph::new(" [Tab] Next field  |  [Enter] Start Download  |  [Esc] Cancel ")
            .style(Style::default().fg(Color::Yellow))
            .alignment(Alignment::Center);
        f.render_widget(help, fields[3]);
    }
}

fn render_refresh_modal(f: &mut Frame, app: &App, area: Rect) {
    if let ModalState::RefreshLink {
        ref task_name,
        ref new_url_input,
        ..
    } = app.modal
    {
        let modal_area = centered_rect(65, 35, area);
        f.render_widget(Clear, modal_area);

        let block = Block::default()
            .title(" Refresh Expired Download Link ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(Color::Rgb(25, 20, 20)));

        let inner = block.inner(modal_area);
        f.render_widget(block, modal_area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(3),
                Constraint::Length(2),
            ])
            .split(inner);

        let info = Paragraph::new(format!("Task: {}", task_name))
            .style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD));
        f.render_widget(info, chunks[0]);

        let input = Paragraph::new(new_url_input.as_str())
            .block(Block::default().title(" New URL ").borders(Borders::ALL).border_style(Style::default().fg(Color::Cyan)));
        f.render_widget(input, chunks[1]);

        let help = Paragraph::new(" [Enter] Apply & Resume  |  [Esc] Cancel ")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        f.render_widget(help, chunks[2]);
    }
}

fn render_delete_modal(f: &mut Frame, app: &App, area: Rect) {
    if let ModalState::ConfirmDelete {
        ref file_name,
        delete_disk_file,
        ..
    } = app.modal
    {
        let modal_area = centered_rect(50, 25, area);
        f.render_widget(Clear, modal_area);

        let block = Block::default()
            .title(" Confirm Delete ")
            .borders(Borders::ALL)
            .border_type(BorderType::Rounded)
            .style(Style::default().bg(Color::Rgb(25, 15, 15)));

        let inner = block.inner(modal_area);
        f.render_widget(block, modal_area);

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Length(2),
                Constraint::Length(2),
            ])
            .split(inner);

        let msg = Paragraph::new(format!("Delete task '{}'?", file_name))
            .style(Style::default().fg(Color::White).add_modifier(Modifier::BOLD));
        f.render_widget(msg, chunks[0]);

        let toggle = Paragraph::new(format!(
            "[{}] Also delete downloaded file from disk (Press [f] to toggle)",
            if delete_disk_file { "X" } else { " " }
        ))
        .style(Style::default().fg(Color::Yellow));
        f.render_widget(toggle, chunks[1]);

        let help = Paragraph::new(" [Enter] Delete  |  [Esc] Cancel ")
            .style(Style::default().fg(Color::DarkGray))
            .alignment(Alignment::Center);
        f.render_widget(help, chunks[2]);
    }
}

fn render_help_modal(f: &mut Frame, area: Rect) {
    let modal_area = centered_rect(60, 60, area);
    f.render_widget(Clear, modal_area);
    render_help_view(f, modal_area);
}

fn centered_rect(percent_x: u16, percent_y: u16, r: Rect) -> Rect {
    let popup_layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(r);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(popup_layout[1])[1]
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

fn format_speed(bps: u64) -> String {
    format!("{}/s", format_bytes(bps))
}

fn format_eta(secs: u64) -> String {
    if secs >= 3600 {
        format!("{:02}h {:02}m", secs / 3600, (secs % 3600) / 60)
    } else {
        format!("{:02}m {:02}s", secs / 60, secs % 60)
    }
}

fn format_progress_bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let empty = width.saturating_sub(filled);
    format!("[{}{}]", "█".repeat(filled), "░".repeat(empty))
}
