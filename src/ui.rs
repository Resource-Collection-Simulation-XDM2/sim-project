use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Gauge, Paragraph};
use ratatui::Frame;

use crate::app::{AppSnapshot, RobotKind};
use crate::world::{Cell, Position};

pub(crate) fn render(f: &mut Frame, snapshot: &AppSnapshot<'_>) {
    let area = f.area();
    let layout = Layout::vertical([Constraint::Min(1), Constraint::Length(6)]).split(area);
    render_map(f, snapshot, layout[0]);
    render_status(f, snapshot, layout[1]);
}

fn render_map(f: &mut Frame, snapshot: &AppSnapshot<'_>, area: Rect) {
    let w = snapshot.sim_world.width();
    let h = snapshot.sim_world.height();
    let cw = snapshot.visual_theme.cell_width as usize;
    let mut lines: Vec<Line> = Vec::with_capacity(h);

    for y in 0..h {
        let mut spans: Vec<Span> = Vec::with_capacity(w * cw);
        for x in 0..w {
            let pos = Position { x, y };
            let (ch, color) = cell_char(snapshot, pos);
            spans.push(Span::styled(ch, Style::default().fg(color)));
            if cw > 1 && ch.len() < 4 {
                spans.push(Span::styled(" ", Style::default()));
            }
        }
        lines.push(Line::from(spans));
    }

    f.render_widget(Paragraph::new(lines), area);
}

fn cell_char<'a>(snapshot: &'a AppSnapshot<'a>, pos: Position) -> (&'a str, Color) {
    let t = snapshot.visual_theme;
    let idx = pos.y * snapshot.sim_world.width() + pos.x;

    match snapshot.robot_at.get(idx).copied().flatten() {
        Some(RobotKind::Scout) => return (t.scout_char, t.scout_color),
        Some(RobotKind::Collector) => return (t.collector_char, t.collector_color),
        None => {}
    }

    if pos == snapshot.sim_world.base() {
        return (t.base_char, base_pulse_color(t.base_color, snapshot.tick));
    }

    let revealed = snapshot.revealed_cache.get(idx).copied().unwrap_or(false);
    if !revealed {
        return fog_display(snapshot, pos);
    }

    if snapshot.sim_world.cell(pos) == Some(Cell::Obstacle) {
        let biome = snapshot.sim_world.obstacle_biome(pos).unwrap_or(0);
        return (t.obstacle_char(biome), t.obstacle_color_for(biome));
    }

    if let Some(stock_idx) = snapshot.stock_at_cache.get(idx).copied().flatten()
        && stock_idx < snapshot.resource_discovered_cache.len()
        && snapshot.resource_discovered_cache[stock_idx]
        && stock_idx < snapshot.stock_remaining_cache.len()
    {
        let remaining = snapshot.stock_remaining_cache[stock_idx];
        if remaining > 0 {
            let initial = snapshot.stock_initial_cache[stock_idx];
            let ratio = remaining as f64 / (initial as f64).max(1.0);
            let resource = &snapshot.sim_world.resources()[stock_idx];
            let (ch, base_color) = t.resource_display(resource.kind);
            return (ch, resource_pulse_color(base_color, ratio, snapshot.tick));
        }
    }

    (t.ground_char, t.ground_color)
}

fn base_pulse_color(base: Color, tick: u64) -> Color {
    let phase = (tick % 30) as f64 / 30.0;
    fade_color(base, 0.75 + 0.25 * (phase * std::f64::consts::TAU).sin())
}

fn resource_pulse_color(base_color: Color, ratio: f64, tick: u64) -> Color {
    let phase = (tick % 30) as f64 / 30.0;
    let pulse = 0.80 + 0.20 * (phase * std::f64::consts::TAU).sin();
    let depleted = glow_color(base_color, ratio);
    fade_color(depleted, pulse)
}

fn fog_display<'a>(snapshot: &'a AppSnapshot<'a>, pos: Position) -> (&'a str, Color) {
    let w = snapshot.sim_world.width();
    let h = snapshot.sim_world.height();
    let revealed = |p: Position| {
        let i = p.y * w + p.x;
        snapshot.revealed_cache.get(i).copied().unwrap_or(false)
    };

    let adjacent = [
        (pos.x > 0).then(|| Position { x: pos.x - 1, y: pos.y }),
        (pos.x + 1 < w).then(|| Position { x: pos.x + 1, y: pos.y }),
        (pos.y > 0).then(|| Position { x: pos.x, y: pos.y - 1 }),
        (pos.y + 1 < h).then(|| Position { x: pos.x, y: pos.y + 1 }),
    ]
    .iter()
    .filter_map(|&p| p)
    .any(revealed);

    if adjacent {
        let edge_char = if snapshot.visual_theme.cell_width > 1 { "·" } else { "." };
        (edge_char, fade_color(snapshot.visual_theme.fog_color, 0.15))
    } else {
        (snapshot.visual_theme.fog_char, snapshot.visual_theme.fog_color)
    }
}

