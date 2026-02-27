use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyEventKind},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    prelude::*,
    widgets::{Block, Borders, Cell, Paragraph, Row, Table},
};
use std::{collections::{HashMap, HashSet}, io};

// --- 1. Physics Engine v5: Lightweight & Robust ---

// ノート名 -> インデックス (0-11)
fn get_note_mapping() -> HashMap<&'static str, u8> {
    let mut m = HashMap::new();
    m.insert("C", 0); m.insert("C#", 1); m.insert("Db", 1);
    m.insert("D", 2); m.insert("D#", 3); m.insert("Eb", 3);
    m.insert("E", 4); 
    m.insert("F", 5); m.insert("F#", 6); m.insert("Gb", 6);
    m.insert("G", 7); m.insert("G#", 8); m.insert("Ab", 8);
    m.insert("A", 9); m.insert("A#", 10); m.insert("Bb", 10);
    m.insert("B", 11);
    m
}

fn idx_to_note_name(idx: u8) -> &'static str {
    match idx % 12 {
        0 => "C", 1 => "Db", 2 => "D", 3 => "Eb", 4 => "E", 5 => "F",
        6 => "F#", 7 => "G", 8 => "Ab", 9 => "A", 10 => "Bb", 11 => "B",
        _ => "?",
    }
}

// コード定義辞書
// ここに定義を追加すれば、どんな変態コードも即座に対応可能
fn get_quality_intervals(quality: &str) -> Vec<u8> {
    match quality {
        // Basic
        "" | "M" | "maj"          => vec![0, 4, 7],
        "m" | "min" | "-"         => vec![0, 3, 7],
        "dim" | "o"               => vec![0, 3, 6],
        "aug" | "+"               => vec![0, 4, 8],
        "sus4" | "sus"            => vec![0, 5, 7],
        "sus2"                    => vec![0, 2, 7],
        
        // 7th
        "7" | "dom7"              => vec![0, 4, 7, 10],
        "M7" | "maj7" | "Maj7" | "jq" => vec![0, 4, 7, 11],
        "m7" | "min7" | "-7"      => vec![0, 3, 7, 10],
        "mM7" | "mMaj7"           => vec![0, 3, 7, 11],
        "dim7" | "o7"             => vec![0, 3, 6, 9],
        "m7-5" | "m7b5" | "half-dim" | "ø" => vec![0, 3, 6, 10],
        "7sus4"                   => vec![0, 5, 7, 10],
        "6"                       => vec![0, 4, 7, 9],
        "m6"                      => vec![0, 3, 7, 9],

        // Extended (9, 11, 13)
        "9"                       => vec![0, 4, 7, 10, 14],
        "add9"                    => vec![0, 4, 7, 14],
        "M9" | "maj9"             => vec![0, 4, 7, 11, 14],
        "m9" | "min9"             => vec![0, 3, 7, 10, 14], // ★Fm9対応
        "11"                      => vec![0, 4, 7, 10, 14, 17],
        "m11"                     => vec![0, 3, 7, 10, 14, 17],
        "13"                      => vec![0, 4, 7, 10, 14, 21],
        "M13"                     => vec![0, 4, 7, 11, 14, 21],

        // Altered / Fancy
        "7#9"                     => vec![0, 4, 7, 10, 15],
        "7b9"                     => vec![0, 4, 7, 10, 13],
        "7#5" | "aug7"            => vec![0, 4, 8, 10],
        
        // Default: Root + 5th (Power chord fallback)
        _ => vec![0, 7], 
    }
}