fn render_status(f: &mut Frame, snapshot: &AppSnapshot<'_>, area: Rect) {
    let t = snapshot.visual_theme;
    let done = snapshot.cached_remaining == 0;
    let initial = snapshot.cached_initial_total.max(1);
    let progress = if done {
        1.0
    } else {
        (initial - snapshot.cached_remaining) as f64 / initial as f64
    };
    let pct = (progress * 100.0).round() as u16;

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(if done { Color::LightGreen } else { Color::DarkGray }))
        .title(if done {
            Line::from(Span::styled(
                " ✓ Mission complete ",
                Style::default()
                    .fg(Color::LightGreen)
                    .add_modifier(Modifier::BOLD),
            ))
        } else {
            Line::from(Span::styled(
                " Resource Collection ",
                Style::default().fg(Color::Cyan),
            ))
        });

    let inner = block.inner(area);
    f.render_widget(block, area);

    let rows = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(0),
    ])
    .split(inner);

    let row1 = Line::from(vec![
        Span::styled("Tick ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<6}", snapshot.tick),
            Style::default().fg(Color::White).add_modifier(Modifier::BOLD),
        ),
        Span::raw("  │  "),
        Span::styled(
            format!("{} Scouts", snapshot.num_scouts),
            Style::default().fg(t.scout_color),
        ),
        Span::raw("  "),
        Span::styled(
            format!("{} Collectors", snapshot.num_collectors),
            Style::default().fg(t.collector_color),
        ),
    ]);
    f.render_widget(Paragraph::new(row1), rows[0]);

    let row2 = Line::from(vec![
        Span::styled(
            format!(" {} ", t.energy_char),
            Style::default().fg(t.energy_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("Energy ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<6}", snapshot.cached_energy),
            Style::default().fg(t.energy_color),
        ),
        Span::raw("  │  "),
        Span::styled(
            format!(" {} ", t.crystal_char),
            Style::default().fg(t.crystal_color).add_modifier(Modifier::BOLD),
        ),
        Span::styled("Crystals ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{:<6}", snapshot.cached_crystals),
            Style::default().fg(t.crystal_color),
        ),
        Span::raw("  │  "),
        Span::styled("Left ", Style::default().fg(Color::DarkGray)),
        Span::styled(
            format!("{}", snapshot.cached_remaining),
            Style::default().fg(if done { Color::LightGreen } else { Color::Yellow }),
        ),
    ]);
    f.render_widget(Paragraph::new(row2), rows[1]);

    let gauge_color = if done { Color::LightGreen } else { Color::Green };
    let gauge = Gauge::default()
        .gauge_style(Style::default().fg(gauge_color).bg(Color::DarkGray))
        .ratio(progress.min(1.0))
        .label(format!("{pct}% collected"));
    f.render_widget(gauge, rows[2]);

    let hint = if done {
        Line::from(vec![
            Span::styled("Press ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "Q",
                Style::default().fg(Color::LightGreen).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to exit", Style::default().fg(Color::DarkGray)),
        ])
    } else {
        Line::from(vec![
            Span::styled("● ", Style::default().fg(Color::Green)),
            Span::styled("Running", Style::default().fg(Color::White)),
            Span::styled("  —  press ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                "Q",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to quit", Style::default().fg(Color::DarkGray)),
        ])
    };
    f.render_widget(Paragraph::new(hint).alignment(Alignment::Center), rows[3]);
}

fn fade_color(c: Color, factor: f64) -> Color {
    let (r, g, b) = color_rgb(c);
    let f = factor.clamp(0.0, 1.0);
    Color::Rgb((f64::from(r) * f) as u8, (f64::from(g) * f) as u8, (f64::from(b) * f) as u8)
}

fn glow_color(c: Color, ratio: f64) -> Color {
    mix_color(c, Color::White, (1.0 - ratio.clamp(0.0, 1.0)) * 0.5)
}

fn mix_color(a: Color, b: Color, t: f64) -> Color {
    let t = t.clamp(0.0, 1.0);
    let (ar, ag, ab) = color_rgb(a);
    let (br, bg, bb) = color_rgb(b);
    Color::Rgb(
        (ar as f64 + (br as f64 - ar as f64) * t) as u8,
        (ag as f64 + (bg as f64 - ag as f64) * t) as u8,
        (ab as f64 + (bb as f64 - ab as f64) * t) as u8,
    )
}

fn color_rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Black => (0, 0, 0),
        Color::White => (255, 255, 255),
        Color::Red => (255, 0, 0),
        Color::Green => (0, 255, 0),
        Color::Blue => (0, 0, 255),
        Color::Yellow => (255, 255, 0),
        Color::Magenta => (255, 0, 255),
        Color::Cyan => (0, 255, 255),
        Color::Gray => (128, 128, 128),
        Color::DarkGray => (64, 64, 64),
        Color::LightRed => (255, 204, 203),
        Color::LightGreen => (144, 238, 144),
        Color::LightBlue => (173, 216, 230),
        Color::LightCyan => (224, 255, 255),
        Color::LightMagenta => (255, 224, 255),
        Color::LightYellow => (255, 255, 224),
        _ => (128, 128, 128),
    }
}