// オンコード対応パーサー
// "C/Bb" -> Root: C, Bass: Bb, Notes: [C, E, G, Bb]
fn parse_chord_v5(input: &str) -> (String, String, Vec<u8>) {
    let map = get_note_mapping();
    let s = input.trim();
    if s.is_empty() { return ("?".into(), "".into(), vec![]); }

    // 1. Slash Chord Split
    let parts: Vec<&str> = s.split('/').collect();
    let symbol = parts[0];
    let bass_str = if parts.len() > 1 { parts[1] } else { "" };

    // 2. Root Separation
    // 2文字目(#/b)チェック
    let (root_str, quality_str) = if symbol.len() > 1 {
        let second = symbol.chars().nth(1).unwrap();
        if second == '#' || second == 'b' {
            (&symbol[0..2], &symbol[2..])
        } else {
            (&symbol[0..1], &symbol[1..])
        }
    } else {
        (symbol, "")
    };

    let root_idx = match map.get(root_str) {
        Some(&i) => i,
        None => return (format!("Err:{}", root_str), "".into(), vec![]),
    };

    // 3. Generate Notes
    let intervals = get_quality_intervals(quality_str);
    let mut notes: Vec<u8> = intervals.iter().map(|&i| (root_idx + i) % 12).collect();

    // 4. Add Bass Note (if exists)
    if !bass_str.is_empty() {
        if let Some(&bass_idx) = map.get(bass_str) {
            // ベース音が構成音になければ追加
            if !notes.contains(&bass_idx) {
                // ベース音は通常最低音だが、集合演算上は単に追加でOK
                notes.insert(0, bass_idx); 
            }
        }
    }

    // 表示用ルート名 (オンコードならベースも表記)
    let display_name = if !bass_str.is_empty() {
        format!("{}/{}", root_str, bass_str)
    } else {
        root_str.to_string()
    };

    (display_name, quality_str.to_string(), notes)
}

fn get_scale_mask(root_u8: u8) -> HashSet<u8> {
    let intervals = [0, 2, 4, 5, 7, 9, 11];
    intervals.iter().map(|i| (root_u8 + i) % 12).collect()
}

// --- 1. Physics Engine Logic ---

fn calculate_tonal_depth(chord_notes: &[u8]) -> (Vec<(i32, &'static str)>, usize, bool) {
    let search_order = [
        (0, 0, "C"), (1, 7, "G"), (-1, 5, "F"),
        (2, 2, "D"), (-2, 10, "Bb"),
        (3, 9, "A"), (-3, 3, "Eb"),
        (4, 4, "E"), (-4, 8, "Ab"),
        (5, 11, "B"), (-5, 1, "Db"),
        (6, 6, "F#"), (-6, 6, "Gb"),
    ];
    
    let chord_set: HashSet<u8> = chord_notes.iter().cloned().collect();
    let total = chord_set.len();
    
    let mut max_score = 0;
    let mut candidates: Vec<(i32, &'static str)> = Vec::new();

    for (depth, r_idx, r_name) in search_order {
        let scale = get_scale_mask(r_idx);
        let score = chord_set.intersection(&scale).count();
        
        if score > max_score {
            max_score = score;
            candidates.clear();
            candidates.push((depth, r_name));
        } else if score == max_score {
            candidates.push((depth, r_name));
        }
    }
    
    let is_perfect = max_score == total;

    // ★ 修正点: 完全一致(Perfect)なら、最小作用の原理で1つに絞る
    if is_perfect && !candidates.is_empty() {
        // 絶対値が最も小さい(=Cに近い)ものを探す
        // sort_by_key は安定ソートなので、同距離(+6と-6など)ならsearch_order順が優先される
        candidates.sort_by_key(|k| k.0.abs());
        candidates.truncate(1); // 先頭の1つだけ残す
    }
    
    (candidates, max_score, is_perfect)
}

fn get_interval_label(root_idx: u8, target_idx: u8) -> &'static str {
    let diff = (target_idx + 12 - root_idx) % 12;
    match diff {
        0 => "R", 1 => "b9", 2 => "9", 3 => "m3", 4 => "M3", 5 => "11",
        6 => "#11", 7 => "5", 8 => "b13", 9 => "13", 10 => "m7", 11 => "M7", _ => "?"
    }
}

// --- 2. App State ---
enum InputMode {
    Chord,
    Tuning,
}

struct App {
    input: String,
    progression: Vec<String>,
    tuning_input: String,
    tuning: Vec<u8>,
    key: u8,
    input_mode: InputMode,
}

impl App {
    fn new() -> Self {
        Self {
            input: String::new(),
            // テスト: Fm9, オンコード(C/Bb), テンション(G13)
            progression: vec!["Fm9".into(), "C/Bb".into(), "G13".into(), "Dbdim7".into()],
            tuning_input: "C G D G A D".to_string(),
            tuning: vec![0, 7, 2, 7, 9, 2], // C G D G A D
            input_mode: InputMode::Chord,
            key: 0,
        }
    }

    fn submit(&mut self) {
        // 1. 入力モードの判定
        match self.input_mode {
            InputMode::Chord => {
                // 2. コード入力モードの場合
                if !self.input.is_empty() {
                    self.progression = self.input.split_whitespace().map(|s| s.to_string()).collect();
                    self.input.clear();
                }
            },
            InputMode::Tuning => {
                // 3. チューニング入力モードの場合
                if !self.tuning_input.is_empty() && self.tuning_input.split_whitespace().count() == 6 {
                    self.tuning = self.tuning_input.split_whitespace().map(|s| *get_note_mapping().get(s).unwrap_or(&0)).collect();
                }
            }
        }

        if !self.input.is_empty() {
            self.progression = self.input.split_whitespace().map(|s| s.to_string()).collect();
            self.input.clear();
        }

        if !self.tuning_input.is_empty() && self.tuning_input.split_whitespace().count() == 6 {
            self.tuning = self.tuning_input.split_whitespace().map(|s| *get_note_mapping().get(s).unwrap_or(&0)).collect();
        }
    }
}

// --- 3. UI ---

fn main() -> Result<()> {
    enable_raw_mode()?;
    let mut stderr = io::stderr();
    execute!(stderr, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stderr);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new();
    loop {
        terminal.draw(|f| ui(f, &mut app))?;
        if let Event::Key(key) = event::read()? {
            if key.kind == KeyEventKind::Press {
                match key.code {
                    KeyCode::Esc => break,
                    KeyCode::Enter => app.submit(),
                    KeyCode::Up => app.key = (app.key + 1) % 12,
                    KeyCode::Down => app.key = (app.key + 11) % 12,
                    _ => {}
                }

                match app.input_mode {
                    InputMode::Chord => {
                        match key.code {
                            KeyCode::Char(c) => app.input.push(c),
                            KeyCode::Backspace => { app.input.pop(); },
                            KeyCode::Tab => app.input_mode = InputMode::Tuning,
                            _ => {}
                        }
                    },
                    InputMode::Tuning => {
                        match key.code {
                            KeyCode::Char(c) => app.tuning_input.push(c),
                            KeyCode::Backspace => { app.tuning_input.pop(); },
                            KeyCode::Tab => app.input_mode = InputMode::Chord,
                            _ => {}
                        }
                    }
                }
            }
        }
    }
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    Ok(())
}

fn ui(f: &mut Frame, app: &mut App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(1), Constraint::Length(1)])
        .split(f.size());

    let current_key_name = idx_to_note_name(app.key);

    match app.input_mode {
        InputMode::Chord => {
            // ★ タイトルに現在のKeyを埋め込む
            let title = format!(" Input Chords (Key: {}) ", current_key_name);
            let input_p = Paragraph::new(app.input.as_str())
            .block(Block::default().borders(Borders::ALL).title(title))
            .style(Style::default().fg(Color::Cyan));
            f.render_widget(input_p, chunks[0]);
        },
        InputMode::Tuning => {
            // ★ こちらも同様に
            let title = format!(" Input Tuning (Key: {}) ", current_key_name);
            let input_p = Paragraph::new(app.tuning_input.as_str())
            .block(Block::default().borders(Borders::ALL).title(title))
            .style(Style::default().fg(Color::Cyan));
            f.render_widget(input_p, chunks[0]);
        }
    }

    let mut header_cells = vec![
        Cell::from("Chord").style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Cell::from("Depth").style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Cell::from("Local Key").style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
        Cell::from("Notes").style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)),
    ];

    let string_count = app.tuning.len();
    // 弦の数に合わせて「6(C), 5(G)...」を生成
    for (i, &note_idx) in app.tuning.iter().enumerate() {
        let string_num = string_count - i;
        let note_name = idx_to_note_name(note_idx);
        let header_label = format!("{}({})", string_num, note_name);
        header_cells.push(Cell::from(header_label).style(Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)));
    }

    let header = Row::new(header_cells).height(1).bottom_margin(1);

    let rows = app.progression.iter().map(|chord_str| {
        let (root_disp, _quality, notes) = parse_chord_v5(chord_str);
        let relative_notes = notes.iter().map(|n| (n + 12 - app.key) % 12).collect::<Vec<u8>>();
        
        let parts: Vec<&str> = root_disp.split('/').collect();
        let root_idx = *get_note_mapping().get(parts[0]).unwrap_or(&0);

        // ★ 変更点: 複数の候補を受け取る
        let (candidates, _score, perfect) = calculate_tonal_depth(&relative_notes);
        let display_candidates: Vec<(i32, String)> = candidates.iter()
            .map(|&(d, s)| {
                let rel_key_idx = *get_note_mapping().get(s).unwrap_or(&0);
                let abs_key_idx = (rel_key_idx + app.key) % 12; // 実際の音階に戻す
                let abs_key_name = idx_to_note_name(abs_key_idx).to_string();
                (d, abs_key_name)
            })
            .collect();

        // 代表値（スケール表示用）
        let key_root_name = display_candidates.first().map(|c| c.1.as_str()).unwrap_or("C");
        let key_root_idx = *get_note_mapping().get(key_root_name).unwrap_or(&0);
        let scale_notes = get_scale_mask(key_root_idx);

        let mut cells = Vec::new();
        cells.push(Cell::from(chord_str.as_str()).style(Style::default().add_modifier(Modifier::BOLD)));
        
        // Depth表示 (display_candidates を使うように変更)
        let depth_str = display_candidates.iter()
            .map(|(d, _)| format!("{:+}", d))
            .collect::<Vec<_>>()
            .join(" ");
            
        let d_style = if perfect {
            let rep_depth = display_candidates[0].0; 
            let c = if rep_depth == 0 { Color::Green } else if rep_depth.abs() <= 1 { Color::Yellow } else { Color::Red };
            Style::default().fg(c)
        } else {
            Style::default().fg(Color::Magenta).add_modifier(Modifier::ITALIC)
        };
        cells.push(Cell::from(depth_str).style(d_style));

        // Key表示 (display_candidates を使うように変更)
        let key_str = display_candidates.iter()
            .map(|(_, name)| name.clone())
            .collect::<Vec<_>>()
            .join(" ");
        cells.push(Cell::from(key_str));
        // ... (Notes, Strings表示は変更なし) ...
        
        let note_names: Vec<String> = notes.iter().map(|&i| idx_to_note_name(i).to_string()).collect();
        cells.push(Cell::from(note_names.join(" ")).style(Style::default().fg(Color::DarkGray)));

        for &t_idx in &app.tuning {
            let interval = get_interval_label(root_idx, t_idx);
            let in_chord = notes.contains(&t_idx);
            let in_scale = scale_notes.contains(&t_idx);

            let (txt, sty) = if in_chord {
                (interval.to_string(), Style::default().fg(Color::Green).add_modifier(Modifier::BOLD))
            } else if in_scale {
                (interval.to_string(), Style::default().fg(Color::Cyan))
            } else {
                (format!("X({})", interval), Style::default().fg(Color::Red))
            };
            cells.push(Cell::from(txt).style(sty));
        }

        Row::new(cells)
    });

   let mut constraints = vec![
        Constraint::Percentage(12), // Chord
        Constraint::Percentage(10), // Depth
        Constraint::Percentage(10), // Local Key
        Constraint::Percentage(14), // Notes
    ];
    let string_width = 54 / string_count.max(1) as u16; // ゼロ除算防止
    for _ in 0..string_count {
        constraints.push(Constraint::Percentage(string_width));
    }

    let table = Table::new(rows, constraints)
        .header(header)
        .block(Block::default().borders(Borders::ALL).title(" Physics Engine v5 (Zero-Dependency) "));

    f.render_widget(table, chunks[1]);
    
    let footer = Paragraph::new("Ultra-Lightweight Mode | No ML, No Audio | Esc to Quit")
        .style(Style::default().fg(Color::DarkGray));
    f.render_widget(footer, chunks[2]);
}
